use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FIND_FIRST_EX_LARGE_FETCH,
    WIN32_FIND_DATAW,
};

use super::db::Database;
use super::types::ScannedItem;

const BATCH_SIZE: usize = 5_000;

#[allow(dead_code)]
pub struct VolumeScanProgress {
    pub indexed_count: u64,
}

pub fn scan_volume(
    root: &Path,
    volume_id: &str,
    generation: u64,
    db: Arc<Database>,
    app_data_dir: &Path,
    cancel: Arc<AtomicBool>,
    progress_callback: impl Fn(u64),
) -> Result<u64, String> {
    let mut queue = VecDeque::new();
    queue.push_back(root.to_path_buf());

    let mut batch = Vec::with_capacity(BATCH_SIZE);
    let mut total_scanned = 0u64;

    let app_data_normalized = app_data_dir
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase();

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

        loop {
            let filename = wide_filename_to_string(&find_data.cFileName);
            if !filename.is_empty() && filename != "." && filename != ".." {
                let is_dir = (find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0;
                let is_reparse = (find_data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0;

                let child_path = dir.join(&filename);
                let child_display = child_path.to_string_lossy().into_owned();
                let child_normalized = child_display.to_lowercase();

                let lower_name = filename.to_lowercase();
                let extension = if is_dir {
                    None
                } else {
                    Path::new(&filename)
                        .extension()
                        .map(|ext| ext.to_string_lossy().to_lowercase())
                };

                let file_size =
                    ((find_data.nFileSizeHigh as u64) << 32) | (find_data.nFileSizeLow as u64);
                let modified_at = ((find_data.ftLastWriteTime.dwHighDateTime as u64) << 32)
                    | (find_data.ftLastWriteTime.dwLowDateTime as u64);

                batch.push(ScannedItem {
                    normalized_path: child_normalized,
                    display_path: child_display,
                    name: filename,
                    lower_name,
                    parent: parent_display.clone(),
                    is_directory: is_dir,
                    extension,
                    modified_at,
                    size: file_size,
                });

                total_scanned += 1;

                if batch.len() >= BATCH_SIZE {
                    db.insert_batch(volume_id, generation, &batch)?;
                    batch.clear();
                    progress_callback(total_scanned);
                }

                // If it is a directory and NOT a reparse point (junction / symlink / mount point), recurse into it
                if is_dir && !is_reparse {
                    queue.push_back(child_path);
                }
            }

            let next_ok = unsafe { FindNextFileW(handle, &mut find_data).is_ok() };
            if !next_ok {
                break;
            }
        }

        let _ = unsafe { FindClose(handle) };
    }

    // Flush remaining items
    if !batch.is_empty() {
        db.insert_batch(volume_id, generation, &batch)?;
        batch.clear();
    }

    if cancel.load(Ordering::Relaxed) {
        return Err("scan cancelled".to_string());
    }

    db.finish_volume_scan(volume_id, generation, total_scanned)?;
    progress_callback(total_scanned);

    Ok(total_scanned)
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
