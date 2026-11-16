//! The change-journal source of catalog v2: reads the NTFS USN journal
//! without administrator rights and turns its records into `FsChange` values.
//!
//! What was measured (spike 2026-11-16, see `oploop/catalog-v2/SPEC.md`):
//! - A handle to the volume ROOT FOLDER (`X:\`, access `FILE_READ_ATTRIBUTES`,
//!   `FILE_FLAG_BACKUP_SEMANTICS`) accepts `FSCTL_QUERY_USN_JOURNAL` and
//!   `FSCTL_READ_UNPRIVILEGED_USN_JOURNAL`. The device `\\.\X:` needs admin.
//! - The unprivileged read works only when `StartUsn` is a record boundary:
//!   0, `FirstUsn`, `NextUsn`, or the `next_usn` of an earlier read. Any other
//!   value gives `ERROR_INVALID_PARAMETER` (87).
//! - The records have no file name. `resolve` opens the file by its id and
//!   asks for its name.
//!
//! The handle is a synchronous handle: Windows runs one call at a time on
//! it. Use one thread per volume for `read` and `resolve`. Reads never wait
//! for new records (see `read_into`): the caller polls.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_JOURNAL_DELETE_IN_PROGRESS, ERROR_JOURNAL_ENTRY_DELETED,
    ERROR_JOURNAL_NOT_ACTIVE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileAttributeTagInfo, FileIdType, FileNameInfo, GetFileInformationByHandleEx,
    OpenFileById, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_UNPRIVILEGED_USN_JOURNAL, READ_USN_JOURNAL_DATA_V1,
    USN_JOURNAL_DATA_V0, USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE,
    USN_REASON_HARD_LINK_CHANGE, USN_REASON_RENAME_NEW_NAME,
};
use windows::Win32::System::IO::DeviceIoControl;

use super::types::{JournalCheckpoint, JournalMetadata};
use super::store::{make_frn, split_frn, FsChange, FsNode};
use super::walk::{is_link_folder, Exclusions};

/// Bytes of the output buffer of one read.
const READ_BUFFER_BYTES: usize = 1024 * 1024;

/// Smallest size of a V2 record (fixed part, no name) and of a V3 record.
const USN_V2_HEADER: usize = 60;
const USN_V3_HEADER: usize = 76;

/// The reasons that the catalog needs. The read lets the kernel skip every
/// other record (data writes are the bulk of the journal traffic). A record
/// that has one of these bits together with others still matches. The close
/// bit and `RENAME_OLD_NAME` are not needed: the new-name record carries the
/// move. A hard link change can change the name of the one entry of a file.
const REASON_MASK: u32 = USN_REASON_FILE_CREATE
    | USN_REASON_FILE_DELETE
    | USN_REASON_RENAME_NEW_NAME
    | USN_REASON_HARD_LINK_CHANGE;

// ---------------------------------------------------------------------------
// Continuity (moved from ntfs/usn.rs)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalContinuity {
    /// The checkpoint is the end of the journal: nothing to read.
    Current,
    /// The journal holds everything since the checkpoint: read it.
    CatchUp,
    /// The history is lost: walk the volume again.
    Rebuild(RebuildReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebuildReason {
    MissingCheckpoint,
    JournalIdChanged,
    CursorTruncated,
    CursorAhead,
}

pub fn journal_continuity(
    checkpoint: Option<JournalCheckpoint>,
    journal: JournalMetadata,
) -> JournalContinuity {
    let Some(checkpoint) = checkpoint else {
        return JournalContinuity::Rebuild(RebuildReason::MissingCheckpoint);
    };
    if checkpoint.journal_id != journal.journal_id {
        return JournalContinuity::Rebuild(RebuildReason::JournalIdChanged);
    }
    let oldest = journal.first_usn.max(journal.lowest_valid_usn);
    if checkpoint.next_usn < oldest {
        return JournalContinuity::Rebuild(RebuildReason::CursorTruncated);
    }
    if checkpoint.next_usn > journal.next_usn {
        return JournalContinuity::Rebuild(RebuildReason::CursorAhead);
    }
    if checkpoint.next_usn == journal.next_usn {
        JournalContinuity::Current
    } else {
        JournalContinuity::CatchUp
    }
}

// ---------------------------------------------------------------------------
// Errors and records
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalError {
    /// The volume has no active journal: it can only be walked.
    NotActive(String),
    /// The journal no longer holds the start USN, or it is being deleted.
    HistoryLost(String),
    /// Any other failure.
    Failed(String),
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive(text) | Self::HistoryLost(text) | Self::Failed(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<JournalError> for String {
    fn from(error: JournalError) -> String {
        error.to_string()
    }
}

/// One journal record, without a name (the unprivileged read gives none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsnRecord {
    /// File id with its sequence number (V3: the low 64 bits of the 128-bit id).
    pub frn: u64,
    pub parent_frn: u64,
    pub usn: i64,
    pub reason: u32,
    pub attributes: u32,
}

impl UsnRecord {
    pub fn is_dir(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalBatch {
    /// Where the next read starts. Always a record boundary.
    pub next_usn: i64,
    pub records: Vec<UsnRecord>,
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?))
}

fn le_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?))
}

fn le_i64(bytes: &[u8], offset: usize) -> Option<i64> {
    le_u64(bytes, offset).map(|value| value as i64)
}

/// Reads the records that follow the 8-byte cursor of a read result. A record
/// of a major version other than 2 or 3 is skipped by its length.
pub fn parse_records(bytes: &[u8]) -> Result<Vec<UsnRecord>, String> {
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let rest = &bytes[offset..];
        let (Some(length), Some(major)) = (le_u32(rest, 0), le_u16(rest, 4)) else {
            return Err("USN record is shorter than its common header".to_string());
        };
        let length = length as usize;
        if length < 8 || length > rest.len() {
            return Err("USN record length is outside the returned buffer".to_string());
        }
        let record = &rest[..length];
        let fields = match major {
            2 if length >= USN_V2_HEADER => Some((
                le_u64(record, 8),
                le_u64(record, 16),
                le_i64(record, 24),
                le_u32(record, 40),
                le_u32(record, 52),
            )),
            // A 128-bit id: its low 64 bits come first (little endian).
            3 if length >= USN_V3_HEADER => Some((
                le_u64(record, 8),
                le_u64(record, 24),
                le_i64(record, 40),
                le_u32(record, 56),
                le_u32(record, 68),
            )),
            2 | 3 => return Err(format!("USN v{major} record is shorter than its fixed header")),
            _ => None,
        };
        if let Some((Some(frn), Some(parent_frn), Some(usn), Some(reason), Some(attributes))) = fields {
            records.push(UsnRecord { frn, parent_frn, usn, reason, attributes });
        }
        offset += length;
    }
    Ok(records)
}

// ---------------------------------------------------------------------------
// Fold
// ---------------------------------------------------------------------------

/// The final action for one file id in a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folded {
    Remove { record: u64, seq: u16 },
    /// Look the file up now: its name and place are the current ones.
    /// `from_path`: a hard link changed. The record's parent can be the folder
    /// of a link that is gone, so the parent is read from the current path.
    Resolve { record: u64, seq: u16, parent_record: u64, is_dir: bool, from_path: bool },
}

/// One final action per file id, in the order the ids first appear in the
/// batch (a folder is created before its children, so a parent comes first).
///
/// Rules, per file id (the id includes the sequence number):
/// - a record with `FILE_DELETE` ends it with a remove;
/// - `FILE_CREATE`, `RENAME_NEW_NAME` or `HARD_LINK_CHANGE` ends it with a
///   resolve (the last such record gives the parent);
/// - `RENAME_OLD_NAME` and any other reason change nothing;
/// - a file created and deleted in the same batch gives nothing at all.
pub fn fold(records: &[UsnRecord]) -> Vec<Folded> {
    fold_with(records, false)
}

/// `fold`. With `remove_created_and_deleted` a file created and deleted in
/// the same batch gives a remove instead of nothing. The journal can hold the
/// same create bit in two records of one open handle (the second one with
/// `CLOSE`), and those can fall into two reads: the create may then have been
/// resolved by the earlier read, so the delete must still remove it. A remove
/// of an unknown id does nothing in the store.
fn fold_with(records: &[UsnRecord], remove_created_and_deleted: bool) -> Vec<Folded> {
    #[derive(Clone, Copy)]
    enum Action {
        None,
        Remove,
        Resolve { parent_frn: u64, is_dir: bool },
    }
    struct Slot {
        frn: u64,
        created: bool,
        link_change: bool,
        action: Action,
    }

    let mut slots: Vec<Slot> = Vec::new();
    let mut index: HashMap<u64, usize> = HashMap::new();
    for record in records {
        let reason = record.reason;
        if reason & REASON_MASK == 0 {
            continue;
        }
        let at = *index.entry(record.frn).or_insert_with(|| {
            slots.push(Slot { frn: record.frn, created: false, link_change: false, action: Action::None });
            slots.len() - 1
        });
        let slot = &mut slots[at];
        if reason & USN_REASON_FILE_CREATE != 0 {
            slot.created = true;
        }
        if reason & USN_REASON_HARD_LINK_CHANGE != 0 {
            slot.link_change = true;
        }
        if reason & USN_REASON_FILE_DELETE != 0 {
            slot.action = if slot.created && !remove_created_and_deleted {
                Action::None
            } else {
                Action::Remove
            };
        } else if matches!(slot.action, Action::Remove) {
            // Nothing follows a delete of the same id.
        } else {
            slot.action = Action::Resolve { parent_frn: record.parent_frn, is_dir: record.is_dir() };
        }
    }

    slots
        .into_iter()
        .filter_map(|slot| {
            let (record, seq) = split_frn(slot.frn);
            match slot.action {
                Action::None => None,
                Action::Remove => Some(Folded::Remove { record, seq }),
                Action::Resolve { parent_frn, is_dir } => Some(Folded::Resolve {
                    record,
                    seq,
                    parent_record: split_frn(parent_frn).0,
                    is_dir,
                    from_path: slot.link_change,
                }),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The volume
// ---------------------------------------------------------------------------

/// An open volume root with its journal.
pub struct JournalVolume {
    handle: HANDLE,
    /// Lowercase path of the volume root with no trailing separator (`c:`).
    /// `resolve` puts it before the path below the root.
    prefix: String,
}

// SAFETY: a kernel handle can be used from any thread. The calls on it are
// serialized by Windows (synchronous handle).
unsafe impl Send for JournalVolume {}
unsafe impl Sync for JournalVolume {}

impl JournalVolume {
    /// Opens the volume root folder (`X:\`; a mount folder works too). It does
    /// not open `\\.\X:`: that needs administrator rights.
    pub fn open(root: &Path) -> Result<Self, String> {
        let text = root.to_string_lossy();
        let trimmed = text.trim_end_matches(['\\', '/']);
        let wide: Vec<u16> = format!("{trimmed}\\").encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_READ_ATTRIBUTES.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(|error| format!("open volume root {}: {error}", root.display()))?;
        let prefix = trimmed.strip_prefix(r"\\?\").unwrap_or(trimmed).to_lowercase();
        Ok(Self { handle, prefix })
    }

    /// The journal id and its USN range.
    pub fn query(&self) -> Result<JournalMetadata, JournalError> {
        let mut output = vec![0u8; std::mem::size_of::<USN_JOURNAL_DATA_V0>()];
        let returned = self.ioctl::<u8>(FSCTL_QUERY_USN_JOURNAL, None, &mut output)?;
        parse_query(&output[..returned.min(output.len())])
    }

    /// Reads the records after `start_usn`, without waiting. An empty batch
    /// means the end of the journal; its `next_usn` is where to read next.
    pub fn read(&self, start_usn: i64, journal_id: u64) -> Result<JournalBatch, JournalError> {
        self.read_into(&mut Vec::new(), start_usn, journal_id)
    }

    /// `read` with a buffer that the caller keeps: a reader that loops does
    /// not allocate one buffer for each read.
    pub fn read_with(
        &self,
        buffer: &mut Vec<u8>,
        start_usn: i64,
        journal_id: u64,
    ) -> Result<JournalBatch, JournalError> {
        self.read_into(buffer, start_usn, journal_id)
    }

    fn read_into(
        &self,
        output: &mut Vec<u8>,
        start_usn: i64,
        journal_id: u64,
    ) -> Result<JournalBatch, JournalError> {
        // No wait: measured on a real C: (2026-11-16), a read with
        // `BytesToWaitFor` = 1 ignored its 1 s `Timeout` and blocked 13 s until
        // a record that the mask keeps came. A blocked read cannot see a stop
        // request. The reader polls instead.
        let input = READ_USN_JOURNAL_DATA_V1 {
            StartUsn: start_usn,
            ReasonMask: REASON_MASK,
            ReturnOnlyOnClose: 0,
            Timeout: 0,
            BytesToWaitFor: 0,
            UsnJournalID: journal_id,
            MinMajorVersion: 2,
            MaxMajorVersion: 3,
        };
        output.resize(READ_BUFFER_BYTES, 0);
        let returned = self.ioctl(FSCTL_READ_UNPRIVILEGED_USN_JOURNAL, Some(&input), output)?;
        if returned < 8 || returned > output.len() {
            return Err(JournalError::Failed(
                "journal read returned an invalid buffer length".to_string(),
            ));
        }
        let next_usn = le_i64(output, 0).ok_or_else(|| {
            JournalError::Failed("journal response did not contain a cursor".to_string())
        })?;
        let records = parse_records(&output[8..returned]).map_err(JournalError::Failed)?;
        Ok(JournalBatch { next_usn, records })
    }

    /// The current name and place of a file: opens it by id and reads its
    /// path below the volume root. `frn` includes the sequence number, so the
    /// open fails when the record was reused. `None` when the open fails (the
    /// file is gone, or no access): a later delete record removes it.
    /// `excluded` is set by the exclusion list for a folder. `link` is set for
    /// a junction, a symlink or a mount point (read from the opened file).
    /// With `parent_from_path` the parent is the folder of the current path,
    /// not `parent_record`.
    pub fn resolve(
        &self,
        frn: u64,
        parent_record: u64,
        is_dir: bool,
        parent_from_path: bool,
        exclusions: &Exclusions,
    ) -> Option<FsNode> {
        let descriptor = FILE_ID_DESCRIPTOR {
            dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32,
            Type: FileIdType,
            Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: frn as i64 },
        };
        // SAFETY: `descriptor` is filled and outlives the call. The volume
        // handle is the hint for the volume of the id. The reparse flag opens
        // a junction or a symlink as itself.
        let file = unsafe {
            OpenFileById(
                self.handle,
                &descriptor,
                FILE_READ_ATTRIBUTES.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            )
        }
        .ok()?;
        let path = name_below_root(file);
        let link = is_dir && is_link_handle(file);
        // SAFETY: `file` was opened above and is closed once.
        let _ = unsafe { CloseHandle(file) };
        let path = path?;

        let name = path.rsplit('\\').next().filter(|name| !name.is_empty())?.to_string();
        let parent_record = if parent_from_path {
            let folder = path.rsplit_once('\\').map_or("", |(folder, _)| folder);
            let folder = format!("{}{folder}", self.prefix);
            super::walk::root_record(Path::new(&folder)).ok()?
        } else {
            parent_record
        };
        let excluded = is_dir && {
            let lower_name = name.to_lowercase();
            let lower_path = format!("{}{}", self.prefix, path.to_lowercase());
            exclusions.is_excluded(&lower_path, &lower_name)
        };
        let (record, seq) = split_frn(frn);
        Some(FsNode { record, seq, parent_record, name, is_dir, excluded, link })
    }

    /// Turns one read into store changes. `accepts_children(record)` says
    /// whether the store holds the folder `record` as a live, not excluded
    /// folder. A new or renamed entry whose parent is not accepted is not
    /// opened: it gives a `Remove` instead (the entry may be a known one that
    /// moved out of the index, to the recycle bin or an excluded folder; a
    /// remove of an unknown id does nothing). A folder that this same batch
    /// creates counts as accepted for the entries after it.
    pub fn changes(
        &self,
        batch: &JournalBatch,
        accepts_children: &dyn Fn(u64) -> bool,
        exclusions: &Exclusions,
    ) -> Vec<FsChange> {
        let mut changes = Vec::new();
        let mut new_folders = std::collections::HashSet::new();
        for action in fold_with(&batch.records, true) {
            match action {
                Folded::Remove { record, seq } => changes.push(FsChange::Remove { record, seq }),
                Folded::Resolve { record, seq, parent_record, is_dir, from_path } => {
                    // With `from_path` the parent is not known yet: the store
                    // drops an entry whose parent is not accepted.
                    if !from_path
                        && !new_folders.contains(&parent_record)
                        && !accepts_children(parent_record)
                    {
                        changes.push(FsChange::Remove { record, seq });
                        continue;
                    }
                    if let Some(node) = self.resolve(
                        make_frn(record, seq),
                        parent_record,
                        is_dir,
                        from_path,
                        exclusions,
                    ) {
                        if node.is_dir && !node.excluded {
                            new_folders.insert(record);
                        }
                        changes.push(FsChange::Upsert(node));
                    }
                }
            }
        }
        changes
    }

    fn ioctl<I: Sized>(
        &self,
        control_code: u32,
        input: Option<&I>,
        output: &mut [u8],
    ) -> Result<usize, JournalError> {
        let mut returned = 0u32;
        // SAFETY: the buffers are valid for the sizes given; `returned` is a
        // local that the call writes.
        let result = unsafe {
            DeviceIoControl(
                self.handle,
                control_code,
                input.map(|value| value as *const I as *const c_void),
                input.map_or(0, |_| std::mem::size_of::<I>() as u32),
                Some(output.as_mut_ptr().cast()),
                output.len() as u32,
                Some(&mut returned),
                None,
            )
        };
        result.map_err(|error| classify(&error))?;
        Ok(returned as usize)
    }
}

impl Drop for JournalVolume {
    fn drop(&mut self) {
        // SAFETY: the handle was opened in `open` and is closed once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

fn classify(error: &windows::core::Error) -> JournalError {
    // DeviceIoControl gives HRESULT_FROM_WIN32; its low 16 bits are the system error.
    let code = error.code().0 as u32 & 0xffff;
    if code == ERROR_JOURNAL_NOT_ACTIVE.0 {
        JournalError::NotActive(format!("the volume has no active USN journal: {error}"))
    } else if code == ERROR_JOURNAL_ENTRY_DELETED.0 || code == ERROR_JOURNAL_DELETE_IN_PROGRESS.0 {
        JournalError::HistoryLost(format!("USN journal history is no longer continuous: {error}"))
    } else {
        JournalError::Failed(format!("USN journal call failed: {error}"))
    }
}

/// Reads the output of `FSCTL_QUERY_USN_JOURNAL` (`USN_JOURNAL_DATA_V0` or later).
fn parse_query(bytes: &[u8]) -> Result<JournalMetadata, JournalError> {
    let field = |value: Option<u64>, what: &str| {
        value.ok_or_else(|| JournalError::Failed(format!("journal response omitted {what}")))
    };
    if bytes.len() < 32 {
        return Err(JournalError::Failed(
            "FSCTL_QUERY_USN_JOURNAL returned an invalid buffer length".to_string(),
        ));
    }
    Ok(JournalMetadata {
        journal_id: field(le_u64(bytes, 0), "its id")?,
        first_usn: field(le_u64(bytes, 8), "FirstUsn")? as i64,
        next_usn: field(le_u64(bytes, 16), "NextUsn")? as i64,
        lowest_valid_usn: field(le_u64(bytes, 24), "LowestValidUsn")? as i64,
    })
}

/// True when the open file is a link folder (see `is_link_folder`). A failed
/// query gives false: the folder is then walked, as before.
fn is_link_handle(file: HANDLE) -> bool {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: `info` is writable for its whole size.
    let read = unsafe {
        GetFileInformationByHandleEx(
            file,
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    read.is_ok() && is_link_folder(info.FileAttributes, info.ReparseTag)
}

/// Reads the path below the volume root of an open file (`\Users\x\a.txt`).
fn name_below_root(file: HANDLE) -> Option<String> {
    // First a small buffer, then one that holds the longest NTFS path.
    for bytes in [2 * 1024usize, 68 * 1024] {
        let mut buffer = vec![0u64; bytes / 8];
        // SAFETY: `buffer` is writable for `bytes` bytes and 8-byte aligned.
        let read = unsafe {
            GetFileInformationByHandleEx(file, FileNameInfo, buffer.as_mut_ptr().cast(), bytes as u32)
        };
        if read.is_err() {
            continue;
        }
        // SAFETY: the call filled the buffer; the slice covers it.
        let raw = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), bytes) };
        return parse_file_name_info(raw);
    }
    None
}

/// Reads a `FILE_NAME_INFO`: a byte length, then UTF-16 text.
fn parse_file_name_info(raw: &[u8]) -> Option<String> {
    let length = le_u32(raw, 0)? as usize;
    if !length.is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = raw
        .get(4..4usize.checked_add(length)?)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE_ID_A: u64 = 0x0002_0000_0000_0064; // record 100, seq 2
    const FILE_ID_B: u64 = 0x0001_0000_0000_00C8; // record 200, seq 1
    const PARENT: u64 = 0x0005_0000_0000_0005; // record 5

    fn put(buffer: &mut [u8], offset: usize, bytes: &[u8]) {
        buffer[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    /// A V2 record with no name (length 64: header 60 padded to 8).
    fn v2(frn: u64, parent: u64, usn: i64, reason: u32, attributes: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        put(&mut bytes, 0, &64u32.to_le_bytes());
        put(&mut bytes, 4, &2u16.to_le_bytes());
        put(&mut bytes, 8, &frn.to_le_bytes());
        put(&mut bytes, 16, &parent.to_le_bytes());
        put(&mut bytes, 24, &usn.to_le_bytes());
        put(&mut bytes, 40, &reason.to_le_bytes());
        put(&mut bytes, 52, &attributes.to_le_bytes());
        bytes
    }

    /// A V3 record with no name (length 80: header 76 padded to 8).
    fn v3(frn: u64, parent: u64, usn: i64, reason: u32, attributes: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 80];
        put(&mut bytes, 0, &80u32.to_le_bytes());
        put(&mut bytes, 4, &3u16.to_le_bytes());
        put(&mut bytes, 8, &frn.to_le_bytes());
        // The upper 64 bits of both ids carry a value that must be ignored.
        put(&mut bytes, 16, &0xFFFF_FFFF_FFFF_FFFFu64.to_le_bytes());
        put(&mut bytes, 24, &parent.to_le_bytes());
        put(&mut bytes, 32, &0xFFFF_FFFF_FFFF_FFFFu64.to_le_bytes());
        put(&mut bytes, 40, &usn.to_le_bytes());
        put(&mut bytes, 56, &reason.to_le_bytes());
        put(&mut bytes, 68, &attributes.to_le_bytes());
        bytes
    }

    fn rec(frn: u64, parent: u64, reason: u32, attributes: u32) -> UsnRecord {
        UsnRecord { frn, parent_frn: parent, usn: 0, reason, attributes }
    }

    const FILE: u32 = 0x20;
    const DIR: u32 = FILE_ATTRIBUTE_DIRECTORY.0;
    const CREATE: u32 = USN_REASON_FILE_CREATE;
    const DELETE: u32 = USN_REASON_FILE_DELETE;
    const NEW_NAME: u32 = USN_REASON_RENAME_NEW_NAME;
    const OLD_NAME: u32 = windows::Win32::System::Ioctl::USN_REASON_RENAME_OLD_NAME;
    const CLOSE: u32 = windows::Win32::System::Ioctl::USN_REASON_CLOSE;
    const DATA: u32 = windows::Win32::System::Ioctl::USN_REASON_DATA_EXTEND;

    // --- record parsing ---

    #[test]
    fn v2_records_are_parsed() {
        let mut buffer = v2(FILE_ID_A, PARENT, 1000, CREATE | CLOSE, FILE);
        buffer.extend(v2(FILE_ID_B, FILE_ID_A, 1064, DELETE, DIR));
        let records = parse_records(&buffer).unwrap();
        assert_eq!(
            records,
            vec![
                UsnRecord { frn: FILE_ID_A, parent_frn: PARENT, usn: 1000, reason: CREATE | CLOSE, attributes: FILE },
                UsnRecord { frn: FILE_ID_B, parent_frn: FILE_ID_A, usn: 1064, reason: DELETE, attributes: DIR },
            ]
        );
        assert!(records[1].is_dir());
        assert!(!records[0].is_dir());
    }

    #[test]
    fn v3_records_keep_the_low_64_bits_of_the_ids() {
        let mut buffer = v3(FILE_ID_A, PARENT, 5000, NEW_NAME, DIR);
        buffer.extend(v2(FILE_ID_B, PARENT, 5080, CREATE, FILE));
        let records = parse_records(&buffer).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].frn, FILE_ID_A);
        assert_eq!(records[0].parent_frn, PARENT);
        assert_eq!(records[0].usn, 5000);
        assert_eq!(records[0].reason, NEW_NAME);
        assert_eq!(records[0].attributes, DIR);
        assert_eq!(records[1].frn, FILE_ID_B);
    }

    #[test]
    fn an_empty_buffer_has_no_records() {
        assert_eq!(parse_records(&[]).unwrap(), vec![]);
    }

    #[test]
    fn a_truncated_buffer_is_an_error() {
        let full = v2(FILE_ID_A, PARENT, 1, CREATE, FILE);
        // The record is cut: its length is longer than the bytes.
        assert!(parse_records(&full[..40]).is_err());
        // Less than a common header.
        assert!(parse_records(&full[..6]).is_err());
        // A second record that is cut after a whole first one.
        let mut two = full.clone();
        two.extend(&full[..20]);
        assert!(parse_records(&two).is_err());
    }

    #[test]
    fn a_bad_record_length_is_an_error() {
        let mut zero = v2(FILE_ID_A, PARENT, 1, CREATE, FILE);
        put(&mut zero, 0, &0u32.to_le_bytes());
        assert!(parse_records(&zero).is_err());
        // A V2 record shorter than its fixed header.
        let mut short = v2(FILE_ID_A, PARENT, 1, CREATE, FILE);
        put(&mut short, 0, &32u32.to_le_bytes());
        assert!(parse_records(&short).is_err());
    }

    #[test]
    fn an_unknown_version_is_skipped_by_its_length() {
        let mut other = vec![0u8; 24];
        put(&mut other, 0, &24u32.to_le_bytes());
        put(&mut other, 4, &4u16.to_le_bytes());
        other.extend(v2(FILE_ID_B, PARENT, 9, CREATE, FILE));
        let records = parse_records(&other).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].frn, FILE_ID_B);
    }

    #[test]
    fn the_query_output_is_parsed() {
        let mut bytes = vec![0u8; 56];
        put(&mut bytes, 0, &0xABCDu64.to_le_bytes());
        put(&mut bytes, 8, &100i64.to_le_bytes());
        put(&mut bytes, 16, &900i64.to_le_bytes());
        put(&mut bytes, 24, &120i64.to_le_bytes());
        let metadata = parse_query(&bytes).unwrap();
        assert_eq!(
            metadata,
            JournalMetadata { journal_id: 0xABCD, first_usn: 100, next_usn: 900, lowest_valid_usn: 120 }
        );
        assert!(parse_query(&bytes[..20]).is_err());
    }

    #[test]
    fn a_file_name_info_is_parsed() {
        let text = r"\Users\hi\a.txt";
        let units: Vec<u8> = text.encode_utf16().flat_map(|unit| unit.to_le_bytes()).collect();
        let mut raw = (units.len() as u32).to_le_bytes().to_vec();
        raw.extend(&units);
        raw.extend([0u8; 16]);
        assert_eq!(parse_file_name_info(&raw).as_deref(), Some(text));
        // A length that goes past the buffer.
        assert_eq!(parse_file_name_info(&raw[..10]), None);
    }

    // --- fold ---

    fn resolve(frn: u64, parent: u64, is_dir: bool) -> Folded {
        let (record, seq) = split_frn(frn);
        Folded::Resolve { record, seq, parent_record: split_frn(parent).0, is_dir, from_path: false }
    }

    fn remove(frn: u64) -> Folded {
        let (record, seq) = split_frn(frn);
        Folded::Remove { record, seq }
    }

    #[test]
    fn a_create_resolves_with_its_parent() {
        let records = [rec(FILE_ID_A, PARENT, CREATE, FILE), rec(FILE_ID_A, PARENT, CREATE | DATA | CLOSE, FILE)];
        assert_eq!(fold(&records), vec![resolve(FILE_ID_A, PARENT, false)]);
    }

    #[test]
    fn a_create_and_a_delete_in_one_batch_give_nothing() {
        let records = [rec(FILE_ID_A, PARENT, CREATE, FILE), rec(FILE_ID_A, PARENT, DELETE | CLOSE, FILE)];
        assert_eq!(fold(&records), vec![]);
        // Create and delete in one record (a short-lived temp file).
        let one = [rec(FILE_ID_A, PARENT, CREATE | DELETE | CLOSE, FILE)];
        assert_eq!(fold(&one), vec![]);
        // Create, rename, delete: still nothing.
        let renamed = [
            rec(FILE_ID_A, PARENT, CREATE, FILE),
            rec(FILE_ID_A, PARENT, OLD_NAME, FILE),
            rec(FILE_ID_A, FILE_ID_B, NEW_NAME, FILE),
            rec(FILE_ID_A, FILE_ID_B, DELETE, FILE),
        ];
        assert_eq!(fold(&renamed), vec![]);
    }

    #[test]
    fn the_changes_variant_still_removes_a_created_and_deleted_file() {
        let records = [rec(FILE_ID_A, PARENT, CREATE, FILE), rec(FILE_ID_A, PARENT, DELETE | CLOSE, FILE)];
        assert_eq!(fold_with(&records, true), vec![remove(FILE_ID_A)]);
    }

    #[test]
    fn a_delete_of_a_known_file_removes_it() {
        let records = [rec(FILE_ID_A, PARENT, DELETE | CLOSE, FILE)];
        assert_eq!(fold(&records), vec![remove(FILE_ID_A)]);
    }

    #[test]
    fn a_delete_wins_over_an_earlier_rename() {
        let records = [
            rec(FILE_ID_A, PARENT, OLD_NAME, FILE),
            rec(FILE_ID_A, FILE_ID_B, NEW_NAME, FILE),
            rec(FILE_ID_A, FILE_ID_B, DELETE | CLOSE, FILE),
        ];
        assert_eq!(fold(&records), vec![remove(FILE_ID_A)]);
    }

    #[test]
    fn a_rename_resolves_with_the_new_parent() {
        let records = [
            rec(FILE_ID_A, PARENT, OLD_NAME, DIR),
            rec(FILE_ID_A, FILE_ID_B, NEW_NAME, DIR),
            rec(FILE_ID_A, FILE_ID_B, NEW_NAME | CLOSE, DIR),
        ];
        assert_eq!(fold(&records), vec![resolve(FILE_ID_A, FILE_ID_B, true)]);
    }

    const LINK: u32 = USN_REASON_HARD_LINK_CHANGE;

    #[test]
    fn a_hard_link_change_resolves_with_the_parent_from_the_path() {
        let folded = fold(&[rec(FILE_ID_A, PARENT, LINK | CLOSE, FILE)]);
        let (record, seq) = split_frn(FILE_ID_A);
        assert_eq!(
            folded,
            vec![Folded::Resolve {
                record,
                seq,
                parent_record: split_frn(PARENT).0,
                is_dir: false,
                from_path: true
            }]
        );
        // A delete after the link change still removes the file.
        let deleted = [rec(FILE_ID_A, PARENT, LINK, FILE), rec(FILE_ID_A, PARENT, DELETE | CLOSE, FILE)];
        assert_eq!(fold_with(&deleted, true), vec![remove(FILE_ID_A)]);
        // The read asks the kernel for this reason.
        assert_ne!(REASON_MASK & USN_REASON_HARD_LINK_CHANGE, 0);
    }

    #[test]
    fn an_old_name_alone_gives_nothing() {
        assert_eq!(fold(&[rec(FILE_ID_A, PARENT, OLD_NAME, FILE)]), vec![]);
        // Data writes and close records give nothing either.
        assert_eq!(fold(&[rec(FILE_ID_A, PARENT, DATA | CLOSE, FILE), rec(FILE_ID_B, PARENT, CLOSE, FILE)]), vec![]);
    }

    #[test]
    fn the_result_is_in_the_order_of_first_appearance() {
        // The folder is renamed after its child is created: the folder stays first.
        let records = [
            rec(FILE_ID_A, PARENT, CREATE, DIR),
            rec(FILE_ID_B, FILE_ID_A, CREATE, FILE),
            rec(FILE_ID_A, PARENT, OLD_NAME, DIR),
            rec(FILE_ID_A, PARENT, NEW_NAME, DIR),
        ];
        assert_eq!(
            fold(&records),
            vec![resolve(FILE_ID_A, PARENT, true), resolve(FILE_ID_B, FILE_ID_A, false)]
        );
    }

    #[test]
    fn a_reused_record_with_a_new_sequence_is_a_different_file() {
        let old = 0x0001_0000_0000_0064u64;
        let new = 0x0002_0000_0000_0064u64;
        let records = [rec(old, PARENT, DELETE, FILE), rec(new, PARENT, CREATE, FILE)];
        assert_eq!(fold(&records), vec![remove(old), resolve(new, PARENT, false)]);
        assert_eq!(split_frn(old).0, split_frn(new).0);
    }

    // --- continuity ---

    fn journal() -> JournalMetadata {
        JournalMetadata { journal_id: 7, first_usn: 100, next_usn: 500, lowest_valid_usn: 120 }
    }

    fn checkpoint(journal_id: u64, next_usn: i64) -> Option<JournalCheckpoint> {
        Some(JournalCheckpoint { journal_id, next_usn })
    }

    #[test]
    fn continuity_has_every_outcome() {
        use JournalContinuity::*;
        use RebuildReason::*;
        assert_eq!(journal_continuity(None, journal()), Rebuild(MissingCheckpoint));
        assert_eq!(journal_continuity(checkpoint(8, 300), journal()), Rebuild(JournalIdChanged));
        // Behind the oldest valid record (the larger of FirstUsn and LowestValidUsn).
        assert_eq!(journal_continuity(checkpoint(7, 119), journal()), Rebuild(CursorTruncated));
        assert_eq!(journal_continuity(checkpoint(7, 501), journal()), Rebuild(CursorAhead));
        assert_eq!(journal_continuity(checkpoint(7, 500), journal()), Current);
        assert_eq!(journal_continuity(checkpoint(7, 120), journal()), CatchUp);
        assert_eq!(journal_continuity(checkpoint(7, 300), journal()), CatchUp);
    }
}
