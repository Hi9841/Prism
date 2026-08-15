use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::UNIX_EPOCH;

use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED,
    FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_CREATION,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};

use super::db::Database;
use super::types::ScannedItem;
use super::IndexCounts;

const BUFFER_SIZE: usize = 256 * 1024; // 256 KB buffer

#[derive(Clone, Debug)]
pub enum WatcherEvent {
    Added(PathBuf),
    Removed(PathBuf),
    Modified(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Overflow,
}

pub struct VolumeWatcher {
    #[allow(dead_code)]
    root: PathBuf,
    volume_id: String,
    #[allow(dead_code)]
    stop: Arc<AtomicBool>,
    #[allow(dead_code)]
    handle: Option<JoinHandle<()>>,
    event_queue: Arc<Mutex<Vec<WatcherEvent>>>,
    buffering: Arc<AtomicBool>,
    counts: IndexCounts,
    drive: String,
    on_overflow: Arc<dyn Fn(String) + Send + Sync>,
}

impl VolumeWatcher {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        root: PathBuf,
        volume_id: String,
        db: Arc<Database>,
        app_data_dir: PathBuf,
        counts: IndexCounts,
        drive: String,
        on_overflow: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let event_queue = Arc::new(Mutex::new(Vec::new()));
        let buffering = Arc::new(AtomicBool::new(true));
        let on_overflow: Arc<dyn Fn(String) + Send + Sync> = on_overflow;

        let stop_clone = stop.clone();
        let queue_clone = event_queue.clone();
        let buffering_clone = buffering.clone();
        let root_clone = root.clone();
        let vol_id_clone = volume_id.clone();
        let counts_clone = counts.clone();
        let drive_clone = drive.clone();
        let overflow_clone = on_overflow.clone();

        let handle = std::thread::Builder::new()
            .name(format!("prism-watch-{}", volume_id))
            .spawn(move || {
                run_watcher_loop(
                    root_clone,
                    vol_id_clone,
                    db,
                    app_data_dir,
                    stop_clone,
                    queue_clone,
                    buffering_clone,
                    counts_clone,
                    drive_clone,
                    overflow_clone,
                );
            })
            .ok()?;

        Some(Self {
            root,
            volume_id,
            stop,
            handle: Some(handle),
            event_queue,
            buffering,
            counts,
            drive,
            on_overflow,
        })
    }

    pub fn set_buffering(&self, buffering: bool) {
        self.buffering.store(buffering, Ordering::SeqCst);
    }

    /// Applies queued events as a single batched write: events are folded to
    /// one final state per path, renames are paired, and removals win over
    /// earlier upserts. An overflow observed while buffering schedules a
    /// reconcile, because the events it would have contained were lost.
    pub fn flush_queue(&self, db: &Database) {
        self.buffering.store(false, Ordering::SeqCst);
        let events: Vec<WatcherEvent> = {
            let mut q = self.event_queue.lock().unwrap();
            q.drain(..).collect()
        };

        let mut overflow = false;
        let mut renames: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut touched: Vec<PathBuf> = Vec::new();
        let mut removed: Vec<PathBuf> = Vec::new();

        for event in events {
            match event {
                WatcherEvent::Overflow => overflow = true,
                WatcherEvent::Renamed { from, to } => renames.push((from, to)),
                WatcherEvent::Removed(path) => {
                    touched.retain(|t| t != &path);
                    if !removed.contains(&path) {
                        removed.push(path);
                    }
                }
                WatcherEvent::Added(path) | WatcherEvent::Modified(path) => {
                    removed.retain(|r| r != &path);
                    if !touched.contains(&path) {
                        touched.push(path);
                    }
                }
            }
        }

        let mut delta = 0i64;

        for (from, to) in &renames {
            let old_normalized = from.to_string_lossy().to_lowercase();
            if let Some(item) = inspect_single_path(to) {
                delta += db
                    .rename_file(&self.volume_id, &old_normalized, &item)
                    .unwrap_or(0);
            } else {
                // Target vanished before it could be inspected; treat as removal.
                delta -= db
                    .remove_file(&self.volume_id, &old_normalized, true)
                    .unwrap_or(0);
            }
        }

        let items: Vec<ScannedItem> = touched
            .iter()
            .filter_map(|path| inspect_single_path(path))
            .collect();
        delta += db.insert_batch(&self.volume_id, 0, &items).unwrap_or(0) as i64;

        for path in &removed {
            let normalized = path.to_string_lossy().to_lowercase();
            delta -= db
                .remove_file(&self.volume_id, &normalized, true)
                .unwrap_or(0);
        }

        self.counts.adjust(&self.drive, delta);

        if overflow {
            (self.on_overflow)(self.volume_id.clone());
        }
    }

    #[allow(dead_code)]
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_watcher_loop(
    root: PathBuf,
    volume_id: String,
    db: Arc<Database>,
    app_data_dir: PathBuf,
    stop: Arc<AtomicBool>,
    event_queue: Arc<Mutex<Vec<WatcherEvent>>>,
    buffering: Arc<AtomicBool>,
    counts: IndexCounts,
    drive: String,
    on_overflow: Arc<dyn Fn(String) + Send + Sync>,
) {
    let wide_root: Vec<u16> = root
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();

    let dir_handle = match unsafe {
        CreateFileW(
            PCWSTR(wide_root.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    } {
        Ok(h) => h,
        Err(_) => return,
    };

    let mut buffer = vec![0u8; BUFFER_SIZE];
    let notify_filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_CREATION
        | FILE_NOTIFY_CHANGE_SIZE;

    let app_data_normalized = app_data_dir.to_string_lossy().to_lowercase();
    let mut pending_rename_old: Option<PathBuf> = None;

    while !stop.load(Ordering::Relaxed) {
        let mut bytes_returned = 0u32;
        let success = unsafe {
            ReadDirectoryChangesW(
                dir_handle,
                buffer.as_mut_ptr() as *mut _,
                BUFFER_SIZE as u32,
                true,
                notify_filter,
                Some(&mut bytes_returned),
                None,
                None,
            )
            .is_ok()
        };

        if !success || bytes_returned == 0 {
            if !stop.load(Ordering::Relaxed) {
                // Buffer overflow or communication lost
                if buffering.load(Ordering::SeqCst) {
                    event_queue.lock().unwrap().push(WatcherEvent::Overflow);
                } else {
                    on_overflow(volume_id.clone());
                }
            }
            break;
        }

        let mut offset = 0;
        loop {
            if offset + std::mem::size_of::<FILE_NOTIFY_INFORMATION>() > bytes_returned as usize {
                break;
            }

            let info = unsafe { &*(buffer.as_ptr().add(offset) as *const FILE_NOTIFY_INFORMATION) };
            let name_len = (info.FileNameLength / 2) as usize;
            let name_slice = unsafe {
                std::slice::from_raw_parts(buffer.as_ptr().add(offset + 12) as *const u16, name_len)
            };
            let relative_name = OsString::from_wide(name_slice)
                .to_string_lossy()
                .into_owned();
            let target_path = root.join(&relative_name);
            let target_normalized = target_path.to_string_lossy().to_lowercase();

            // Exclude Prism's own app data directory
            if !target_normalized.starts_with(&app_data_normalized) {
                match info.Action {
                    FILE_ACTION_ADDED => {
                        let event = WatcherEvent::Added(target_path);
                        dispatch_event(
                            &volume_id,
                            &db,
                            &event_queue,
                            &buffering,
                            &counts,
                            &drive,
                            event,
                        );
                    }
                    FILE_ACTION_REMOVED => {
                        let event = WatcherEvent::Removed(target_path);
                        dispatch_event(
                            &volume_id,
                            &db,
                            &event_queue,
                            &buffering,
                            &counts,
                            &drive,
                            event,
                        );
                    }
                    FILE_ACTION_MODIFIED => {
                        let event = WatcherEvent::Modified(target_path);
                        dispatch_event(
                            &volume_id,
                            &db,
                            &event_queue,
                            &buffering,
                            &counts,
                            &drive,
                            event,
                        );
                    }
                    FILE_ACTION_RENAMED_OLD_NAME => {
                        pending_rename_old = Some(target_path);
                    }
                    FILE_ACTION_RENAMED_NEW_NAME => {
                        if let Some(old_path) = pending_rename_old.take() {
                            let event = WatcherEvent::Renamed {
                                from: old_path,
                                to: target_path,
                            };
                            dispatch_event(
                                &volume_id,
                                &db,
                                &event_queue,
                                &buffering,
                                &counts,
                                &drive,
                                event,
                            );
                        } else {
                            let event = WatcherEvent::Added(target_path);
                            dispatch_event(
                                &volume_id,
                                &db,
                                &event_queue,
                                &buffering,
                                &counts,
                                &drive,
                                event,
                            );
                        }
                    }
                    _ => {}
                }
            }

            if info.NextEntryOffset == 0 {
                break;
            }
            offset += info.NextEntryOffset as usize;
        }
    }

    let _ = unsafe { CloseHandle(dir_handle) };
}

#[allow(clippy::too_many_arguments)]
fn dispatch_event(
    volume_id: &str,
    db: &Database,
    queue: &Mutex<Vec<WatcherEvent>>,
    buffering: &AtomicBool,
    counts: &IndexCounts,
    drive: &str,
    event: WatcherEvent,
) {
    if buffering.load(Ordering::SeqCst) {
        queue.lock().unwrap().push(event);
    } else {
        apply_watcher_event(volume_id, db, counts, drive, event);
    }
}

fn apply_watcher_event(
    volume_id: &str,
    db: &Database,
    counts: &IndexCounts,
    drive: &str,
    event: WatcherEvent,
) {
    match event {
        WatcherEvent::Added(path) | WatcherEvent::Modified(path) => {
            if let Some(item) = inspect_single_path(&path) {
                let delta = db.add_or_update_file(volume_id, &item).unwrap_or(0);
                counts.adjust(drive, delta);
            }
        }
        WatcherEvent::Removed(path) => {
            let normalized = path.to_string_lossy().to_lowercase();
            // Assume directory true to clean up children as well
            let delta = db.remove_file(volume_id, &normalized, true).unwrap_or(0);
            counts.adjust(drive, -delta);
        }
        WatcherEvent::Renamed { from, to } => {
            let old_normalized = from.to_string_lossy().to_lowercase();
            if let Some(item) = inspect_single_path(&to) {
                let delta = db
                    .rename_file(volume_id, &old_normalized, &item)
                    .unwrap_or(0);
                counts.adjust(drive, delta);
            } else {
                let delta = db
                    .remove_file(volume_id, &old_normalized, true)
                    .unwrap_or(0);
                counts.adjust(drive, -delta);
            }
        }
        WatcherEvent::Overflow => {}
    }
}

fn inspect_single_path(path: &Path) -> Option<ScannedItem> {
    let metadata = std::fs::metadata(path).ok()?;
    let is_dir = metadata.is_dir();
    let display_path = path.to_string_lossy().into_owned();
    let normalized_path = display_path.to_lowercase();
    let filename = path.file_name()?.to_string_lossy().into_owned();
    let lower_name = filename.to_lowercase();
    let parent = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = if is_dir {
        None
    } else {
        path.extension().map(|e| e.to_string_lossy().to_lowercase())
    };
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|duration| {
            duration.as_secs() * 10_000_000
                + u64::from(duration.subsec_nanos()) / 100
                + 116_444_736_000_000_000
        })
        .unwrap_or(0);
    let size = if is_dir { 0 } else { metadata.len() };

    Some(ScannedItem {
        normalized_path,
        display_path,
        name: filename,
        lower_name,
        parent,
        is_directory: is_dir,
        extension,
        modified_at,
        size,
    })
}
