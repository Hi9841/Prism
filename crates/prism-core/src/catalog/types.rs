use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VolumeState {
    Ready,
    Indexing,
    Offline,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeCoverage {
    pub drive: String,
    pub state: VolumeState,
    pub indexed_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_progress: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub parent: String,
    pub is_directory: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickAccessEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchError {
    pub kind: FileSearchErrorKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileSearchErrorKind {
    IndexQuery,
    DirectoryAccess,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchResponse {
    pub items: Vec<FileEntry>,
    pub ready: bool,
    pub indexing: bool,
    pub path_browse: bool,
    pub volumes: Vec<VolumeCoverage>,
    pub total_indexed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<FileSearchError>,
}

#[derive(Clone, Debug)]
pub struct VolumeInfo {
    pub volume_id: String,
    pub volume_guid_path: Option<String>,
    pub discovery_sources: Vec<String>,
    pub drive_letter: Option<String>,
    pub mount_paths: Vec<PathBuf>,
    pub drive_type: u32,
    pub label: String,
    pub fs_type: String,
    pub accessible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NtfsNode {
    pub frn: u64,
    pub parent_frn: u64,
    pub name: String,
    pub lower_name: String,
    pub extension: Option<String>,
    pub is_directory: bool,
    pub attributes: u32,
    pub modified_at: i64,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JournalMetadata {
    pub journal_id: u64,
    pub first_usn: i64,
    pub next_usn: i64,
    pub lowest_valid_usn: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JournalCheckpoint {
    pub journal_id: u64,
    pub next_usn: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NtfsChange {
    Upsert(NtfsNode),
    Delete { frn: u64 },
}

#[derive(Clone, Debug)]
pub struct ScannedItem {
    pub normalized_path: String,
    pub display_path: String,
    pub name: String,
    pub lower_name: String,
    pub parent: String,
    pub is_directory: bool,
    pub extension: Option<String>,
    pub modified_at: u64,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct CandidateEntry {
    pub id: i64,
    pub display_path: String,
    pub lower_name: String,
    pub is_directory: bool,
    #[allow(dead_code)]
    pub extension: Option<String>,
}

/// One fallback-catalog row as the in-memory name index loads it. The index
/// needs three numbers from the row's path, not the path, so `NameRow::new`
/// computes them while SQLite still holds the text: the paths of a large
/// catalog (over 600 MB for 5.8 M rows) are never copied.
#[derive(Clone, Debug)]
pub struct NameRow {
    pub id: i64,
    pub lower_name: String,
    /// `search::path_penalty` of the row's path and name.
    pub penalty: i32,
    /// `search::path_depth` of the row's path.
    pub depth: usize,
    /// Byte length of the row's path.
    pub path_len: usize,
    pub is_directory: bool,
}

impl NameRow {
    pub fn new(id: i64, lower_name: String, display_path: &str, is_directory: bool) -> Self {
        Self {
            id,
            penalty: super::search::path_penalty(display_path, &lower_name),
            depth: super::search::path_depth(display_path),
            path_len: display_path.len(),
            lower_name,
            is_directory,
        }
    }
}

/// Current state of a fallback row that the name index selected.
#[derive(Clone, Debug)]
pub enum RowLookup {
    Found(CandidateEntry),
    Missing,
    Excluded,
}
