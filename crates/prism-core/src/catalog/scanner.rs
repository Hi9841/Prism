use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_FILES};
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

fn scan_volume_inner(
    root: &Path,
    volume_id: &str,
    plan: ScanPlan,
    db: &Database,
    app_data_dir: &Path,
    cancel: &AtomicBool,
    progress_callback: &impl Fn(u64),
) -> Result<u64, String> {
    let app_data_normalized = app_data_dir
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase();

    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(root.to_path_buf());

    let mut batch = Vec::with_capacity(BATCH_SIZE);
    let mut total_scanned = 0u64;
    let mut read_buffer = vec![0u64; READ_BUFFER_BYTES / 8];

    while let Some(dir) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Err("scan cancelled".to_string());
        }

        let dir_normalized = dir.to_string_lossy().to_lowercase();
        // Skip Prism's own catalog directory to avoid self-triggering updates
        if dir_normalized.starts_with(&app_data_normalized) {
            continue;
        }

        let Some(listing) = read_folder(&dir, &mut read_buffer) else {
            // Access denied or unreadable directory; continue with remaining queue
            continue;
        };

        let parent_display = dir.to_string_lossy().into_owned();
        // The catalog's rows of this folder's children. Each entry the
        // enumeration finds leaves the map; an entry whose write time, size,
        // and kind match its row is not written again. The rows left at the
        // end were removed from disk. A row whose `parent` text differs (a
        // case-only rename of this folder) is not in the map, so its entry is
        // written again with the current parent.
        let mut stored = db.children_of(volume_id, &parent_display)?;

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

            let unchanged = stored.remove(&child_normalized)
                == Some((filetime_ticks as i64, size as i64, is_dir));
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
                    queue.push_back(child_path);
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
        // when the enumeration stalled (the rest of `stored` may be live).
        if listing.complete {
            let removed = stored
                .into_iter()
                .map(|(path, (_, _, is_dir))| (path, is_dir))
                .collect::<Vec<_>>();
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

/// Bytes of the reused listing buffer: one read returns about 600 entries.
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// Lists `dir` with `GetFileInformationByHandleEx(FileFullDirectoryInfo)`
/// into `buffer`. `FindFirstFileExW`/`FindNextFileW` added about 1.5 s of
/// CPU to a walk of 155,000 folders for the same entries. `None` when the
/// folder cannot be opened (access denied, removed).
fn read_folder(dir: &Path, buffer: &mut [u64]) -> Option<FolderListing> {
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
    }
    .ok()?;
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
        let bytes = buffer.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        loop {
            // SAFETY: the system wrote a chain of `FILE_FULL_DIR_INFO` records
            // into `buffer`; each `NextEntryOffset` stays inside it, and each
            // name is `FileNameLength` bytes after its record's start.
            let (entry, name) = unsafe {
                let entry = &*bytes.add(offset).cast::<FILE_FULL_DIR_INFO>();
                let name = std::slice::from_raw_parts(
                    entry.FileName.as_ptr(),
                    entry.FileNameLength as usize / 2,
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
            offset += entry.NextEntryOffset as usize;
        }
    }
    // SAFETY: `handle` was opened above and is closed once.
    let _ = unsafe { CloseHandle(handle) };
    Some(listing)
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
