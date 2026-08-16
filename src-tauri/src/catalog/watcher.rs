use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
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

/// ReadDirectoryChangesW on a synchronous handle accepts at most 64 KB -
/// larger buffers fail instantly with an invalid-parameter error (which a
/// naive retry loop turns into a hot spin). Bursts beyond 64 KB surface as
/// overflow markers handled by the rate-limited reconcile instead.
const BUFFER_SIZE: usize = 64 * 1024;

/// How often queued events are folded and applied as one batched write.
/// Sub-second latency for search freshness, but a bounded number of
/// transactions per minute instead of one per filesystem event.
const FLUSH_INTERVAL: Duration = Duration::from_millis(750);

/// Backoff after a hard read error so a persistently failing handle can
/// never become a busy loop.
const READ_ERROR_BACKOFF: Duration = Duration::from_millis(250);

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
    /// Scan coordination: while a volume sweep runs, the periodic flusher
    /// defers and the sweep's own `flush_queue` drains the queue at the end.
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
        // Start deferred: a scan is about to run (the watcher is started just
        // before sweeps) and will flush the queue itself when it finishes.
        let buffering = Arc::new(AtomicBool::new(true));

        let reader_handle = std::thread::Builder::new()
            .name(format!("prism-watch-{volume_id}"))
            .spawn({
                let root = root.clone();
                let stop = stop.clone();
                let queue = event_queue.clone();
                move || run_reader_loop(root, app_data_dir, stop, queue)
            })
            .ok()?;

        std::thread::Builder::new()
            .name(format!("prism-flush-{volume_id}"))
            .spawn({
                let db = db.clone();
                let stop = stop.clone();
                let queue = event_queue.clone();
                let buffering = buffering.clone();
                let counts = counts.clone();
                let drive = drive.clone();
                let on_overflow = on_overflow.clone();
                let volume_id = volume_id.clone();
                move || {
                    run_flusher_loop(
                        db,
                        stop,
                        queue,
                        buffering,
                        volume_id,
                        counts,
                        drive,
                        on_overflow,
                    )
                }
            })
            .ok()?;

        Some(Self {
            root,
            volume_id,
            stop,
            handle: Some(reader_handle),
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

    /// Drains queued events and applies them as batched writes. Called by a
    /// finished volume sweep (which also releases the flush hold). An overflow
    /// observed anywhere in the drained batch is reported to the callback; the
    /// callback's reconcile decision is rate-limited and scan-guarded.
    pub fn flush_queue(&self, db: &Database) {
        self.buffering.store(false, Ordering::SeqCst);
        let overflow = apply_queued_events(
            db,
            &self.event_queue,
            &self.volume_id,
            &self.counts,
            &self.drive,
        );
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

/// The reader never applies events inline: it only pushes to the queue. That
/// keeps the time between `ReadDirectoryChangesW` calls nanosecond-scale, so
/// the kernel buffer rarely fills and bursts do not escalate into overflows.
fn run_reader_loop(
    root: PathBuf,
    app_data_dir: PathBuf,
    stop: Arc<AtomicBool>,
    event_queue: Arc<Mutex<Vec<WatcherEvent>>>,
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
            // Buffer overflow or communication lost: events between the last
            // read and now are gone. Queue a marker and keep reading - the
            // handle stays valid and the flusher's rate-limited reconcile
            // decides whether a re-walk is warranted. Never exit the loop:
            // a dead reader silently took the volume off live updates before.
            if !stop.load(Ordering::Relaxed) {
                event_queue.lock().unwrap().push(WatcherEvent::Overflow);
            }
            // A hard error returns immediately; back off so a failing
            // handle cannot become a busy loop. A true overflow
            // (bytes_returned == 0 with a successful call) blocks normally
            // on the next read and needs no sleep.
            if !success {
                std::thread::sleep(READ_ERROR_BACKOFF);
            }
            continue;
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

            // Exclude Prism's own app data directory and the unsearchable
            // roots the scanner skips (recycle-bin hash churn must not leak
            // rows into the index between sweeps).
            if !target_normalized.starts_with(&app_data_normalized)
                && !is_unsearchable_top_level(&relative_name)
            {
                let event = match info.Action {
                    FILE_ACTION_ADDED | FILE_ACTION_MODIFIED => {
                        Some(WatcherEvent::Modified(target_path))
                    }
                    FILE_ACTION_REMOVED => Some(WatcherEvent::Removed(target_path)),
                    FILE_ACTION_RENAMED_OLD_NAME => {
                        pending_rename_old = Some(target_path);
                        None
                    }
                    FILE_ACTION_RENAMED_NEW_NAME => match pending_rename_old.take() {
                        Some(old_path) => Some(WatcherEvent::Renamed {
                            from: old_path,
                            to: target_path,
                        }),
                        // An unpaired new name is a plain addition.
                        None => Some(WatcherEvent::Added(target_path)),
                    },
                    _ => None,
                };
                if let Some(event) = event {
                    event_queue.lock().unwrap().push(event);
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

/// Folds queued events to a final state per path and applies them as a
/// handful of batched writes. Returns whether an overflow marker was seen.
fn apply_queued_events(
    db: &Database,
    event_queue: &Mutex<Vec<WatcherEvent>>,
    volume_id: &str,
    counts: &IndexCounts,
    drive: &str,
) -> bool {
    let events: Vec<WatcherEvent> = {
        let mut q = event_queue.lock().unwrap();
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
                .rename_file(volume_id, &old_normalized, &item)
                .unwrap_or(0);
        } else {
            // Target vanished before it could be inspected; treat as removal.
            delta -= db
                .remove_file(volume_id, &old_normalized, true)
                .unwrap_or(0);
        }
    }

    let items: Vec<ScannedItem> = touched
        .iter()
        .filter_map(|path| inspect_single_path(path))
        .collect();
    delta += db.insert_batch(volume_id, 0, &items).unwrap_or(0) as i64;

    for path in &removed {
        let normalized = path.to_string_lossy().to_lowercase();
        delta -= db.remove_file(volume_id, &normalized, true).unwrap_or(0);
    }

    counts.adjust(drive, delta);
    overflow
}

#[allow(clippy::too_many_arguments)]
fn run_flusher_loop(
    db: Arc<Database>,
    stop: Arc<AtomicBool>,
    event_queue: Arc<Mutex<Vec<WatcherEvent>>>,
    buffering: Arc<AtomicBool>,
    volume_id: String,
    counts: IndexCounts,
    drive: String,
    on_overflow: Arc<dyn Fn(String) + Send + Sync>,
) {
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(FLUSH_INTERVAL);
        if buffering.load(Ordering::SeqCst) {
            continue;
        }
        let overflow = apply_queued_events(&db, &event_queue, &volume_id, &counts, &drive);
        if overflow {
            on_overflow(volume_id.clone());
        }
    }
}

/// True when the event's path starts under a root the scanner excludes
/// (`$RECYCLE.BIN`, System Volume Information). The watcher root is a volume
/// root, so checking the first path component matches the scanner's rule.
fn is_unsearchable_top_level(relative_name: &str) -> bool {
    let first = relative_name
        .split(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(first.as_str(), "$recycle.bin" | "system volume information")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::db::Database;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db(name: &str) -> (Database, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("prism-watch-{name}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("catalog.db")).unwrap();
        (db, dir)
    }

    #[test]
    fn queued_events_fold_to_a_final_state_and_report_overflow() {
        let (db, dir) = temp_db("fold");
        std::fs::write(dir.join("kept.txt"), "kept").unwrap();
        std::fs::write(dir.join("gone.txt"), "gone").unwrap();
        std::fs::write(dir.join("moved-new.txt"), "moved").unwrap();

        let counts = IndexCounts::default();
        let queue = Mutex::new(Vec::new());
        {
            let mut q = queue.lock().unwrap();
            q.push(WatcherEvent::Modified(dir.join("kept.txt")));
            q.push(WatcherEvent::Modified(dir.join("kept.txt")));
            // Remove-then-add of the same path folds to a live upsert.
            q.push(WatcherEvent::Removed(dir.join("gone.txt")));
            q.push(WatcherEvent::Added(dir.join("gone.txt")));
            q.push(WatcherEvent::Renamed {
                from: dir.join("moved-old.txt"),
                to: dir.join("moved-new.txt"),
            });
            q.push(WatcherEvent::Overflow);
        }

        let overflow = apply_queued_events(&db, &queue, "vol1", &counts, "C:\\");
        assert!(overflow, "the overflow marker must reach the caller");
        assert_eq!(counts.total(), 3, "kept + resurrected + renamed rows");
        assert_eq!(db.get_total_indexed_count().unwrap(), 3);

        // A second drain with nothing queued changes nothing and stays calm.
        let overflow = apply_queued_events(&db, &queue, "vol1", &counts, "C:\\");
        assert!(!overflow);
        assert_eq!(counts.total(), 3);

        let _ = std::fs::remove_dir_all(dir);
    }
}
