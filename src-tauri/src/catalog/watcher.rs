use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

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

const BUFFER_SIZE: usize = 64 * 1024; // 64 KB buffer

#[derive(Clone, Debug)]
pub enum WatcherEvent {
    Added(PathBuf),
    Removed(PathBuf),
    Modified(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Overflow,
}

pub struct VolumeWatcher {
    root: PathBuf,
    volume_id: String,
    #[allow(dead_code)]
    stop: Arc<AtomicBool>,
    #[allow(dead_code)]
    handle: Option<JoinHandle<()>>,
    event_queue: Arc<Mutex<Vec<WatcherEvent>>>,
    buffering: Arc<AtomicBool>,
}

impl VolumeWatcher {
    pub fn start(
        root: PathBuf,
        volume_id: String,
        db: Arc<Database>,
        app_data_dir: PathBuf,
        on_overflow: impl Fn(String) + Send + Sync + 'static,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let event_queue = Arc::new(Mutex::new(Vec::new()));
        let buffering = Arc::new(AtomicBool::new(true));

        let stop_clone = stop.clone();
        let queue_clone = event_queue.clone();
        let buffering_clone = buffering.clone();
        let root_clone = root.clone();
        let vol_id_clone = volume_id.clone();

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
                    on_overflow,
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
        })
    }

    pub fn set_buffering(&self, buffering: bool) {
        self.buffering.store(buffering, Ordering::SeqCst);
    }

    pub fn flush_queue(&self, db: &Database) {
        self.buffering.store(false, Ordering::SeqCst);
        let events: Vec<WatcherEvent> = {
            let mut q = self.event_queue.lock().unwrap();
            q.drain(..).collect()
        };

        for event in events {
            apply_watcher_event(&self.volume_id, &self.root, db, event);
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
    on_overflow: impl Fn(String) + Send + Sync + 'static,
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
                        dispatch_event(&volume_id, &root, &db, &event_queue, &buffering, event);
                    }
                    FILE_ACTION_REMOVED => {
                        let event = WatcherEvent::Removed(target_path);
                        dispatch_event(&volume_id, &root, &db, &event_queue, &buffering, event);
                    }
                    FILE_ACTION_MODIFIED => {
                        let event = WatcherEvent::Modified(target_path);
                        dispatch_event(&volume_id, &root, &db, &event_queue, &buffering, event);
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
                            dispatch_event(&volume_id, &root, &db, &event_queue, &buffering, event);
                        } else {
                            let event = WatcherEvent::Added(target_path);
                            dispatch_event(&volume_id, &root, &db, &event_queue, &buffering, event);
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

fn dispatch_event(
    volume_id: &str,
    root: &Path,
    db: &Database,
    queue: &Mutex<Vec<WatcherEvent>>,
    buffering: &AtomicBool,
    event: WatcherEvent,
) {
    if buffering.load(Ordering::SeqCst) {
        queue.lock().unwrap().push(event);
    } else {
        apply_watcher_event(volume_id, root, db, event);
    }
}

fn apply_watcher_event(volume_id: &str, _root: &Path, db: &Database, event: WatcherEvent) {
    match event {
        WatcherEvent::Added(path) | WatcherEvent::Modified(path) => {
            if let Some(item) = inspect_single_path(&path) {
                let _ = db.add_or_update_file(volume_id, &item);
            }
        }
        WatcherEvent::Removed(path) => {
            let normalized = path.to_string_lossy().to_lowercase();
            // Assume directory true to clean up children as well
            let _ = db.remove_file(volume_id, &normalized, true);
        }
        WatcherEvent::Renamed { from, to } => {
            let old_normalized = from.to_string_lossy().to_lowercase();
            if let Some(item) = inspect_single_path(&to) {
                let _ = db.rename_file(volume_id, &old_normalized, &item);
            } else {
                let _ = db.remove_file(volume_id, &old_normalized, true);
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

    Some(ScannedItem {
        normalized_path,
        display_path,
        name: filename,
        lower_name,
        parent,
        is_directory: is_dir,
        extension,
        modified_at: 0,
        size: metadata.len(),
    })
}
