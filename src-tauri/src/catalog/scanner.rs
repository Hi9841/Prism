use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_SYSTEM, FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
};

use super::db::Database;
use super::types::ScannedItem;

const BATCH_SIZE: usize = 20_000;

#[allow(dead_code)]
pub struct VolumeScanProgress {
    pub indexed_count: u64,
}

/// Directories that are pure noise for a file launcher. Kept in sync with the
/// one-time purge patterns in `db::EXCLUDED_PURGE_PATTERNS`.
fn is_excluded_dir(name_lower: &str, parent_normalized: &str, root_normalized: &str) -> bool {
    match name_lower {
        "node_modules"
        | ".git"
        | ".svn"
        | ".hg"
        | "$recycle.bin"
        | "system volume information"
        | "windows.old"
        | "$windows.~bt"
        | "$windows.~ws"
        | "recovery"
        | "perflogs"
        | "windowsapps" => true,
        // The Windows directory only at the volume root: it is hundreds of
        // thousands of entries of system noise.
        "windows" => parent_normalized == root_normalized,
        // Per-user temp cache; only under AppData\Local so project "temp"
        // folders are untouched.
        "temp" => parent_normalized.contains("\\appdata\\local"),
        _ => false,
    }
}

fn is_noise_entry(attributes: u32) -> bool {
    // System files (desktop.ini, thumbs.db, ...) and offline cloud placeholders
    // are never useful search results.
    (attributes & (FILE_ATTRIBUTE_SYSTEM.0 | FILE_ATTRIBUTE_OFFLINE.0)) != 0
}

/// Walks a volume and reconciles the index with what is on disk.
///
/// The whole tree is enumerated every sweep - a nested change does not bump
/// ancestor directory mtimes, so subtree pruning by mtime would miss changes.
/// Cost is contained three ways:
/// - rows whose (mtime, size) are unchanged are never written again
///   (`filter_changed`), so an idle sweep does almost no writes;
/// - high-noise directories (node_modules, .git, C:\Windows, ...) are skipped;
/// - FTS triggers are dropped during the walk and the index is rebuilt once at
///   the end (`begin_bulk_load` / `end_bulk_load`).
pub fn scan_volume(
    root: &Path,
    volume_id: &str,
    generation: u64,
    db: Arc<Database>,
    app_data_dir: &Path,
    cancel: Arc<AtomicBool>,
    progress_callback: impl Fn(u64),
) -> Result<u64, String> {
    db.begin_bulk_load()?;
    let result = scan_volume_inner(
        root,
        volume_id,
        generation,
        &db,
        app_data_dir,
        &cancel,
        &progress_callback,
    );
    // Always restore the FTS triggers, even on cancel - the reference-counted
    // bulk load guard makes this safe under parallel scans.
    let _ = db.end_bulk_load();
    result
}

fn scan_volume_inner(
    root: &Path,
    volume_id: &str,
    generation: u64,
    db: &Database,
    app_data_dir: &Path,
    cancel: &AtomicBool,
    progress_callback: &impl Fn(u64),
) -> Result<u64, String> {
    let root_normalized = root.to_string_lossy().to_lowercase();
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

        loop {
            let filename = wide_filename_to_string(&find_data.cFileName);
            if !filename.is_empty() && filename != "." && filename != ".." {
                let attributes = find_data.dwFileAttributes;
                let is_dir = (attributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0;
                let is_reparse = (attributes & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0;

                let child_path = dir.join(&filename);
                let child_display = child_path.to_string_lossy().into_owned();
                let child_normalized = child_display.to_lowercase();
                let lower_name = filename.to_lowercase();
                let filetime_ticks = ((find_data.ftLastWriteTime.dwHighDateTime as u64) << 32)
                    | (find_data.ftLastWriteTime.dwLowDateTime as u64);

                if is_noise_entry(attributes) {
                    continue;
                }

                seen.insert(child_normalized.clone());
                total_scanned += 1;

                if is_dir {
                    if !is_reparse
                        && !is_excluded_dir(&lower_name, &dir_normalized, &root_normalized)
                    {
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
                        flush_file_batch(db, volume_id, generation, &mut batch)?;
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
        // directory was walked, so everything current is in `seen`.
        db.prune_removed_children(volume_id, &parent_display, &seen)?;
    }

    if cancel.load(Ordering::Relaxed) {
        return Err("scan cancelled".to_string());
    }

    flush_file_batch(db, volume_id, generation, &mut batch)?;

    db.finish_volume_scan(volume_id, generation, total_scanned)?;
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
