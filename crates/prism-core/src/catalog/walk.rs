//! The file-system walk of catalog v2. It lists every folder of a volume with
//! `GetFileInformationByHandleEx` and sends one `FsNode` per entry to a sink.
//!
//! On NTFS each entry carries its file id (FRN): `record` = low 48 bits,
//! `seq` = high 16 bits. The journal source (`journal.rs`) names entries by the
//! same numbers. A volume that is not NTFS (or has no journal) is walked with
//! synthetic record numbers: the root is 0, then 1, 2, ... and `seq` is 0.
//!
//! Rules kept from the old scanner:
//! - junctions and symlinks are leaves (no descent);
//! - cloud placeholder folders (OneDrive and so on) are descended;
//! - the walk runs at below-normal priority with power throttling.
//!
//! Nodes reach the sink in any order. A parent folder is always sent before
//! its children are listed, but not always before they reach the sink when
//! more than one thread runs.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileFullDirectoryInfo, FileIdExtdDirectoryInfo, FileIdInfo,
    GetFileInformationByHandleEx, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::SystemServices::{IO_REPARSE_TAG_CLOUD, IO_REPARSE_TAG_CLOUD_MASK};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, SetThreadPriority, ThreadPowerThrottling,
    THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_NORMAL,
};

use super::store::{split_frn, FsNode, RECORD_MASK};

// ---------------------------------------------------------------------------
// Exclusions
// ---------------------------------------------------------------------------

/// The folders that the index keeps as one excluded entry and does not descend.
///
/// A list item with no `\` and no `:` is a folder name: it matches a folder of
/// that name anywhere. Any other item is an absolute path: it matches exactly
/// that folder. Compare is in lowercase. `<home>` in an item stands for
/// `USERPROFILE`.
#[derive(Clone, Debug, Default)]
pub struct Exclusions {
    names: HashSet<String>,
    paths: HashSet<String>,
}

impl Exclusions {
    /// Builds the matcher from a list. `<home>` comes from `USERPROFILE`.
    pub fn new<S: AsRef<str>>(entries: &[S]) -> Self {
        let home = std::env::var("USERPROFILE").ok();
        Self::with_home(entries, home.as_deref())
    }

    /// Like `new`, with the home folder given. An item that uses `<home>` is
    /// dropped when `home` is `None`.
    pub fn with_home<S: AsRef<str>>(entries: &[S], home: Option<&str>) -> Self {
        let home = home.map(normalize_item).filter(|home| !home.is_empty());
        let mut result = Self::default();
        for entry in entries {
            let entry = entry.as_ref();
            let lowered = entry.to_lowercase();
            let item = if lowered.contains("<home>") {
                let Some(home) = &home else { continue };
                normalize_item(&lowered.replace("<home>", home))
            } else {
                normalize_item(&lowered)
            };
            if item.is_empty() {
                continue;
            }
            if item.contains('\\') || item.contains(':') {
                result.paths.insert(item);
            } else {
                result.names.insert(item);
            }
        }
        result
    }

    /// The default list of the catalog v2 spec, as text items.
    pub fn default_entries(app_data_dir: &Path) -> Vec<String> {
        let mut list: Vec<String> = [
            "$recycle.bin",
            "system volume information",
            "node_modules",
            ".git",
            "__pycache__",
            ".gradle",
            r"<home>\.cargo\registry",
            r"<home>\.rustup\toolchains",
            r"<home>\.nuget\packages",
            r"<home>\appdata\local\temp",
            r"c:\windows\winsxs",
            r"c:\windows\installer",
            r"c:\programdata\microsoft\windows\containers",
        ]
        .iter()
        .map(|item| item.to_string())
        .collect();
        list.push(app_data_dir.to_string_lossy().into_owned());
        list
    }

    /// The default list of the catalog v2 spec.
    pub fn default_list(app_data_dir: &Path) -> Self {
        Self::new(&Self::default_entries(app_data_dir))
    }

    /// True when the folder is on the list. `lower_path` is the full path of
    /// the folder in lowercase, with no trailing separator.
    pub fn is_excluded(&self, lower_path: &str, lower_name: &str) -> bool {
        self.names.contains(lower_name) || self.paths.contains(lower_path)
    }
}

/// Lowercase item: `/` becomes `\`, no `\\?\` prefix, no trailing separator.
fn normalize_item(text: &str) -> String {
    let text = text.trim().to_lowercase().replace('/', "\\");
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.trim_end_matches('\\').to_string()
}

// ---------------------------------------------------------------------------
// Walk
// ---------------------------------------------------------------------------

/// What one walk did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WalkStats {
    /// Nodes sent to the sink.
    pub entries: u64,
    /// Folders listed.
    pub folders: u64,
    /// Folders skipped or not listed to the end: access denied, or an
    /// enumeration that stopped early (their listed part is still sent).
    pub denied: u64,
    pub ms: u64,
}

/// Threads of `walk_volume`. A few threads over a work stack of folders.
pub const WALK_THREADS: usize = 3;

/// Nodes a worker collects before it sends them to the sink thread.
const SEND_BATCH: usize = 2048;

/// Bytes of the listing buffer of one worker: one read returns about 500 entries.
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// How the walk numbers the entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdMode {
    /// The file id of the file system (NTFS).
    Frn,
    /// Counter numbers: root 0, then 1, 2, ...
    Synthetic,
}

/// Walks an NTFS volume (or any folder of it). Every entry carries its FRN.
/// `sink` is called on the calling thread. Returns an error if the volume
/// cannot give file ids, or when `cancel` is set.
pub fn walk_volume(
    root: &Path,
    exclusions: &Exclusions,
    cancel: &AtomicBool,
    sink: &mut dyn FnMut(FsNode),
) -> Result<WalkStats, String> {
    walk_inner(root, exclusions, cancel, WALK_THREADS, IdMode::Frn, sink)
}

/// Walks a volume that is not NTFS (or has no journal). The entries get
/// synthetic record numbers: the root is 0, then 1, 2, ... and `seq` is 0.
pub fn walk_volume_synthetic(
    root: &Path,
    exclusions: &Exclusions,
    cancel: &AtomicBool,
    sink: &mut dyn FnMut(FsNode),
) -> Result<WalkStats, String> {
    walk_inner(root, exclusions, cancel, WALK_THREADS, IdMode::Synthetic, sink)
}

/// `walk_volume` or `walk_volume_synthetic` with a chosen thread count.
pub fn walk_volume_threads(
    root: &Path,
    exclusions: &Exclusions,
    cancel: &AtomicBool,
    threads: usize,
    synthetic: bool,
    sink: &mut dyn FnMut(FsNode),
) -> Result<WalkStats, String> {
    let mode = if synthetic { IdMode::Synthetic } else { IdMode::Frn };
    walk_inner(root, exclusions, cancel, threads, mode, sink)
}

/// The record number of a folder (for a volume root on NTFS: 5).
pub fn root_record(root: &Path) -> Result<u64, String> {
    let wide = to_wide(&win32_long_path(&display_root(root)));
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
    .map_err(|error| format!("open {}: {error}", root.display()))?;
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `info` is writable for its whole size.
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    // SAFETY: `handle` was opened above and is closed once.
    let _ = unsafe { CloseHandle(handle) };
    result.map_err(|error| format!("read the file id of {}: {error}", root.display()))?;
    let id = u64::from_le_bytes(info.FileId.Identifier[..8].try_into().unwrap());
    Ok(id & RECORD_MASK)
}

/// A folder to list: its display path (no trailing separator) and its record.
struct WorkItem {
    path: String,
    record: u64,
}

struct Queue {
    items: Vec<WorkItem>,
    /// Folders that a worker has taken and not finished.
    active: usize,
}

struct Ctx<'a> {
    exclusions: &'a Exclusions,
    cancel: &'a AtomicBool,
    mode: IdMode,
    queue: Mutex<Queue>,
    wake: Condvar,
    /// Set on cancel, on a fatal error, or when the sink thread is gone.
    stop: AtomicBool,
    fatal: Mutex<Option<String>>,
    /// Next synthetic record number.
    next_synthetic: AtomicU64,
    folders: AtomicU64,
    denied: AtomicU64,
}

impl Ctx<'_> {
    fn halt(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.notify_all();
    }

    fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.cancel.load(Ordering::Relaxed)
    }
}

fn walk_inner(
    root: &Path,
    exclusions: &Exclusions,
    cancel: &AtomicBool,
    threads: usize,
    mode: IdMode,
    sink: &mut dyn FnMut(FsNode),
) -> Result<WalkStats, String> {
    let started = Instant::now();
    let path = display_root(root);
    let record = match mode {
        IdMode::Frn => root_record(root)?,
        IdMode::Synthetic => 0,
    };
    let ctx = Ctx {
        exclusions,
        cancel,
        mode,
        queue: Mutex::new(Queue { items: vec![WorkItem { path, record }], active: 0 }),
        wake: Condvar::new(),
        stop: AtomicBool::new(false),
        fatal: Mutex::new(None),
        next_synthetic: AtomicU64::new(1),
        folders: AtomicU64::new(0),
        denied: AtomicU64::new(0),
    };

    let entries = std::thread::scope(|scope| {
        let (sender, receiver) = sync_channel::<Vec<FsNode>>(32);
        for _ in 0..threads.max(1) {
            let sender = sender.clone();
            let ctx = &ctx;
            scope.spawn(move || worker(ctx, sender));
        }
        drop(sender);
        let mut entries = 0u64;
        for batch in receiver {
            entries += batch.len() as u64;
            for node in batch {
                sink(node);
            }
        }
        entries
    });

    if cancel.load(Ordering::Relaxed) {
        return Err("walk cancelled".to_string());
    }
    if let Some(message) = ctx.fatal.lock().unwrap().take() {
        return Err(message);
    }
    Ok(WalkStats {
        entries,
        folders: ctx.folders.load(Ordering::Relaxed),
        denied: ctx.denied.load(Ordering::Relaxed),
        ms: started.elapsed().as_millis() as u64,
    })
}

/// One walk thread: take a folder, list it, push its subfolders, send its nodes.
fn worker(ctx: &Ctx<'_>, sender: SyncSender<Vec<FsNode>>) {
    let _priority = BackgroundGuard::new();
    let mut buffer = vec![0u64; READ_BUFFER_BYTES / 8];
    let mut pending: Vec<FsNode> = Vec::new();
    loop {
        let item = {
            let mut queue = ctx.queue.lock().unwrap();
            loop {
                if ctx.should_stop() {
                    ctx.wake.notify_all();
                    drop(queue);
                    flush(&sender, &mut pending, ctx);
                    return;
                }
                if let Some(item) = queue.items.pop() {
                    queue.active += 1;
                    break item;
                }
                if queue.active == 0 {
                    ctx.wake.notify_all();
                    drop(queue);
                    flush(&sender, &mut pending, ctx);
                    return;
                }
                // Nothing to take now: send what this worker holds, then wait.
                drop(queue);
                flush(&sender, &mut pending, ctx);
                queue = ctx.queue.lock().unwrap();
                if queue.items.is_empty() && queue.active > 0 && !ctx.should_stop() {
                    queue = ctx
                        .wake
                        .wait_timeout(queue, Duration::from_millis(200))
                        .unwrap()
                        .0;
                }
            }
        };

        let children = process_folder(ctx, &item, &mut buffer, &mut pending);

        {
            let mut queue = ctx.queue.lock().unwrap();
            queue.items.extend(children);
            queue.active -= 1;
        }
        ctx.wake.notify_all();
        if pending.len() >= SEND_BATCH {
            flush(&sender, &mut pending, ctx);
        }
    }
}

fn flush(sender: &SyncSender<Vec<FsNode>>, pending: &mut Vec<FsNode>, ctx: &Ctx<'_>) {
    if pending.is_empty() {
        return;
    }
    if sender.send(std::mem::take(pending)).is_err() {
        // The sink thread is gone.
        ctx.halt();
    }
}

/// Lists one folder. Pushes its nodes to `pending` and returns its subfolders.
fn process_folder(
    ctx: &Ctx<'_>,
    item: &WorkItem,
    buffer: &mut [u64],
    pending: &mut Vec<FsNode>,
) -> Vec<WorkItem> {
    let listing = match list_folder(&item.path, ctx.mode, buffer) {
        FolderRead::Listed(listing) => listing,
        // Removed since its parent was listed.
        FolderRead::Gone => return Vec::new(),
        FolderRead::Denied => {
            ctx.denied.fetch_add(1, Ordering::Relaxed);
            return Vec::new();
        }
        FolderRead::Unsupported => {
            let mut fatal = ctx.fatal.lock().unwrap();
            if fatal.is_none() {
                *fatal = Some(format!(
                    "{}: the volume does not give file ids; use the synthetic walk",
                    item.path
                ));
            }
            drop(fatal);
            ctx.halt();
            return Vec::new();
        }
    };
    ctx.folders.fetch_add(1, Ordering::Relaxed);
    if !listing.complete {
        ctx.denied.fetch_add(1, Ordering::Relaxed);
    }

    let synthetic_base = match ctx.mode {
        IdMode::Synthetic => ctx
            .next_synthetic
            .fetch_add(listing.entries.len() as u64, Ordering::Relaxed),
        IdMode::Frn => 0,
    };
    let lower_path = item.path.to_lowercase();
    let mut children = Vec::new();
    for (index, entry) in listing.entries.into_iter().enumerate() {
        let (record, seq) = match ctx.mode {
            IdMode::Frn => split_frn(entry.id),
            IdMode::Synthetic => (synthetic_base + index as u64, 0),
        };
        let is_dir = entry.attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
        let mut excluded = false;
        let mut link = false;
        if is_dir {
            let lower_name = entry.name.to_lowercase();
            let lower_child = format!("{lower_path}\\{lower_name}");
            excluded = ctx.exclusions.is_excluded(&lower_child, &lower_name);
            link = is_link_folder(entry.attributes, entry.reparse_tag);
            if !excluded && !link {
                children.push(WorkItem { path: format!("{}\\{}", item.path, entry.name), record });
            }
        }
        pending.push(FsNode {
            record,
            seq,
            parent_record: item.record,
            name: entry.name,
            is_dir,
            excluded,
            link,
        });
    }
    children
}

/// True for a junction, a symlink or a mount point folder: a reparse folder
/// that is not a cloud placeholder (see `is_cloud_reparse_tag`). A walk does
/// not enter it.
pub(super) fn is_link_folder(attributes: u32, tag: u32) -> bool {
    attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 && !is_cloud_reparse_tag(tag)
}

/// Cloud-sync placeholder roots (OneDrive and friends) surface as reparse
/// points, but they hold the user's real files: they are descended. Every
/// other reparse folder (junction, symlink, mount point) stays a leaf, so no
/// subtree is walked twice and no link loop can happen.
fn is_cloud_reparse_tag(tag: u32) -> bool {
    (tag & !IO_REPARSE_TAG_CLOUD_MASK) == IO_REPARSE_TAG_CLOUD
}

// ---------------------------------------------------------------------------
// Folder listing
// ---------------------------------------------------------------------------

/// One entry of a folder listing (without `.` and `..`).
#[derive(Debug, PartialEq, Eq)]
struct RawEntry {
    name: String,
    attributes: u32,
    /// The reparse tag when `attributes` has `FILE_ATTRIBUTE_REPARSE_POINT`.
    reparse_tag: u32,
    /// Low 64 bits of the file id. 0 when the listing has no ids.
    id: u64,
}

struct Listing {
    entries: Vec<RawEntry>,
    /// False when the listing stopped before its end.
    complete: bool,
}

enum FolderRead {
    Listed(Listing),
    /// The folder does not exist (any more).
    Gone,
    /// The folder exists and cannot be opened.
    Denied,
    /// The volume does not support the listing class that gives file ids.
    Unsupported,
}

/// Byte offsets in one directory-information record.
struct Layout {
    /// Offset of `FileName`.
    header: usize,
    /// Offset of the reparse tag.
    tag: usize,
    /// Offset of the 128-bit file id, if the class has one.
    id: Option<usize>,
}

/// `FILE_ID_EXTD_DIR_INFO`: file id (128 bit) and `ReparsePointTag`.
const LAYOUT_EXTD: Layout = Layout { header: 88, tag: 68, id: Some(72) };
/// `FILE_FULL_DIR_INFO`: no id; `EaSize` holds the reparse tag.
const LAYOUT_FULL: Layout = Layout { header: 68, tag: 64, id: None };

/// Offsets that every class shares.
const OFFSET_ATTRIBUTES: usize = 56;
const OFFSET_NAME_LENGTH: usize = 60;

/// How the parse of one buffer ended.
#[derive(Debug, PartialEq, Eq)]
enum ParseEnd {
    /// All entries of the buffer were read.
    Done,
    /// The buffer is not valid, or the listing repeats itself.
    Stopped,
}

/// Reads the entries of one filled buffer. `repeats` counts the entries that
/// equal the one before them.
///
/// NTFS enumeration can return the same entry again and again when the folder
/// changes at the same time (database folders, browser caches, temp churn). A
/// listing that repeats an entry 4 times in a row is stopped.
fn parse_buffer(
    bytes: &[u8],
    layout: &Layout,
    entries: &mut Vec<RawEntry>,
    repeats: &mut u32,
) -> ParseEnd {
    let mut offset = 0usize;
    loop {
        let Some(header_end) = offset.checked_add(layout.header) else {
            return ParseEnd::Stopped;
        };
        if header_end > bytes.len() || !offset.is_multiple_of(8) {
            return ParseEnd::Stopped;
        }
        let record = &bytes[offset..];
        let next = read_u32(record, 0) as usize;
        let attributes = read_u32(record, OFFSET_ATTRIBUTES);
        let name_bytes = read_u32(record, OFFSET_NAME_LENGTH) as usize;
        let name_end = header_end.checked_add(name_bytes);
        if !name_bytes.is_multiple_of(2) || name_end.is_none_or(|end| end > bytes.len()) {
            return ParseEnd::Stopped;
        }
        let units: Vec<u16> = bytes[header_end..header_end + name_bytes]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let name = String::from_utf16_lossy(&units);
        if name != "." && name != ".." {
            if entries.last().is_some_and(|last| last.name == name) {
                *repeats += 1;
                if *repeats >= 4 {
                    return ParseEnd::Stopped;
                }
            } else {
                *repeats = 0;
                entries.push(RawEntry {
                    name,
                    attributes,
                    reparse_tag: read_u32(record, layout.tag),
                    id: layout.id.map_or(0, |at| read_u64(record, at)),
                });
            }
        }
        if next == 0 {
            return ParseEnd::Done;
        }
        // The step is a multiple of 8, and it must stay in the buffer.
        let Some(next_offset) = offset.checked_add(next) else {
            return ParseEnd::Stopped;
        };
        if next < 8 || !next.is_multiple_of(8) || next_offset > bytes.len() {
            return ParseEnd::Stopped;
        }
        offset = next_offset;
    }
}

/// Reads at `offset`; 0 when the bytes are missing (the caller checked the header).
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    bytes
        .get(offset..offset + 4)
        .and_then(|raw| raw.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    bytes
        .get(offset..offset + 8)
        .and_then(|raw| raw.try_into().ok())
        .map_or(0, u64::from_le_bytes)
}

/// Lists `path` into entries. `buffer` is reused between folders.
fn list_folder(path: &str, mode: IdMode, buffer: &mut [u64]) -> FolderRead {
    let wide = to_wide(&win32_long_path(path));
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let handle = match handle {
        Ok(handle) => handle,
        Err(error) => {
            let code = error.code();
            return if code == ERROR_FILE_NOT_FOUND.to_hresult()
                || code == ERROR_PATH_NOT_FOUND.to_hresult()
            {
                FolderRead::Gone
            } else {
                FolderRead::Denied
            };
        }
    };
    let (class, layout) = match mode {
        IdMode::Frn => (FileIdExtdDirectoryInfo, &LAYOUT_EXTD),
        IdMode::Synthetic => (FileFullDirectoryInfo, &LAYOUT_FULL),
    };
    let mut listing = Listing { entries: Vec::new(), complete: true };
    let mut repeats = 0u32;
    let mut first_read = true;
    let mut unsupported = false;
    let buffer_bytes = buffer.len() * 8;
    loop {
        // SAFETY: `buffer` is writable for its whole length and 8-byte aligned.
        let read = unsafe {
            GetFileInformationByHandleEx(
                handle,
                class,
                buffer.as_mut_ptr().cast(),
                buffer_bytes as u32,
            )
        };
        if let Err(error) = read {
            let code = error.code();
            if code != ERROR_NO_MORE_FILES.to_hresult() {
                // 1 = invalid function, 50 = not supported, 87 = invalid parameter.
                let win32 = code.0 as u32 & 0xffff;
                if first_read && matches!(win32, 1 | 50 | 87) {
                    unsupported = true;
                } else {
                    listing.complete = false;
                }
            }
            break;
        }
        first_read = false;
        // SAFETY: `buffer` was filled by the call above; the slice covers it.
        let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer_bytes) };
        if parse_buffer(bytes, layout, &mut listing.entries, &mut repeats) == ParseEnd::Stopped {
            listing.complete = false;
            break;
        }
    }
    // SAFETY: `handle` was opened above and is closed once.
    let _ = unsafe { CloseHandle(handle) };
    if unsupported {
        FolderRead::Unsupported
    } else {
        FolderRead::Listed(listing)
    }
}

// ---------------------------------------------------------------------------
// Paths and thread priority
// ---------------------------------------------------------------------------

/// The root as the walk writes it: no `\\?\` prefix, no trailing separator.
/// The walk builds child paths as `<this>\<name>`.
fn display_root(root: &Path) -> String {
    let text = root.to_string_lossy();
    let text = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text.into_owned()
    };
    text.trim_end_matches(['\\', '/']).to_string()
}

/// `dir` as a `\\?\` path that ends with a separator (a bare `\\?\C:` would
/// name the volume, not its root folder).
fn win32_long_path(dir: &str) -> String {
    let trimmed = dir.trim_end_matches(['\\', '/']);
    if trimmed.starts_with(r"\\?\") {
        format!("{trimmed}\\")
    } else if let Some(stripped) = trimmed.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{stripped}\")
    } else {
        format!(r"\\?\{trimmed}\")
    }
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// File enumeration is background work. For its duration a thread runs at
/// below-normal priority and under Windows power throttling (EcoQoS): the
/// scheduler then runs the walk at efficiency speed. Priority alone only
/// yields CPU time; it never lowers the power cost of the time it gets.
struct BackgroundGuard;

impl BackgroundGuard {
    fn new() -> Self {
        Self::apply(THREAD_PRIORITY_BELOW_NORMAL, THREAD_POWER_THROTTLING_EXECUTION_SPEED);
        Self
    }

    fn apply(priority: windows::Win32::System::Threading::THREAD_PRIORITY, state_mask: u32) {
        let state = THREAD_POWER_THROTTLING_STATE {
            Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: state_mask,
        };
        // SAFETY: the pseudo handle of the current thread is always valid, and
        // `state` is a plain struct that outlives the call.
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), priority);
            let _ = SetThreadInformation(
                GetCurrentThread(),
                ThreadPowerThrottling,
                (&state as *const THREAD_POWER_THROTTLING_STATE).cast(),
                std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
            );
        }
    }
}

impl Drop for BackgroundGuard {
    fn drop(&mut self) {
        Self::apply(THREAD_PRIORITY_NORMAL, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::super::store::make_frn;
    use super::*;

    fn lowered(entries: &[&str], home: Option<&str>) -> Exclusions {
        Exclusions::with_home(entries, home)
    }

    #[test]
    fn frn_splits_into_record_and_sequence() {
        let frn = 0x0003_0000_0000_1234u64;
        assert_eq!(split_frn(frn), (0x1234, 3));
        assert_eq!(make_frn(0x1234, 3), frn);
        assert_eq!(split_frn(make_frn(RECORD_MASK, u16::MAX)), (RECORD_MASK, u16::MAX));
    }

    #[test]
    fn a_name_matches_anywhere_in_lowercase() {
        let list = lowered(&["Node_Modules", ".GIT"], None);
        assert!(list.is_excluded(r"c:\a\b\node_modules", "node_modules"));
        assert!(list.is_excluded(r"d:\x\.git", ".git"));
        assert!(!list.is_excluded(r"c:\a\node_modules2", "node_modules2"));
    }

    #[test]
    fn a_path_matches_exactly_that_folder() {
        let list = lowered(&[r"C:\Windows\WinSxS", "d:/data/cache/"], None);
        assert!(list.is_excluded(r"c:\windows\winsxs", "winsxs"));
        assert!(list.is_excluded(r"d:\data\cache", "cache"));
        // A folder of the same name elsewhere, and a child of the folder, do not match.
        assert!(!list.is_excluded(r"c:\other\winsxs", "winsxs"));
        assert!(!list.is_excluded(r"c:\windows\winsxs\amd64", "amd64"));
    }

    #[test]
    fn home_expands_and_is_dropped_without_a_home() {
        let list = lowered(&[r"<home>\.Cargo\registry"], Some(r"C:\Users\Hi\"));
        assert!(list.is_excluded(r"c:\users\hi\.cargo\registry", "registry"));
        assert!(!list.is_excluded(r"c:\users\you\.cargo\registry", "registry"));
        let none = lowered(&[r"<home>\.cargo\registry"], None);
        assert!(!none.is_excluded(r"c:\users\hi\.cargo\registry", "registry"));
    }

    #[test]
    fn an_extended_prefix_and_empty_items_are_ignored() {
        let list = lowered(&[r"\\?\C:\Data\Skip", "  ", ""], None);
        assert!(list.is_excluded(r"c:\data\skip", "skip"));
        assert!(!list.is_excluded("", ""));
    }

    #[test]
    fn the_default_list_has_the_spec_items() {
        let list = Exclusions::default_list(Path::new(r"C:\Users\Hi\AppData\Roaming\app.prism.launcher"));
        for name in ["$recycle.bin", "system volume information", "node_modules", ".git", "__pycache__", ".gradle"] {
            assert!(list.is_excluded("x", name), "{name}");
        }
        assert!(list.is_excluded(r"c:\windows\winsxs", "winsxs"));
        assert!(list.is_excluded(r"c:\windows\installer", "installer"));
        assert!(list.is_excluded(r"c:\programdata\microsoft\windows\containers", "containers"));
        assert!(list.is_excluded(r"c:\users\hi\appdata\roaming\app.prism.launcher", "app.prism.launcher"));
    }

    #[test]
    fn the_root_is_written_without_prefix_or_trailing_separator() {
        assert_eq!(display_root(Path::new(r"C:\")), "C:");
        assert_eq!(display_root(Path::new(r"\\?\C:\Users\Hi\")), r"C:\Users\Hi");
        assert_eq!(display_root(Path::new(r"\\?\UNC\srv\share")), r"\\srv\share");
        assert_eq!(win32_long_path("C:"), r"\\?\C:\");
        assert_eq!(win32_long_path(r"\\srv\share"), r"\\?\UNC\srv\share\");
    }

    #[test]
    fn only_reparse_folders_that_are_not_cloud_placeholders_are_links() {
        let reparse = FILE_ATTRIBUTE_REPARSE_POINT.0;
        assert!(is_link_folder(reparse | FILE_ATTRIBUTE_DIRECTORY.0, 0xA000_0003)); // mount point
        assert!(is_link_folder(reparse, 0xA000_000C)); // symlink
        assert!(!is_link_folder(reparse, IO_REPARSE_TAG_CLOUD | 0x3000));
        assert!(!is_link_folder(FILE_ATTRIBUTE_DIRECTORY.0, 0));
    }

    /// One record of the listing: header (zeros), then the name.
    fn record(layout: &Layout, name: &str, attributes: u32, tag: u32, id: u64, last: bool) -> Vec<u8> {
        let units: Vec<u8> = name.encode_utf16().flat_map(|unit| unit.to_le_bytes()).collect();
        let mut bytes = vec![0u8; layout.header + units.len()];
        bytes[OFFSET_ATTRIBUTES..OFFSET_ATTRIBUTES + 4].copy_from_slice(&attributes.to_le_bytes());
        bytes[OFFSET_NAME_LENGTH..OFFSET_NAME_LENGTH + 4]
            .copy_from_slice(&(units.len() as u32).to_le_bytes());
        bytes[layout.tag..layout.tag + 4].copy_from_slice(&tag.to_le_bytes());
        if let Some(at) = layout.id {
            bytes[at..at + 8].copy_from_slice(&id.to_le_bytes());
        }
        bytes[layout.header..].copy_from_slice(&units);
        while !bytes.len().is_multiple_of(8) {
            bytes.push(0);
        }
        if !last {
            let step = bytes.len() as u32;
            bytes[0..4].copy_from_slice(&step.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_listing_buffer_is_parsed_with_ids_and_tags() {
        let dir = FILE_ATTRIBUTE_DIRECTORY.0;
        let mut buffer = record(&LAYOUT_EXTD, ".", dir, 0, 5, false);
        buffer.extend(record(&LAYOUT_EXTD, "Docs", dir, 0, make_frn(40, 2), false));
        buffer.extend(record(&LAYOUT_EXTD, "link", dir | FILE_ATTRIBUTE_REPARSE_POINT.0, 0xA000_0003, 77, false));
        buffer.extend(record(&LAYOUT_EXTD, "a.txt", 0x20, 0, 91, true));
        let mut entries = Vec::new();
        let mut repeats = 0;
        assert_eq!(parse_buffer(&buffer, &LAYOUT_EXTD, &mut entries, &mut repeats), ParseEnd::Done);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["Docs", "link", "a.txt"]);
        assert_eq!(entries[0].id, make_frn(40, 2));
        assert_eq!(entries[1].reparse_tag, 0xA000_0003);
        assert_eq!(entries[2].attributes, 0x20);
    }

    #[test]
    fn a_full_directory_listing_has_no_ids() {
        let buffer = record(&LAYOUT_FULL, "x", 0x10, 0, 0, true);
        let mut entries = Vec::new();
        let mut repeats = 0;
        assert_eq!(parse_buffer(&buffer, &LAYOUT_FULL, &mut entries, &mut repeats), ParseEnd::Done);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, 0);
    }

    #[test]
    fn a_name_that_leaves_the_buffer_stops_the_parse() {
        let mut buffer = record(&LAYOUT_EXTD, "long-name.txt", 0x20, 0, 9, true);
        buffer.truncate(LAYOUT_EXTD.header + 4);
        let mut entries = Vec::new();
        let mut repeats = 0;
        assert_eq!(parse_buffer(&buffer, &LAYOUT_EXTD, &mut entries, &mut repeats), ParseEnd::Stopped);
        assert!(entries.is_empty());
        // A buffer shorter than a header stops too.
        assert_eq!(parse_buffer(&[0u8; 16], &LAYOUT_EXTD, &mut entries, &mut repeats), ParseEnd::Stopped);
    }

    #[test]
    fn a_listing_that_repeats_one_entry_is_stopped() {
        let mut buffer = Vec::new();
        for _ in 0..5 {
            buffer.extend(record(&LAYOUT_EXTD, "same", 0x20, 0, 9, false));
        }
        buffer.extend(record(&LAYOUT_EXTD, "end", 0x20, 0, 10, true));
        let mut entries = Vec::new();
        let mut repeats = 0;
        assert_eq!(parse_buffer(&buffer, &LAYOUT_EXTD, &mut entries, &mut repeats), ParseEnd::Stopped);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn a_step_that_is_not_aligned_stops_the_parse() {
        let mut buffer = record(&LAYOUT_EXTD, "a", 0x20, 0, 9, false);
        buffer[0..4].copy_from_slice(&13u32.to_le_bytes());
        buffer.extend(vec![0u8; 64]);
        let mut entries = Vec::new();
        let mut repeats = 0;
        assert_eq!(parse_buffer(&buffer, &LAYOUT_EXTD, &mut entries, &mut repeats), ParseEnd::Stopped);
    }
}
