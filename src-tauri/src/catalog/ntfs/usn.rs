use std::collections::HashMap;
use std::path::Path;

use windows::Win32::Foundation::{
    ERROR_JOURNAL_DELETE_IN_PROGRESS, ERROR_JOURNAL_ENTRY_DELETED, ERROR_JOURNAL_NOT_ACTIVE,
};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
    USN_REASON_CLOSE, USN_REASON_FILE_DELETE, USN_REASON_RENAME_OLD_NAME,
};

use crate::catalog::types::{JournalCheckpoint, JournalMetadata, NtfsChange, NtfsNode};

use super::volume::{win32_error_code, NtfsVolume};

const JOURNAL_BUFFER_SIZE: usize = 1024 * 1024;
const USN_V2_MIN_LENGTH: usize = 60;
/// File reference number of an NTFS volume's root directory.
const NTFS_ROOT_FRN: u64 = 5;

/// NTFS metadata files ($Mft, $LogFile, $Volume, $Extend, ...) live directly
/// under the volume root and all start with '$'; the recycle bin keeps hashed
/// $R/$I names and System Volume Information holds restore blobs. None are
/// findable by filename, so the MFT catalog skips them and their subtrees -
/// matching the directory scanner's exclusion rules (see `scanner::is_excluded_dir`).
pub(super) fn is_root_system_node(name: &str, parent_frn: u64) -> bool {
    parent_frn == NTFS_ROOT_FRN
        && (name.starts_with('$') || name.eq_ignore_ascii_case("system volume information"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalContinuity {
    Current,
    CatchUp,
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

#[derive(Clone, Debug)]
pub(crate) struct JournalReadBatch {
    pub next_usn: i64,
    pub changes: Vec<NtfsChange>,
    pub record_count: u64,
}

pub(super) fn query_journal(volume: &NtfsVolume) -> Result<JournalMetadata, String> {
    let mut output = vec![0u8; std::mem::size_of::<USN_JOURNAL_DATA_V0>()];
    let returned = volume
        .ioctl::<u8>(FSCTL_QUERY_USN_JOURNAL, None, &mut output)
        .map_err(|error| format!("query NTFS USN journal: {error}"))? as usize;
    if returned < 32 || returned > output.len() {
        return Err("FSCTL_QUERY_USN_JOURNAL returned an invalid buffer length".to_string());
    }
    Ok(JournalMetadata {
        journal_id: read_u64(&output[..returned], 0)
            .ok_or_else(|| "journal response omitted its ID".to_string())?,
        first_usn: read_i64(&output[..returned], 8)
            .ok_or_else(|| "journal response omitted FirstUsn".to_string())?,
        next_usn: read_i64(&output[..returned], 16)
            .ok_or_else(|| "journal response omitted NextUsn".to_string())?,
        lowest_valid_usn: read_i64(&output[..returned], 24)
            .ok_or_else(|| "journal response omitted LowestValidUsn".to_string())?,
    })
}

pub(super) fn read_journal(
    volume: &NtfsVolume,
    start_usn: i64,
    journal_id: u64,
    _target_usn: i64,
) -> Result<JournalReadBatch, String> {
    let input = READ_USN_JOURNAL_DATA_V0 {
        StartUsn: start_usn,
        // Close-only records are the bulk of journal traffic (one per handle
        // close after any write) and are discarded by fold_records anyway.
        // Masking them out lets the kernel skip delivering them, which keeps
        // catch-up reads small while the disk is busy. Records that combine
        // CLOSE with a meaningful reason still match through the other bit.
        ReasonMask: !USN_REASON_CLOSE,
        ReturnOnlyOnClose: 0,
        Timeout: 0,
        BytesToWaitFor: 0,
        UsnJournalID: journal_id,
    };
    let mut output = vec![0u8; JOURNAL_BUFFER_SIZE];
    let returned = volume
        .ioctl(FSCTL_READ_USN_JOURNAL, Some(&input), &mut output)
        .map_err(|error| {
            let code = win32_error_code(&error);
            if matches!(
                code,
                value if value == ERROR_JOURNAL_ENTRY_DELETED.0
                    || value == ERROR_JOURNAL_NOT_ACTIVE.0
                    || value == ERROR_JOURNAL_DELETE_IN_PROGRESS.0
            ) {
                format!("USN journal history is no longer continuous: {error}")
            } else {
                format!("read NTFS USN journal: {error}")
            }
        })? as usize;
    if returned < 8 || returned > output.len() {
        return Err("FSCTL_READ_USN_JOURNAL returned an invalid buffer length".to_string());
    }
    let next_usn = read_i64(&output[..returned], 0)
        .ok_or_else(|| "journal response did not contain a cursor".to_string())?;
    let mut records = Vec::new();
    let mut offset = 8usize;
    while offset < returned {
        let (record, length) = parse_record(&output[offset..returned])?;
        match record {
            ParsedUsnRecord::V2(record) => records.push(record),
            ParsedUsnRecord::Unsupported { major_version } => {
                return Err(format!(
                    "unsupported USN record major version {major_version} in journal"
                ));
            }
        }
        offset = offset
            .checked_add(length)
            .ok_or_else(|| "journal record offset overflow".to_string())?;
    }
    if offset != returned {
        return Err("journal response ended between records".to_string());
    }
    let record_count = records.len() as u64;
    Ok(JournalReadBatch {
        next_usn,
        changes: fold_records(records),
        record_count,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UsnRecordV2 {
    pub frn: u64,
    pub parent_frn: u64,
    pub usn: i64,
    pub timestamp: i64,
    pub reason: u32,
    pub attributes: u32,
    pub name: String,
}

impl UsnRecordV2 {
    pub(super) fn into_node(self) -> NtfsNode {
        let is_directory = self.attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
        let extension = if is_directory {
            None
        } else {
            Path::new(&self.name)
                .extension()
                .map(|value| value.to_string_lossy().to_lowercase())
        };
        NtfsNode {
            frn: self.frn,
            parent_frn: self.parent_frn,
            lower_name: self.name.to_lowercase(),
            name: self.name,
            extension,
            is_directory,
            attributes: self.attributes,
            modified_at: self.timestamp,
            // USN records intentionally do not include allocation/file size.
            // This remains zero until a future lazy enrichment path needs it.
            size: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ParsedUsnRecord {
    V2(UsnRecordV2),
    Unsupported { major_version: u16 },
}

pub(super) fn parse_record(bytes: &[u8]) -> Result<(ParsedUsnRecord, usize), String> {
    let record_length = read_u32(bytes, 0)
        .ok_or_else(|| "USN record is shorter than its common header".to_string())?
        as usize;
    let major_version = read_u16(bytes, 4)
        .ok_or_else(|| "USN record is shorter than its version header".to_string())?;
    let _minor_version = read_u16(bytes, 6)
        .ok_or_else(|| "USN record is shorter than its version header".to_string())?;
    if record_length < 8 || record_length > bytes.len() {
        return Err("USN record length is outside the returned buffer".to_string());
    }
    if major_version != 2 {
        return Ok((
            ParsedUsnRecord::Unsupported { major_version },
            record_length,
        ));
    }
    if record_length < USN_V2_MIN_LENGTH {
        return Err("USN v2 record is shorter than the fixed header".to_string());
    }

    let name_length = read_u16(bytes, 56).unwrap() as usize;
    let name_offset = read_u16(bytes, 58).unwrap() as usize;
    if !name_length.is_multiple_of(2)
        || !name_offset.is_multiple_of(2)
        || name_offset < USN_V2_MIN_LENGTH
    {
        return Err("USN v2 record contains an invalid UTF-16 filename range".to_string());
    }
    let name_end = name_offset
        .checked_add(name_length)
        .ok_or_else(|| "USN filename range overflow".to_string())?;
    if name_end > record_length {
        return Err("USN filename extends beyond its record".to_string());
    }
    let mut utf16 = Vec::with_capacity(name_length / 2);
    for pair in bytes[name_offset..name_end].chunks_exact(2) {
        utf16.push(u16::from_le_bytes([pair[0], pair[1]]));
    }

    Ok((
        ParsedUsnRecord::V2(UsnRecordV2 {
            frn: read_u64(bytes, 8).unwrap(),
            parent_frn: read_u64(bytes, 16).unwrap(),
            usn: read_i64(bytes, 24).unwrap(),
            timestamp: read_i64(bytes, 32).unwrap(),
            reason: read_u32(bytes, 40).unwrap(),
            attributes: read_u32(bytes, 52).unwrap(),
            name: String::from_utf16_lossy(&utf16),
        }),
        record_length,
    ))
}

fn fold_records(records: Vec<UsnRecordV2>) -> Vec<NtfsChange> {
    let mut changes = HashMap::new();
    for record in records {
        if is_root_system_node(&record.name, record.parent_frn) {
            continue;
        }
        if record.reason & USN_REASON_FILE_DELETE != 0 {
            changes.insert(record.frn, NtfsChange::Delete { frn: record.frn });
        } else if record.reason & USN_REASON_RENAME_OLD_NAME != 0 {
            // The old-name record is not a new identity. The corresponding
            // new-name record, even in a later read batch, carries the same FRN
            // and authoritative parent/name. Advancing past this record is safe.
        } else if record.reason & !USN_REASON_CLOSE != 0 {
            changes.insert(record.frn, NtfsChange::Upsert(record.into_node()));
        }
    }
    changes.into_values().collect()
}

pub(super) fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let raw: [u8; 2] = bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(raw))
}

pub(super) fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}

pub(super) fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let raw: [u8; 8] = bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_le_bytes(raw))
}

fn read_i64(bytes: &[u8], offset: usize) -> Option<i64> {
    let raw: [u8; 8] = bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?;
    Some(i64::from_le_bytes(raw))
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Ioctl::{
        USN_REASON_FILE_CREATE, USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME,
    };

    use super::*;

    fn metadata(id: u64, first: i64, next: i64) -> JournalMetadata {
        JournalMetadata {
            journal_id: id,
            first_usn: first,
            next_usn: next,
            lowest_valid_usn: first,
        }
    }

    #[test]
    fn detects_journal_id_mismatch_and_range_gaps() {
        let saved = Some(JournalCheckpoint {
            journal_id: 1,
            next_usn: 100,
        });
        assert_eq!(
            journal_continuity(saved, metadata(2, 10, 200)),
            JournalContinuity::Rebuild(RebuildReason::JournalIdChanged)
        );
        assert_eq!(
            journal_continuity(saved, metadata(1, 101, 200)),
            JournalContinuity::Rebuild(RebuildReason::CursorTruncated)
        );
        assert_eq!(
            journal_continuity(saved, metadata(1, 10, 99)),
            JournalContinuity::Rebuild(RebuildReason::CursorAhead)
        );
        assert_eq!(
            journal_continuity(saved, metadata(1, 10, 100)),
            JournalContinuity::Current
        );
        assert_eq!(
            journal_continuity(saved, metadata(1, 10, 101)),
            JournalContinuity::CatchUp
        );
    }

    #[test]
    fn ntfs_metadata_and_unsearchable_roots_are_dropped() {
        let metadata = UsnRecordV2 {
            frn: 1,
            parent_frn: 5,
            usn: 1,
            timestamp: 0,
            reason: USN_REASON_FILE_CREATE,
            attributes: 0,
            name: "$LogFile".into(),
        };
        let restore_blobs = UsnRecordV2 {
            frn: 2,
            parent_frn: 5,
            usn: 2,
            timestamp: 0,
            reason: USN_REASON_FILE_CREATE,
            attributes: 0,
            name: "System Volume Information".into(),
        };
        let user_file = UsnRecordV2 {
            frn: 3,
            parent_frn: 5,
            usn: 3,
            timestamp: 0,
            reason: USN_REASON_FILE_CREATE,
            attributes: 0,
            name: "notes.txt".into(),
        };
        // A '$'-prefixed name away from the root is an ordinary user file.
        let dollar_named = UsnRecordV2 {
            frn: 4,
            parent_frn: 100,
            usn: 4,
            timestamp: 0,
            reason: USN_REASON_FILE_CREATE,
            attributes: 0,
            name: "$temp-notes.txt".into(),
        };

        let changes = fold_records(vec![metadata, restore_blobs, user_file, dollar_named]);
        let mut names: Vec<&str> = changes
            .iter()
            .filter_map(|change| match change {
                NtfsChange::Upsert(node) => Some(node.name.as_str()),
                NtfsChange::Delete { .. } => None,
            })
            .collect();
        // fold_records dedupes through a HashMap; only membership is defined.
        names.sort_unstable();
        assert_eq!(names, vec!["$temp-notes.txt", "notes.txt"]);
    }

    #[test]
    fn rename_old_is_deferred_and_new_name_is_authoritative() {
        let old = UsnRecordV2 {
            frn: 10,
            parent_frn: 5,
            usn: 1,
            timestamp: 0,
            reason: USN_REASON_RENAME_OLD_NAME,
            attributes: 0,
            name: "old.txt".into(),
        };
        assert!(fold_records(vec![old]).is_empty());
        let new = UsnRecordV2 {
            frn: 10,
            parent_frn: 20,
            usn: 2,
            timestamp: 0,
            reason: USN_REASON_RENAME_NEW_NAME,
            attributes: 0,
            name: "new.rs".into(),
        };
        let changes = fold_records(vec![new]);
        let NtfsChange::Upsert(node) = &changes[0] else {
            panic!("expected upsert");
        };
        assert_eq!(node.parent_frn, 20);
        assert_eq!(node.lower_name, "new.rs");
        assert_eq!(node.extension.as_deref(), Some("rs"));
    }

    #[test]
    fn replayed_create_folds_to_one_upsert() {
        let create = UsnRecordV2 {
            frn: 10,
            parent_frn: 5,
            usn: 1,
            timestamp: 0,
            reason: USN_REASON_FILE_CREATE,
            attributes: 0,
            name: "A.TXT".into(),
        };
        let changes = fold_records(vec![create.clone(), create]);
        assert_eq!(changes.len(), 1);
        let NtfsChange::Upsert(node) = &changes[0] else {
            panic!("expected upsert");
        };
        assert_eq!(node.lower_name, "a.txt");
        assert_eq!(node.extension.as_deref(), Some("txt"));
    }

    #[test]
    fn parser_rejects_truncated_and_out_of_bounds_names() {
        assert!(parse_record(&[0; 7]).is_err());
        let mut record = vec![0u8; 60];
        record[0..4].copy_from_slice(&60u32.to_le_bytes());
        record[4..6].copy_from_slice(&2u16.to_le_bytes());
        record[56..58].copy_from_slice(&2u16.to_le_bytes());
        record[58..60].copy_from_slice(&60u16.to_le_bytes());
        assert!(parse_record(&record).is_err());
    }

    #[test]
    fn parser_skips_future_record_versions_by_declared_length() {
        let mut record = vec![0u8; 16];
        record[0..4].copy_from_slice(&16u32.to_le_bytes());
        record[4..6].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(
            parse_record(&record).unwrap(),
            (ParsedUsnRecord::Unsupported { major_version: 3 }, 16)
        );
    }
}
