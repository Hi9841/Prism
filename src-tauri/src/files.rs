//! Fast local file search backed by a compact, persistent fixed-drive index.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::io::Cursor;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Emitter;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetLogicalDriveStringsW, MoveFileExW, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Profile, FOLDERID_Videos, SHGetKnownFolderPath,
};

const CACHE_VERSION: u32 = 3;
const CACHE_TTL_SECONDS: u64 = 6 * 60 * 60;
// The index is a bounded snapshot, not a filesystem watcher. Refreshing it
// every minute causes a full recursive scan and an atomic cache write while
// the launcher is usually hidden. Keep the snapshot reasonably fresh without
// turning idle time into sustained disk I/O.
const REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 16;
const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 20;
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".cache",
    ".gradle",
    ".idea",
    ".venv",
    ".vscode",
    "appdata",
    "__pycache__",
    "build",
    "dist",
    "node_modules",
    "out",
    "target",
    "venv",
    "$recycle.bin",
    "system volume information",
    "windowsapps",
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "users",
    "recovery",
    "perflogs",
    "msocache",
];

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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickAccessEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchResponse {
    pub items: Vec<FileEntry>,
    pub ready: bool,
    pub indexing: bool,
    pub path_browse: bool,
}

/// How often a scan checks whether a newer search has superseded it.
const SEARCH_ABORT_CHECK_INTERVAL: usize = 4_096;

#[derive(Clone, Default)]
pub struct FileIndex {
    inner: Arc<RwLock<IndexData>>,
    /// Latest search generation. A newer keystroke supersedes older scans;
    /// superseded scans abort early instead of finishing useless work.
    search_generation: Arc<AtomicU64>,
}

#[derive(Default)]
struct IndexData {
    entries: Vec<SearchEntry>,
    ready: bool,
    indexing: bool,
}

struct SearchEntry {
    path: Box<str>,
    lower_name: Box<str>,
    is_directory: bool,
}

#[derive(Deserialize, Serialize)]
struct CachedEntry {
    #[serde(rename = "p")]
    path: String,
    #[serde(rename = "d", default, skip_serializing_if = "is_false")]
    is_directory: bool,
}

#[derive(Deserialize, Serialize)]
struct CacheFile {
    version: u32,
    generated_at: u64,
    entries: Vec<CachedEntry>,
}

#[derive(Serialize)]
struct CacheFileRef<'a> {
    version: u32,
    generated_at: u64,
    entries: &'a [CachedEntry],
}

impl FileIndex {
    pub fn search(&self, query: &str, limit: Option<usize>) -> FileSearchResponse {
        let query = query.trim();
        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let (ready, indexing) = self.status();

        if let Some(items) = browse_path(query, limit) {
            return FileSearchResponse {
                items,
                ready,
                indexing,
                path_browse: true,
            };
        }

        let query = query.to_lowercase();
        let Ok(data) = self.inner.read() else {
            return FileSearchResponse {
                items: Vec::new(),
                ready: false,
                indexing: false,
                path_browse: false,
            };
        };
        if query.chars().count() < 2 {
            return FileSearchResponse {
                items: Vec::new(),
                ready: data.ready,
                indexing: data.indexing,
                path_browse: false,
            };
        }

        // Claim this search. A newer search supersedes it, which lets the
        // scan loop abort early when the user has already typed further.
        let generation = self.search_generation.fetch_add(1, Ordering::AcqRel) + 1;

        let tokens: Vec<&str> = query.split_whitespace().collect();
        let mut best: Vec<(i32, &SearchEntry)> = Vec::with_capacity(limit);
        for (index, entry) in data.entries.iter().enumerate() {
            // Cheap cooperative cancellation: only check on a boundary and
            // only when another search has started.
            if index % SEARCH_ABORT_CHECK_INTERVAL == 0
                && self.search_generation.load(Ordering::Acquire) != generation
            {
                return FileSearchResponse {
                    items: Vec::new(),
                    ready: data.ready,
                    indexing: data.indexing,
                    path_browse: false,
                };
            }
            let Some(score) = entry_score(entry, &tokens) else {
                continue;
            };
            let insert_at = best.partition_point(|(current, _)| *current >= score);
            if insert_at < limit {
                best.insert(insert_at, (score, entry));
                best.truncate(limit);
            }
        }

        FileSearchResponse {
            items: best
                .into_iter()
                .filter_map(|(_, entry)| indexed_file_entry(entry))
                .collect(),
            ready: data.ready,
            indexing: data.indexing,
            path_browse: false,
        }
    }

    fn status(&self) -> (bool, bool) {
        self.inner
            .read()
            .map(|data| (data.ready, data.indexing))
            .unwrap_or((false, false))
    }

    fn replace(&self, entries: Vec<CachedEntry>, indexing: bool) {
        if let Ok(mut data) = self.inner.write() {
            data.entries = prepare(entries);
            data.ready = true;
            data.indexing = indexing;
        }
    }

    /// Returns true when a fresh scan differs from the in-memory snapshot.
    fn changed(&self, next: &[CachedEntry]) -> bool {
        self.inner
            .read()
            .map(|data| !same_entries(&data.entries, next))
            .unwrap_or(false)
    }

    fn set_indexing(&self, indexing: bool) {
        if let Ok(mut data) = self.inner.write() {
            data.indexing = indexing;
        }
    }
}

/// Absolute paths are browsed directly so opening a known folder never waits
/// for (or depends on) the background index. A non-existent final component is
/// treated as a partial filename within its existing parent directory.
fn browse_path(query: &str, limit: usize) -> Option<Vec<FileEntry>> {
    let requested = PathBuf::from(query);
    if !requested.is_absolute() {
        return None;
    }

    if requested.is_dir() {
        return Some(list_directory(&requested, None, limit));
    }
    if requested.is_file() {
        return Some(path_entry(&requested).into_iter().collect());
    }

    let parent = requested.parent()?;
    if !parent.is_dir() {
        return Some(Vec::new());
    }
    let needle = requested
        .file_name()
        .map(|name| name.to_string_lossy().trim().to_lowercase())
        .unwrap_or_default();
    Some(list_directory(parent, Some(&needle), limit))
}

fn list_directory(directory: &Path, needle: Option<&str>, limit: usize) -> Vec<FileEntry> {
    let Ok(children) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let parent = directory.to_string_lossy().into_owned();
    let mut entries: Vec<(i32, FileEntry)> = children
        .flatten()
        .filter_map(|child| {
            let name = child.file_name().to_string_lossy().trim().to_string();
            if name.is_empty() {
                return None;
            }
            // DirEntry::file_type() comes free from the enumeration; a fresh
            // stat per child would cost thousands of syscalls in big folders.
            let is_directory = child.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            let score = match needle {
                Some(value) if !value.is_empty() => target_score(value, &name.to_lowercase())?,
                _ => 0,
            };
            Some((
                score,
                FileEntry {
                    name,
                    path: child.path().to_string_lossy().into_owned(),
                    parent: parent.clone(),
                    is_directory,
                    thumbnail: (!is_directory)
                        .then(|| image_thumbnail(&child.path()))
                        .flatten(),
                },
            ))
        })
        .collect();

    entries.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| right.is_directory.cmp(&left.is_directory))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    entries
        .into_iter()
        .take(limit)
        .map(|(_, entry)| entry)
        .collect()
}

fn path_entry(path: &Path) -> Option<FileEntry> {
    let name = path.file_name()?.to_string_lossy().trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(FileEntry {
        name,
        path: path.to_string_lossy().into_owned(),
        parent: path
            .parent()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default(),
        is_directory: path.is_dir(),
        thumbnail: (!path.is_dir()).then(|| image_thumbnail(path)).flatten(),
    })
}

pub fn warm(index: FileIndex, cache_path: PathBuf, app: tauri::AppHandle) {
    index.set_indexing(true);
    tauri::async_runtime::spawn(async move {
        let mut refresh_now = true;
        let cached_path = cache_path.clone();
        if let Ok(Ok((entries, fresh))) =
            tauri::async_runtime::spawn_blocking(move || load_cache(&cached_path)).await
        {
            index.replace(entries, !fresh);
            let _ = app.emit("file-index-updated", ());
            if fresh {
                refresh_now = false;
            }
        }

        if refresh_now {
            refresh_index(&index, &cache_path, &app).await;
        }

        loop {
            tokio::time::sleep(REFRESH_INTERVAL).await;
            refresh_index(&index, &cache_path, &app).await;
        }
    });
}

fn same_entries(current: &[SearchEntry], next: &[CachedEntry]) -> bool {
    current.len() == next.len()
        && current.iter().zip(next).all(|(current, next)| {
            current.path.as_ref() == next.path && current.is_directory == next.is_directory
        })
}

async fn refresh_index(index: &FileIndex, cache_path: &Path, app: &tauri::AppHandle) {
    index.set_indexing(true);
    let scan_path = cache_path.to_path_buf();
    let index_for_task = index.clone();
    let changed = tauri::async_runtime::spawn_blocking(move || {
        let entries = scan_user_folders();
        // Every successful scan refreshes the verification timestamp, so a
        // boot shortly after a periodic refresh skips the startup rescan even
        // when the unchanged snapshot was not rewritten.
        let _ = write_verified_at(&scan_path);
        // Identical scans skip the multi-MB cache rewrite entirely.
        if !index_for_task.changed(&entries) {
            return false;
        }
        // A cache write failure must not discard a valid in-memory scan.
        let _ = save_cache(&scan_path, &entries);
        index_for_task.replace(entries, false);
        true
    })
    .await
    .unwrap_or(false);

    if changed {
        let _ = app.emit("file-index-updated", ());
    } else {
        index.set_indexing(false);
    }
}

pub fn quick_access() -> Vec<QuickAccessEntry> {
    known_locations()
        .into_iter()
        .filter(|(_, path, _)| path.is_dir())
        .map(|(name, path, kind)| QuickAccessEntry {
            name: name.to_string(),
            path: path.to_string_lossy().into_owned(),
            kind: kind.to_string(),
        })
        .collect()
}

fn scan_user_folders() -> Vec<CachedEntry> {
    let mut roots = fixed_drive_roots();
    roots.extend(
        known_locations()
            .into_iter()
            .filter(|(_, _, kind)| *kind != "home")
            .map(|(_, path, _)| path)
            .filter(|path| path.is_dir()),
    );
    roots.sort();
    roots.dedup();

    let mut entries = Vec::with_capacity(16_384);
    let mut queue = VecDeque::new();
    for root in roots {
        queue.push_back((root, 0usize));
    }

    while let Some((dir, depth)) = queue.pop_front() {
        if entries.len() >= MAX_ENTRIES || depth > MAX_DEPTH {
            break;
        }
        let Ok(children) = std::fs::read_dir(&dir) else {
            continue;
        };
        for child in children.flatten() {
            if entries.len() >= MAX_ENTRIES {
                break;
            }
            let path = child.path();
            let Ok(file_type) = child.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = child.file_name().to_string_lossy().trim().to_string();
            if name.is_empty() {
                continue;
            }
            let is_directory = file_type.is_dir();
            if is_directory && should_skip_dir(&name) {
                continue;
            }
            let path_text = path.to_string_lossy().into_owned();
            entries.push(CachedEntry {
                path: path_text,
                is_directory,
            });
            if is_directory && depth < MAX_DEPTH {
                queue.push_back((path, depth + 1));
            }
        }
    }
    entries
}

fn fixed_drive_roots() -> Vec<PathBuf> {
    let required = unsafe { GetLogicalDriveStringsW(None) };
    if required == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; required as usize + 1];
    let written = unsafe { GetLogicalDriveStringsW(Some(&mut buffer)) } as usize;
    if written == 0 || written > buffer.len() {
        return Vec::new();
    }

    buffer[..written]
        .split(|unit| *unit == 0)
        .filter(|slice| !slice.is_empty())
        .filter_map(|slice| {
            let mut root = slice.to_vec();
            root.push(0);
            (unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } == DRIVE_FIXED)
                .then(|| PathBuf::from(String::from_utf16_lossy(slice)))
        })
        .collect()
}

fn should_skip_dir(name: &str) -> bool {
    let lower = name.to_lowercase();
    SKIP_DIRS.iter().any(|skip| lower == *skip)
}

fn prepare(entries: Vec<CachedEntry>) -> Vec<SearchEntry> {
    entries
        .into_iter()
        .filter_map(|value| {
            let lower_name = Path::new(&value.path)
                .file_name()?
                .to_string_lossy()
                .trim()
                .to_lowercase();
            if lower_name.is_empty() {
                return None;
            }
            Some(SearchEntry {
                path: value.path.into_boxed_str(),
                lower_name: lower_name.into_boxed_str(),
                is_directory: value.is_directory,
            })
        })
        .collect()
}

fn entry_score(entry: &SearchEntry, tokens: &[&str]) -> Option<i32> {
    let mut total = if entry.is_directory { 20 } else { 0 };
    for token in tokens {
        total += target_score(token, &entry.lower_name)?;
    }
    Some(total - entry.lower_name.len().min(80) as i32)
}

fn indexed_file_entry(entry: &SearchEntry) -> Option<FileEntry> {
    let path = Path::new(entry.path.as_ref());
    let metadata = std::fs::metadata(path).ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some(FileEntry {
        name,
        path: entry.path.to_string(),
        parent: path
            .parent()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default(),
        is_directory: metadata.is_dir(),
        // Indexed search stays metadata-only. Thumbnails are requested after
        // the row is visible so image decoding never delays search results.
        thumbnail: None,
    })
}

const THUMBNAIL_MAX_BYTES: u64 = 32 * 1024 * 1024;
const THUMBNAIL_SIZE: u32 = 64;

fn image_thumbnail(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_string_lossy().to_lowercase();
    if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
    ) {
        return None;
    }
    if std::fs::metadata(path).ok()?.len() > THUMBNAIL_MAX_BYTES {
        return None;
    }
    let image = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let preview = image.thumbnail(THUMBNAIL_SIZE, THUMBNAIL_SIZE);
    let mut bytes = Cursor::new(Vec::new());
    preview.write_to(&mut bytes, image::ImageFormat::Png).ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    ))
}

/// Loads a thumbnail for an absolute, existing file on demand.
pub fn file_thumbnail(path: &str) -> Option<String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.is_dir() {
        return None;
    }
    image_thumbnail(path)
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn target_score(query: &str, target: &str) -> Option<i32> {
    if target == query {
        return Some(1_000);
    }
    if target.starts_with(query) {
        return Some(800);
    }
    if let Some(position) = target.find(query) {
        let boundary = position == 0
            || target
                .as_bytes()
                .get(position.wrapping_sub(1))
                .is_some_and(|byte| matches!(*byte, b' ' | b'-' | b'_' | b'.' | b'\\' | b'/'));
        return Some(620 - position.min(100) as i32 * 2 + if boundary { 80 } else { 0 });
    }

    // Byte-level subsequence matching: in UTF-8 a byte subsequence matches
    // exactly the same strings as the char subsequence (each char's bytes are
    // contiguous), which keeps the hottest loop allocation- and decode-free.
    let query_bytes = query.as_bytes();
    let target_bytes = target.as_bytes();
    let query_length = query_bytes.len() as i32;
    let mut current = *query_bytes.first()?;
    let mut matched = 0i32;
    let mut gaps = 0i32;
    for &byte in target_bytes {
        if byte == current {
            matched += 1;
            if let Some(&next) = query_bytes.get(matched as usize) {
                current = next;
            } else {
                if query_length >= 3 && gaps > (query_length * 2).max(6) {
                    return None;
                }
                return Some(300 + matched * 8 - gaps.min(120));
            }
        } else if matched > 0 {
            gaps += 1;
        }
    }
    None
}

fn load_cache(path: &Path) -> Result<(Vec<CachedEntry>, bool), String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let cache: CacheFile = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    if cache.version != CACHE_VERSION {
        return Err("stale file index".to_string());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let age = now.saturating_sub(cache.generated_at);
    // A recent verification pass (sidecar timestamp) also counts as fresh,
    // since it rescanned the same unchanged snapshot.
    let verified_fresh = read_verified_at(path)
        .map(|verified| now.saturating_sub(verified) <= CACHE_TTL_SECONDS)
        .unwrap_or(false);
    Ok((cache.entries, age <= CACHE_TTL_SECONDS || verified_fresh))
}

fn verified_path(cache_path: &Path) -> PathBuf {
    cache_path.with_extension("verified")
}

fn write_verified_at(cache_path: &Path) -> std::io::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let path = verified_path(cache_path);
    let temp = path.with_extension("verified.tmp");
    std::fs::write(&temp, now.to_string())?;
    crate::files::replace_file(&temp, &path).map_err(std::io::Error::other)
}

fn read_verified_at(cache_path: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(verified_path(cache_path)).ok()?;
    text.trim().parse().ok()
}

fn save_cache(path: &Path, entries: &[CachedEntry]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let cache = CacheFileRef {
        version: CACHE_VERSION,
        generated_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        entries,
    };
    let text = serde_json::to_vec(&cache).map_err(|error| error.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|error| error.to_string())?;
    replace_file(&temp, path)
}

pub(crate) fn replace_file(temp: &Path, destination: &Path) -> Result<(), String> {
    let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| error.to_string())
    }
}

fn known_locations() -> Vec<(&'static str, PathBuf, &'static str)> {
    [
        ("Home", &FOLDERID_Profile, "home"),
        ("Desktop", &FOLDERID_Desktop, "desktop"),
        ("Downloads", &FOLDERID_Downloads, "downloads"),
        ("Documents", &FOLDERID_Documents, "documents"),
        ("Pictures", &FOLDERID_Pictures, "pictures"),
        ("Music", &FOLDERID_Music, "music"),
        ("Videos", &FOLDERID_Videos, "videos"),
    ]
    .into_iter()
    .filter_map(|(name, id, kind)| known_folder(id).map(|path| (name, path, kind)))
    .collect()
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let path = SHGetKnownFolderPath(id, Default::default(), None).ok()?;
        let text = path.to_string().unwrap_or_default();
        CoTaskMemFree(Some(path.as_ptr() as *const c_void));
        if text.is_empty() {
            None
        } else {
            Some(PathBuf::from(text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, path: &str, is_directory: bool) -> SearchEntry {
        SearchEntry {
            path: path.into(),
            lower_name: name.to_lowercase().into(),
            is_directory,
        }
    }

    #[test]
    fn exact_and_prefix_names_rank_first() {
        let exact = entry("report.pdf", r"C:\Users\me\Documents\report.pdf", false);
        let prefix = entry(
            "report-final.pdf",
            r"C:\Users\me\Downloads\report-final.pdf",
            false,
        );
        let unrelated = entry("notes.txt", r"C:\Users\me\report\notes.txt", false);
        let tokens = ["report"];
        assert!(entry_score(&exact, &tokens) > entry_score(&prefix, &tokens));
        assert!(entry_score(&unrelated, &tokens).is_none());
    }

    #[test]
    fn supports_fuzzy_and_multi_token_matches() {
        let file = entry(
            "Project Roadmap 2026.pdf",
            r"C:\Users\me\Documents\Project Roadmap 2026.pdf",
            false,
        );
        assert!(entry_score(&file, &["prjct"]).is_some());
        assert!(entry_score(&file, &["road", "2026"]).is_some());
        assert!(entry_score(&file, &["road", "missing"]).is_none());
        assert!(target_score("wez", "windows performance analyzer").is_none());
    }

    #[test]
    fn search_is_capped_and_requires_two_characters() {
        let base = std::env::temp_dir().join(format!(
            "prism-file-search-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).expect("create search test folder");
        let entries = (0..30)
            .map(|number| {
                let path = base.join(format!("report-{number}.txt"));
                std::fs::write(&path, "test").expect("create search test file");
                CachedEntry {
                    path: path.to_string_lossy().into_owned(),
                    is_directory: false,
                }
            })
            .collect();
        let index = FileIndex::default();
        index.replace(entries, false);
        assert!(index.search("r", Some(5)).items.is_empty());
        assert_eq!(index.search("report", Some(5)).items.len(), 5);
        assert_eq!(index.search("report", Some(200)).items.len(), 20);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn deleted_index_entry_is_never_returned() {
        let path = std::env::temp_dir().join(format!(
            "prism-vanishing-note-{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&path, "test").expect("create indexed test file");
        let index = FileIndex::default();
        index.replace(
            vec![CachedEntry {
                path: path.to_string_lossy().into_owned(),
                is_directory: false,
            }],
            false,
        );
        assert_eq!(index.search("vanishing", Some(5)).items.len(), 1);
        std::fs::remove_file(&path).expect("delete indexed test file");
        assert!(index.search("vanishing", Some(5)).items.is_empty());
    }

    #[test]
    fn indexed_results_defer_thumbnail_loading() {
        let path = std::env::temp_dir().join(format!(
            "prism-lazy-preview-{}.png",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        image::RgbaImage::new(1, 1)
            .save(&path)
            .expect("create preview test image");
        let path_text = path.to_string_lossy().into_owned();
        let index = FileIndex::default();
        index.replace(
            vec![CachedEntry {
                path: path_text.clone(),
                is_directory: false,
            }],
            false,
        );

        let result = index.search("prism-lazy", Some(5));
        assert_eq!(result.items.len(), 1);
        assert!(result.items[0].thumbnail.is_none());
        assert!(file_thumbnail(&path_text).is_some());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn absolute_directory_browse_does_not_depend_on_the_index() {
        let base = std::env::temp_dir().join(format!(
            "prism-file-browse-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let folder = base.join("Saved");
        std::fs::create_dir_all(&folder).expect("create test folder");
        std::fs::write(base.join("notes.txt"), "test").expect("create test file");

        let response = FileIndex::default().search(&base.to_string_lossy(), Some(8));
        assert!(response.path_browse);
        assert!(!response.ready);
        assert_eq!(response.items.len(), 2);
        assert_eq!(response.items[0].name, "Saved");
        assert!(response.items[0].is_directory);

        let partial = base.join("sav");
        let response = FileIndex::default().search(&partial.to_string_lossy(), Some(8));
        assert!(response.path_browse);
        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].name, "Saved");

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn cache_replace_overwrites_an_existing_file() {
        let base = std::env::temp_dir().join(format!(
            "prism-file-replace-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let destination = base.with_extension("json");
        let temp = base.with_extension("tmp");
        std::fs::write(&destination, "old").expect("write old file");
        std::fs::write(&temp, "new").expect("write replacement");
        replace_file(&temp, &destination).expect("replace existing file");
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new");
        let _ = std::fs::remove_file(destination);
    }

    #[test]
    fn generated_directories_are_excluded() {
        for name in [
            "build",
            "dist",
            "out",
            ".venv",
            "__pycache__",
            "Windows",
            "Program Files",
            "ProgramData",
            "Users",
        ] {
            assert!(should_skip_dir(name), "should skip {name}");
        }
        assert!(!should_skip_dir("Projects"));
    }

    /// Manual hot-path benchmark: per-search cost against a full 100k index.
    #[test]
    #[ignore]
    fn search_bench() {
        use std::time::Instant;

        let entries = (0..100_000)
            .map(|number| CachedEntry {
                path: format!(
                    r"C:\Users\bench\Documents\folder-{}\file-{}-report-{}.txt",
                    number % 200,
                    number,
                    number % 50
                ),
                is_directory: number % 10 == 0,
            })
            .collect();
        let index = FileIndex::default();
        index.replace(entries, false);

        for query in ["report", "rep", "report 7", "zzz", "folder-19 file"] {
            let started = Instant::now();
            let mut hits = 0;
            for _ in 0..50 {
                hits = index.search(query, Some(8)).items.len();
            }
            eprintln!(
                "search {query:?}: {:.2} ms/search ({} hits)",
                started.elapsed().as_secs_f64() * 1000.0 / 50.0,
                hits
            );
        }
    }
}
