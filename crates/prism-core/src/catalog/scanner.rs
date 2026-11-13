use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileFullDirectoryInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FULL_DIR_INFO,
    FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::SystemServices::{IO_REPARSE_TAG_CLOUD, IO_REPARSE_TAG_CLOUD_MASK};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, SetThreadPriority, ThreadPowerThrottling,
    THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_NORMAL,
};

use super::db::Database;
use super::types::ScannedItem;

const BATCH_SIZE: usize = 20_000;

#[derive(Clone, Copy)]
struct ScanPlan {
    generation: u64,
    finish_volume: bool,
    descend_directories: bool,
}

#[allow(dead_code)]
pub struct VolumeScanProgress {
    pub indexed_count: u64,
}

/// Directories that never contain files a filename search can find: the
/// recycle bin stores hashed names ($R... copies) and System Volume
/// Information holds restore-point blobs. Everything else is indexed -
/// C:\Windows, node_modules, .git, temp folders included - and merely ranks
/// lower at query time (see `search::path_penalty`). Kept in sync with the
/// NTFS backend's root-node filter in `catalog::ntfs::usn`.
fn is_excluded_dir(name_lower: &str) -> bool {
    matches!(name_lower, "$recycle.bin" | "system volume information")
}

/// Cloud-sync placeholder roots (OneDrive and friends) surface as reparse
/// points, but they hold the user's real files - they must be descended so
/// their contents stay findable. Every other reparse directory (junctions,
/// symlinks, mount points) stays a leaf: following those would loop through
/// ancestor cycles and duplicate already-indexed subtrees.
fn is_cloud_reparse_tag(tag: u32) -> bool {
    (tag & !IO_REPARSE_TAG_CLOUD_MASK) == IO_REPARSE_TAG_CLOUD
}

/// Walks a volume and reconciles the index with what is on disk.
///
/// The walk is full-coverage - every directory except unsearchable roots
/// (`$RECYCLE.BIN`, System Volume Information, Prism's own app data) is
/// indexed, and noisy locations are down-ranked at query time instead of
/// being dropped here. A nested change does not bump ancestor directory
/// mtimes, so the whole tree is enumerated every sweep. Cost is contained
/// three ways:
/// - rows whose (mtime, size) are unchanged are never written again
///   (`Database::children_of`), so an idle sweep does almost no writes;
/// - junctions and symlinks stay leaves, so no subtree is walked twice.
pub fn scan_volume(
    root: &Path,
    volume_id: &str,
    generation: u64,
    db: Arc<Database>,
    app_data_dir: &Path,
    cancel: Arc<AtomicBool>,
    progress_callback: impl Fn(u64),
) -> Result<u64, String> {
    let _priority = ScanEfficiencyGuard::new();
    scan_volume_inner(
        root,
        volume_id,
        ScanPlan {
            generation,
            finish_volume: true,
            descend_directories: true,
        },
        &db,
        app_data_dir,
        &cancel,
        &progress_callback,
        &mut disk_lister(),
    )
}

/// Reconciles one directory subtree without completing or pruning a
/// volume-wide generation. The per-directory `seen` sets remain authoritative
/// only for directories reached below `root`, so rows elsewhere are untouched.
///
/// No bulk load here: a scoped repair writes a few thousand rows at most, and
/// the FTS triggers keep the index current row by row. A bulk load would end
/// with a full FTS rebuild (every name in the catalog, gigabytes of reads)
/// after any change, on every watcher overflow.
pub fn reconcile_scope(
    root: &Path,
    volume_id: &str,
    generation: u64,
    db: Arc<Database>,
    app_data_dir: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<u64, String> {
    let _priority = ScanEfficiencyGuard::new();
    scan_volume_inner(
        root,
        volume_id,
        ScanPlan {
            generation,
            finish_volume: false,
            descend_directories: true,
        },
        &db,
        app_data_dir,
        &cancel,
        &|_| {},
        &mut disk_lister(),
    )
}

/// Reconciles only the direct children of one directory. This is used by the
/// nonrecursive topology watcher; recursive child shards own their subtrees.
pub fn reconcile_directory(
    root: &Path,
    volume_id: &str,
    generation: u64,
    db: Arc<Database>,
    app_data_dir: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<u64, String> {
    // One directory's children: the FTS triggers stay (see reconcile_scope).
    let _priority = ScanEfficiencyGuard::new();
    scan_volume_inner(
        root,
        volume_id,
        ScanPlan {
            generation,
            finish_volume: false,
            descend_directories: false,
        },
        &db,
        app_data_dir,
        &cancel,
        &|_| {},
        &mut disk_lister(),
    )
}

/// File enumeration is background maintenance. For its duration the worker
/// thread runs at below-normal priority and under Windows power throttling
/// (EcoQoS): the scheduler and frequency governor then run catalog work at
/// efficiency speed. Without the throttle a multi-million-entry walk pins a
/// core at full clock for minutes - priority alone only yields CPU time, it
/// never lowers the power cost of the time it does get.
pub struct ScanEfficiencyGuard;

impl ScanEfficiencyGuard {
    pub fn new() -> Self {
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
            let throttled = THREAD_POWER_THROTTLING_STATE {
                Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            };
            let _ = SetThreadInformation(
                GetCurrentThread(),
                ThreadPowerThrottling,
                &throttled as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
            );
        }
        Self
    }

    fn clear() {
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_NORMAL);
            let unthrottled = THREAD_POWER_THROTTLING_STATE {
                Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: 0,
            };
            let _ = SetThreadInformation(
                GetCurrentThread(),
                ThreadPowerThrottling,
                &unthrottled as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
            );
        }
    }
}

impl Drop for ScanEfficiencyGuard {
    fn drop(&mut self) {
        Self::clear();
    }
}

/// Lists a folder for a walk. The walk takes it as a parameter so that a test
/// can make a folder unreadable.
type FolderLister<'a> = dyn FnMut(&Path) -> FolderRead + 'a;

/// The lister of the real file system. It reuses one buffer.
fn disk_lister() -> impl FnMut(&Path) -> FolderRead {
    let mut buffer = vec![0u64; READ_BUFFER_BYTES / 8];
    move |dir| read_folder(dir, &mut buffer)
}

#[allow(clippy::too_many_arguments)]
fn scan_volume_inner(
    root: &Path,
    volume_id: &str,
    plan: ScanPlan,
    db: &Database,
    app_data_dir: &Path,
    cancel: &AtomicBool,
    progress_callback: &impl Fn(u64),
    list: &mut FolderLister<'_>,
) -> Result<u64, String> {
    let app_data_normalized = app_data_dir
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase();

    // The folder and whether its rows may keep another letter case of its
    // path. The root is tested by its own row; below it, a folder whose row
    // has other letters than the listing was renamed in case only.
    let mut queue: VecDeque<(PathBuf, bool)> = VecDeque::new();
    let root_text = root.to_string_lossy().into_owned();
    let root_renamed = db
        .display_path_of(volume_id, &root_text.to_lowercase())?
        .is_some_and(|stored| stored != root_text);
    queue.push_back((root.to_path_buf(), root_renamed));

    let mut batch = Vec::with_capacity(BATCH_SIZE);
    let mut total_scanned = 0u64;

    while let Some((dir, any_case)) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Err("scan cancelled".to_string());
        }

        let dir_normalized = dir.to_string_lossy().to_lowercase();
        // Skip Prism's own catalog directory to avoid self-triggering updates
        if dir_normalized.starts_with(&app_data_normalized) {
            continue;
        }

        let listing = match list(&dir) {
            FolderRead::Listed(listing) => listing,
            // Removed since its parent was listed: its removal has its own
            // event, so nothing is left to repair here.
            FolderRead::Gone => continue,
            // Access denied or unreadable directory; continue with the queue.
            FolderRead::Denied => continue,
        };

        let parent_display = dir.to_string_lossy().into_owned();
        // The catalog's rows of this folder's children. Each entry the
        // enumeration finds leaves the map; an entry whose write time, size,
        // and kind match its row is not written again. The rows left at the
        // end were removed from disk. A row whose `parent` text differs (a
        // case-only rename of this folder) is written again with the current
        // parent. NTFS paths are not case sensitive, so the rows of a folder
        // that was renamed in case are found by their normalized path.
        let mut stored = db.children_of(volume_id, &parent_display, any_case)?;

        for FolderEntry {
            name: filename,
            attributes,
            write_time: filetime_ticks,
            size: file_size,
            reparse_tag,
        } in listing.entries
        {
            let is_dir = (attributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0;
            let is_reparse = (attributes & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0;

            let child_path = dir.join(&filename);
            let child_display = child_path.to_string_lossy().into_owned();
            let child_normalized = child_display.to_lowercase();
            let lower_name = filename.to_lowercase();

            let size = if is_dir { 0 } else { file_size };

            // System-attributed files (desktop.ini, pagefile.sys, ...) are
            // indexed too - full coverage means rank-down at query time,
            // never silent exclusion. Offline cloud placeholders stay
            // searchable: opening one hydrates it through the provider.

            let stored_row = stored.remove(&child_normalized);
            // A folder whose row has other letters than the listing was
            // renamed in case: the rows below it keep the old text.
            let renamed = is_dir
                && stored_row
                    .as_ref()
                    .and_then(|row| row.display_path.as_deref())
                    .is_some_and(|text| text != child_display);
            let unchanged = !renamed
                && stored_row.is_some_and(|row| {
                    row.same_parent
                        && row.modified_at == filetime_ticks as i64
                        && row.size == size as i64
                        && row.is_directory == is_dir
                });
            total_scanned += 1;

            if is_dir {
                if !unchanged {
                    batch.push(ScannedItem {
                        normalized_path: child_normalized,
                        display_path: child_display,
                        name: filename.clone(),
                        lower_name: lower_name.clone(),
                        parent: parent_display.clone(),
                        is_directory: true,
                        extension: None,
                        modified_at: filetime_ticks,
                        size: 0,
                    });
                }

                if batch.len() >= BATCH_SIZE {
                    flush_file_batch(db, volume_id, plan.generation, &mut batch)?;
                    progress_callback(total_scanned);
                }

                // Full coverage: descend everywhere except the excluded
                // roots. Reparse directories are leaves unless they are
                // cloud placeholder roots (OneDrive) - see
                // `is_cloud_reparse_tag`.
                let descend = if is_reparse {
                    is_cloud_reparse_tag(reparse_tag)
                } else {
                    !is_excluded_dir(&lower_name)
                };
                if descend && plan.descend_directories {
                    queue.push_back((child_path, renamed));
                }
            } else if !unchanged {
                let extension = Path::new(&filename)
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_lowercase());

                batch.push(ScannedItem {
                    normalized_path: child_normalized,
                    display_path: child_display,
                    name: filename,
                    lower_name,
                    parent: parent_display.clone(),
                    is_directory: false,
                    extension,
                    modified_at: filetime_ticks,
                    size,
                });

                if batch.len() >= BATCH_SIZE {
                    flush_file_batch(db, volume_id, plan.generation, &mut batch)?;
                    progress_callback(total_scanned);
                }
            }
        }

        // Children that disappeared since the last scan are removed here - the
        // directory was walked, so every current entry left `stored`. Skipped
        // when the enumeration stalled (the rest of `stored` may be live); the
        // periodic sweep takes that folder.
        if listing.complete {
            let mut removed = stored
                .into_iter()
                .map(|(path, row)| (path, row.is_directory))
                .collect::<Vec<_>>();
            // In path order the deletes walk the index once. In the order of
            // the map they hit pages at random: tens of thousands of stale
            // rows (a repair of a folder under churn) then read the 4 GB
            // catalog several times over.
            removed.sort_unstable();
            db.remove_rows(volume_id, &removed)?;
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Err("scan cancelled".to_string());
    }

    flush_file_batch(db, volume_id, plan.generation, &mut batch)?;

    if plan.finish_volume {
        db.finish_volume_scan(volume_id, plan.generation, total_scanned)?;
    }
    progress_callback(total_scanned);

    Ok(total_scanned)
}

fn flush_file_batch(
    db: &Database,
    volume_id: &str,
    generation: u64,
    batch: &mut Vec<ScannedItem>,
) -> Result<(), String> {
    // The watcher can rename or remove an entry between the listing and this
    // write. Drop each item that is gone now, or the walk writes a ghost row.
    // An idle sweep has almost no changed rows, so this test is cheap. It does
    // not follow links, and any error except "not found" counts as "there".
    batch.retain(|item| {
        !matches!(
            std::fs::symlink_metadata(&item.display_path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        )
    });
    if batch.is_empty() {
        return Ok(());
    }
    db.insert_batch(volume_id, generation, batch)?;
    batch.clear();
    Ok(())
}

/// One entry of a folder listing, as the catalog needs it.
struct FolderEntry {
    name: String,
    attributes: u32,
    /// Last write time in FILETIME ticks.
    write_time: u64,
    size: u64,
    /// The reparse tag when `attributes` has `FILE_ATTRIBUTE_REPARSE_POINT`.
    reparse_tag: u32,
}

/// The entries of one folder (without `.` and `..`). `complete` is false when
/// the listing stopped early, so its rows must not be pruned.
struct FolderListing {
    entries: Vec<FolderEntry>,
    complete: bool,
}

/// The result of opening and listing one folder.
enum FolderRead {
    Listed(FolderListing),
    /// The folder does not exist (any more).
    Gone,
    /// The folder exists and cannot be opened.
    Denied,
}

/// Bytes of the reused listing buffer: one read returns about 600 entries.
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// Lists `dir` with `GetFileInformationByHandleEx(FileFullDirectoryInfo)`
/// into `buffer`. `FindFirstFileExW`/`FindNextFileW` added about 1.5 s of
/// CPU to a walk of 155,000 folders for the same entries.
fn read_folder(dir: &Path, buffer: &mut [u64]) -> FolderRead {
    let wide: Vec<u16> = win32_long_path(dir).encode_utf16().chain(Some(0)).collect();
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
            return if code == ERROR_FILE_NOT_FOUND.to_hresult() || code == ERROR_PATH_NOT_FOUND.to_hresult() {
                FolderRead::Gone
            } else {
                FolderRead::Denied
            };
        }
    };
    let mut listing = FolderListing {
        entries: Vec::new(),
        complete: true,
    };
    // NTFS enumeration can return the same entry repeatedly when the
    // directory is being modified concurrently (Postgres data dirs, browser
    // caches, temp churn). Detect the stall and stop listing this directory
    // instead of spinning forever.
    let mut repeats = 0u32;
    'reads: loop {
        // SAFETY: `buffer` is writable for its whole length, 8-byte aligned.
        let read = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileFullDirectoryInfo,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
            )
        };
        if let Err(error) = read {
            // Any error but the end leaves the listing incomplete.
            listing.complete = error.code() == ERROR_NO_MORE_FILES.to_hresult();
            break;
        }
        let buffer_bytes = buffer.len() * 8;
        let bytes = buffer.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        // `FileName` starts at byte 68. A short or oversized name must not
        // be read past `buffer`.
        const HEADER: usize = 68;
        loop {
            let Some(header_end) = offset.checked_add(HEADER) else {
                listing.complete = false;
                break 'reads;
            };
            if header_end > buffer_bytes || offset % 8 != 0 {
                listing.complete = false;
                break 'reads;
            }
            // SAFETY: `offset` is 8-byte aligned and the 68-byte header is
            // inside `buffer`. The name length is checked before the slice.
            let (entry, name) = unsafe {
                let entry = &*bytes.add(offset).cast::<FILE_FULL_DIR_INFO>();
                let name_bytes = entry.FileNameLength as usize;
                let name_end = header_end.checked_add(name_bytes);
                if name_bytes % 2 != 0 || name_end.is_none_or(|end| end > buffer_bytes) {
                    listing.complete = false;
                    break 'reads;
                }
                let name = std::slice::from_raw_parts(
                    bytes.add(header_end).cast::<u16>(),
                    name_bytes / 2,
                );
                (entry, name)
            };
            let name = OsString::from_wide(name).to_string_lossy().into_owned();
            if name != "." && name != ".." {
                if listing.entries.last().is_some_and(|last| last.name == name) {
                    repeats += 1;
                    if repeats >= 4 {
                        eprintln!(
                            "[Prism Catalog] enumeration stalled on '{}' in {}; stopping this directory",
                            name,
                            dir.display()
                        );
                        listing.complete = false;
                        break 'reads;
                    }
                } else {
                    repeats = 0;
                    listing.entries.push(FolderEntry {
                        name,
                        attributes: entry.FileAttributes,
                        write_time: entry.LastWriteTime as u64,
                        size: entry.EndOfFile as u64,
                        reparse_tag: entry.EaSize,
                    });
                }
            }
            if entry.NextEntryOffset == 0 {
                break;
            }
            let next = entry.NextEntryOffset as usize;
            let Some(next_offset) = offset.checked_add(next) else {
                listing.complete = false;
                break 'reads;
            };
            // NextEntryOffset is a multiple of 8, and it must stay in `buffer`.
            if next < 8 || next % 8 != 0 || next_offset > buffer_bytes {
                listing.complete = false;
                break 'reads;
            }
            offset = next_offset;
        }
    }
    // SAFETY: `handle` was opened above and is closed once.
    let _ = unsafe { CloseHandle(handle) };
    FolderRead::Listed(listing)
}

/// `dir` as a `\\?\` path that ends with a separator (a bare `\\?\C:` would
/// name the volume, not its root folder).
fn win32_long_path(dir: &Path) -> String {
    let raw = dir.to_string_lossy();
    let trimmed = raw.trim_end_matches(['\\', '/']);
    if trimmed.starts_with(r"\\?\") {
        format!("{trimmed}\\")
    } else if let Some(stripped) = trimmed.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{stripped}\")
    } else {
        format!(r"\\?\{trimmed}\")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test folder with a catalog, a tree to walk, and an app data folder.
    struct Fixture {
        base: PathBuf,
        tree: PathBuf,
        data: PathBuf,
        db: Arc<Database>,
    }

    const VOLUME: &str = "vol-scan-test";

    impl Fixture {
        fn new(label: &str) -> Self {
            let base = std::env::temp_dir().join(format!("prism-scan-test-{}-{label}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let tree = base.join("tree");
            let data = base.join("data");
            std::fs::create_dir_all(&tree).unwrap();
            std::fs::create_dir_all(&data).unwrap();
            let db = Arc::new(Database::open(&data.join("catalog.db")).unwrap());
            Self { base, tree, data, db }
        }

        fn file(&self, relative: &str) {
            let path = self.tree.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }

        /// A walk of the tree with a lister that can change what the disk shows.
        fn walk(&self, mut change: impl FnMut(&Path, FolderRead) -> FolderRead) -> u64 {
            let mut real = disk_lister();
            let mut list = |dir: &Path| change(dir, real(dir));
            let plan = ScanPlan { generation: 1, finish_volume: false, descend_directories: true };
            scan_volume_inner(
                &self.tree,
                VOLUME,
                plan,
                &self.db,
                &self.data,
                &AtomicBool::new(false),
                &|_| {},
                &mut list,
            )
            .unwrap()
        }

        fn rows_below(&self, folder: &str) -> Vec<String> {
            let text = self.tree.join(folder).to_string_lossy().into_owned();
            let mut rows: Vec<String> = self.db.children_of(VOLUME, &text, true).unwrap().into_keys().collect();
            rows.sort();
            rows
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn as_is(_: &Path, read: FolderRead) -> FolderRead {
        read
    }

    #[test]
    fn a_walk_of_a_readable_tree_counts_every_entry() {
        let fixture = Fixture::new("complete");
        fixture.file("a\\one.txt");
        fixture.file("b.txt");
        assert_eq!(fixture.walk(as_is), 3);
    }

    #[test]
    fn a_folder_that_cannot_be_read_keeps_its_rows() {
        let fixture = Fixture::new("denied");
        fixture.file("ok\\one.txt");
        fixture.file("locked\\two.txt");
        fixture.walk(as_is);
        assert_eq!(fixture.rows_below("locked").len(), 1);

        std::fs::remove_file(fixture.tree.join("locked\\two.txt")).unwrap();
        let denied = |dir: &Path, read| if dir.ends_with("locked") { FolderRead::Denied } else { read };
        fixture.walk(denied);
        assert_eq!(fixture.rows_below("locked").len(), 1, "no pruning of a folder that was not read");

        fixture.walk(as_is);
        assert!(fixture.rows_below("locked").is_empty());
    }

    #[test]
    fn a_stalled_listing_keeps_its_rows_then_a_full_listing_prunes_them() {
        let fixture = Fixture::new("stalled");
        fixture.file("busy\\keep.txt");
        fixture.file("busy\\gone.txt");
        fixture.walk(as_is);
        assert_eq!(fixture.rows_below("busy").len(), 2);

        std::fs::remove_file(fixture.tree.join("busy\\gone.txt")).unwrap();
        fixture.walk(|dir, read| match read {
            FolderRead::Listed(mut listing) if dir.ends_with("busy") => {
                listing.complete = false;
                FolderRead::Listed(listing)
            }
            other => other,
        });
        assert_eq!(fixture.rows_below("busy").len(), 2, "no pruning from a partial listing");

        fixture.walk(as_is);
        assert_eq!(fixture.rows_below("busy").len(), 1);
    }

    #[test]
    fn a_case_only_rename_does_not_leave_the_old_children_behind() {
        let fixture = Fixture::new("case");
        fixture.file("Old\\a.txt");
        fixture.file("Old\\b.txt");
        fixture.file("Old\\Sub\\c.txt");
        fixture.file("Old\\Sub\\d.txt");
        fixture.walk(as_is);
        assert_eq!(fixture.rows_below("Old").len(), 3);

        // The rename event was dropped; a file went away after it.
        std::fs::rename(fixture.tree.join("Old"), fixture.tree.join("old")).unwrap();
        std::fs::remove_file(fixture.tree.join("old\\b.txt")).unwrap();
        std::fs::remove_file(fixture.tree.join("old\\Sub\\d.txt")).unwrap();
        fixture.walk(as_is);

        let old = fixture.tree.join("old").to_string_lossy().to_lowercase();
        assert_eq!(fixture.rows_below("old"), vec![format!("{old}\\a.txt"), format!("{old}\\sub")]);
        let sub = fixture.tree.join("old\\Sub").to_string_lossy().to_lowercase();
        assert_eq!(fixture.rows_below("old\\Sub"), vec![format!("{sub}\\c.txt")]);
        // The survivors carry the new spelling, so the next walk finds them by their parent.
        let text = fixture.tree.join("old").to_string_lossy().into_owned();
        assert_eq!(fixture.db.children_of(VOLUME, &text, false).unwrap().len(), 2);
        let sub_text = fixture.tree.join("old\\Sub").to_string_lossy().into_owned();
        assert_eq!(fixture.db.children_of(VOLUME, &sub_text, false).unwrap().len(), 1);
    }
}
