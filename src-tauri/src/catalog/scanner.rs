use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FIND_FIRST_EX_LARGE_FETCH,
    WIN32_FIND_DATAW,
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
///   (`filter_changed`), so an idle sweep does almost no writes - and a sweep
///   that changed nothing also skips the FTS rebuild entirely;
/// - junctions and symlinks stay leaves, so no subtree is walked twice;
/// - FTS triggers are dropped during the walk and the index is rebuilt once
///   at the end (`begin_bulk_load` / `end_bulk_load`), but only when rows
///   actually changed (bulk-load writes bypass the FTS triggers).
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
    db.begin_bulk_load()?;
    let result = scan_volume_inner(
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
    );
    // Always restore the FTS triggers, even on cancel - the reference-counted
    // bulk load guard makes this safe under parallel scans. The Database
    // rebuilds the FTS index only if any row was actually written or pruned
    // while the triggers were down; an unchanged re-walk (the common case for
    // overflow reconciles) skips re-tokenizing millions of names for nothing.
    let _ = db.end_bulk_load();
    result
}

/// Reconciles one directory subtree without completing or pruning a
/// volume-wide generation. The per-directory `seen` sets remain authoritative
/// only for directories reached below `root`, so rows elsewhere are untouched.
pub fn reconcile_scope(
    root: &Path,
    volume_id: &str,
    generation: u64,
    db: Arc<Database>,
    app_data_dir: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<u64, String> {
    let _priority = ScanEfficiencyGuard::new();
    db.begin_bulk_load()?;
    let result = scan_volume_inner(
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
    );
    let _ = db.end_bulk_load();
    result
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
    let _priority = ScanEfficiencyGuard::new();
    db.begin_bulk_load()?;
    let result = scan_volume_inner(
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
    );
    let _ = db.end_bulk_load();
    result
}

/// File enumeration is background maintenance. For its duration the worker
/// thread runs at below-normal priority and under Windows power throttling
/// (EcoQoS): the scheduler and frequency governor then run catalog work at
/// efficiency speed. Without the throttle a multi-million-entry walk pins a
/// core at full clock for minutes - priority alone only yields CPU time, it
/// never lowers the power cost of the time it does get.
pub(crate) struct ScanEfficiencyGuard;

impl ScanEfficiencyGuard {
    pub(crate) fn new() -> Self {
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

    while let Some(dir) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Err("scan cancelled".to_string());
        }

        let dir_normalized = dir.to_string_lossy().to_lowercase();
        // Skip Prism's own catalog directory to avoid self-triggering updates
        if dir_normalized.starts_with(&app_data_normalized) {
            continue;
        }

        let search_pattern = format_win32_search_pattern(&dir);
        let wide_pattern: Vec<u16> = search_pattern.encode_utf16().chain(Some(0)).collect();

        let mut find_data = WIN32_FIND_DATAW::default();
        let handle = match unsafe {
            FindFirstFileExW(
                PCWSTR(wide_pattern.as_ptr()),
                FindExInfoBasic,
                &mut find_data as *mut _ as *mut _,
                FindExSearchNameMatch,
                None,
                FIND_FIRST_EX_LARGE_FETCH,
            )
        } {
            Ok(h) => h,
            Err(_) => {
                // Access denied or unreadable directory; continue with remaining queue
                continue;
            }
        };

        let parent_display = dir.to_string_lossy().into_owned();
        let mut seen: HashSet<String> = HashSet::new();

        // NTFS enumeration can return the same entry repeatedly when the
        // directory is being modified concurrently (Postgres data dirs,
        // browser caches, temp churn). Detect the stall and stop enumerating
        // this directory instead of spinning forever. The prune below is
        // skipped for an incomplete enumeration so existing rows survive.
        let mut complete = true;
        let mut repeats = 0u32;
        let mut last_name = String::new();

        loop {
            let filename = wide_filename_to_string(&find_data.cFileName);
            if !filename.is_empty() && filename != "." && filename != ".." {
                if filename == last_name {
                    repeats += 1;
                    if repeats >= 4 {
                        eprintln!(
                            "[Prism Catalog] enumeration stalled on '{}' in {}; stopping this directory",
                            filename, parent_display
                        );
                        complete = false;
                        break;
                    }
                    // Skip re-processing the stalled entry; the first
                    // occurrence already recorded it.
                    let next_ok = unsafe { FindNextFileW(handle, &mut find_data).is_ok() };
                    if !next_ok {
                        break;
                    }
                    continue;
                }
                repeats = 0;
                last_name = filename.clone();
                let attributes = find_data.dwFileAttributes;
                let is_dir = (attributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0;
                let is_reparse = (attributes & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0;

                let child_path = dir.join(&filename);
                let child_display = child_path.to_string_lossy().into_owned();
                let child_normalized = child_display.to_lowercase();
                let lower_name = filename.to_lowercase();
                let filetime_ticks = ((find_data.ftLastWriteTime.dwHighDateTime as u64) << 32)
                    | (find_data.ftLastWriteTime.dwLowDateTime as u64);

                // System-attributed files (desktop.ini, pagefile.sys, ...) are
                // indexed too - full coverage means rank-down at query time,
                // never silent exclusion. Offline cloud placeholders stay
                // searchable: opening one hydrates it through the provider.

                seen.insert(child_normalized.clone());
                total_scanned += 1;

                if is_dir {
                    batch.push(ScannedItem {
                        normalized_path: child_normalized.clone(),
                        display_path: child_display.clone(),
                        name: filename.clone(),
                        lower_name: lower_name.clone(),
                        parent: parent_display.clone(),
                        is_directory: true,
                        extension: None,
                        modified_at: filetime_ticks,
                        size: 0,
                    });

                    if batch.len() >= BATCH_SIZE {
                        flush_file_batch(db, volume_id, plan.generation, &mut batch)?;
                        progress_callback(total_scanned);
                    }

                    // Full coverage: descend everywhere except the excluded
                    // roots. Reparse directories are leaves unless they are
                    // cloud placeholder roots (OneDrive) - see
                    // `is_cloud_reparse_tag`.
                    let descend = if is_reparse {
                        is_cloud_reparse_tag(find_data.dwReserved0)
                    } else {
                        !is_excluded_dir(&lower_name)
                    };
                    if descend && plan.descend_directories {
                        queue.push_back(child_path);
                    }
                } else {
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
                        size: ((find_data.nFileSizeHigh as u64) << 32)
                            | (find_data.nFileSizeLow as u64),
                    });

                    if batch.len() >= BATCH_SIZE {
                        flush_file_batch(db, volume_id, plan.generation, &mut batch)?;
                        progress_callback(total_scanned);
                    }
                }
            }

            let next_ok = unsafe { FindNextFileW(handle, &mut find_data).is_ok() };
            if !next_ok {
                break;
            }
        }

        let _ = unsafe { FindClose(handle) };

        // Children that disappeared since the last scan are removed here - the
        // directory was walked, so everything current is in `seen`. Skipped
        // when the enumeration stalled (partial `seen` would prune live rows).
        if complete {
            db.prune_removed_children(volume_id, &parent_display, &seen)?;
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
    let changed = db.filter_changed(volume_id, batch)?;
    db.insert_batch(volume_id, generation, &changed)?;
    batch.clear();
    Ok(())
}

fn format_win32_search_pattern(dir: &Path) -> String {
    let raw = dir.to_string_lossy();
    let trimmed = raw.trim_end_matches(['\\', '/']);
    if trimmed.starts_with(r"\\?\") {
        format!("{trimmed}\\*")
    } else if let Some(stripped) = trimmed.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{stripped}\*")
    } else {
        format!(r"\\?\{trimmed}\*")
    }
}

fn wide_filename_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    OsString::from_wide(&wide[..end])
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows::Win32::System::SystemServices::{
        IO_REPARSE_TAG_CLOUD_7, IO_REPARSE_TAG_MOUNT_POINT, IO_REPARSE_TAG_SYMLINK,
    };

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("prism-scan-{name}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn scanned_file(path: &Path) -> ScannedItem {
        let display_path = path.to_string_lossy().into_owned();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        ScannedItem {
            normalized_path: display_path.to_lowercase(),
            display_path,
            lower_name: name.to_lowercase(),
            name,
            parent: path.parent().unwrap().to_string_lossy().into_owned(),
            is_directory: false,
            extension: path
                .extension()
                .map(|extension| extension.to_string_lossy().to_lowercase()),
            modified_at: 1,
            size: 1,
        }
    }

    #[test]
    fn cloud_placeholder_roots_are_descended_but_not_other_reparse_points() {
        assert!(is_cloud_reparse_tag(IO_REPARSE_TAG_CLOUD));
        assert!(is_cloud_reparse_tag(IO_REPARSE_TAG_CLOUD_7));
        assert!(!is_cloud_reparse_tag(IO_REPARSE_TAG_MOUNT_POINT));
        assert!(!is_cloud_reparse_tag(IO_REPARSE_TAG_SYMLINK));
        assert!(!is_cloud_reparse_tag(0));
    }

    #[test]
    fn only_unsearchable_roots_are_excluded() {
        assert!(is_excluded_dir("$recycle.bin"));
        assert!(is_excluded_dir("system volume information"));
        assert!(!is_excluded_dir("windows"));
        assert!(!is_excluded_dir("node_modules"));
        assert!(!is_excluded_dir(".git"));
        assert!(!is_excluded_dir("temp"));
        assert!(!is_excluded_dir("windowsapps"));
    }

    #[test]
    fn scoped_repair_converges_only_the_requested_subtree() {
        let dir = temp_dir("scope");
        let scope = dir.join("dirty");
        let outside = dir.join("outside");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let db = Arc::new(Database::open(&dir.join("catalog.db")).unwrap());
        let stale = scope.join("stale-inside.txt");
        let sentinel = outside.join("sentinel-outside.txt");
        db.insert_batch("vol1", 1, &[scanned_file(&stale), scanned_file(&sentinel)])
            .unwrap();

        let fresh = scope.join("fresh-inside.txt");
        std::fs::write(&fresh, "fresh").unwrap();
        reconcile_scope(
            &scope,
            "vol1",
            2,
            db.clone(),
            &dir.join("prism-data"),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(db.search_candidates("stale-inside", 10).unwrap().is_empty());
        assert!(db
            .search_candidates("fresh-inside", 10)
            .unwrap()
            .iter()
            .any(|item| item.display_path == fresh.to_string_lossy()));
        assert!(db
            .search_candidates("sentinel-outside", 10)
            .unwrap()
            .iter()
            .any(|item| item.display_path == sentinel.to_string_lossy()));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn topology_repair_does_not_descend_into_child_shards() {
        let dir = temp_dir("topology");
        let child = dir.join("child-shard");
        std::fs::create_dir_all(&child).unwrap();
        let nested = child.join("nested.txt");
        std::fs::write(&nested, "nested").unwrap();
        let db = Arc::new(Database::open(&dir.join("catalog.db")).unwrap());

        reconcile_directory(
            &dir,
            "vol1",
            1,
            db.clone(),
            &dir.join("prism-data"),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(db
            .search_candidates("child-shard", 10)
            .unwrap()
            .iter()
            .any(|item| item.display_path == child.to_string_lossy() && item.is_directory));
        assert!(db.search_candidates("nested", 10).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
