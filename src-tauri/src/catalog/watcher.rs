use std::collections::HashSet;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_ACTION_ADDED, FILE_ACTION_REMOVED,
    FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::CancelSynchronousIo;

use super::db::Database;
use super::types::ScannedItem;
use super::IndexCounts;

/// ReadDirectoryChangesW on a synchronous handle accepts at most 64 KB -
/// larger buffers fail instantly with an invalid-parameter error (which a
/// naive retry loop turns into a hot spin). Bursts beyond 64 KB surface as
/// overflow markers handled by scoped reconciliation instead.
const BUFFER_SIZE: usize = 64 * 1024;

/// How often queued events are folded and applied as one batched write.
/// Sub-second latency for search freshness, but a bounded number of
/// transactions per minute instead of one per filesystem event.
const FLUSH_INTERVAL: Duration = Duration::from_millis(750);

/// Backoff after a hard read error so a persistently failing handle can
/// never become a busy loop.
const READ_ERROR_BACKOFF: Duration = Duration::from_millis(250);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);

struct ReconnectBackoff {
    next: Duration,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            next: READ_ERROR_BACKOFF,
        }
    }
}

impl ReconnectBackoff {
    fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(MAX_RECONNECT_BACKOFF);
        delay
    }

    fn reset(&mut self) {
        self.next = READ_ERROR_BACKOFF;
    }
}

/// Bound userspace buffering independently of the kernel's 64 KB change
/// buffer. Once saturated, individual events are already incomplete, so keep
/// one overflow marker and let reconciliation restore the authoritative state.
const MAX_QUEUED_EVENTS: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub enum WatcherEvent {
    Added(PathBuf),
    Removed(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Overflow(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WatchShard {
    root: PathBuf,
    recursive: bool,
}

fn discover_watch_shards(root: &Path) -> Vec<WatchShard> {
    let mut shards = vec![WatchShard {
        root: root.to_path_buf(),
        recursive: false,
    }];
    let Ok(children) = std::fs::read_dir(root) else {
        return shards;
    };
    for child in children.flatten() {
        let Ok(file_type) = child.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let name = child.file_name().to_string_lossy().into_owned();
        if is_unsearchable_top_level(&name) {
            continue;
        }
        shards.push(WatchShard {
            root: child.path(),
            recursive: true,
        });
    }
    shards
}

struct WatcherEventQueue {
    events: Vec<WatcherEvent>,
    limit: usize,
    root: PathBuf,
    collapsed: bool,
}

impl WatcherEventQueue {
    fn new(root: PathBuf) -> Self {
        Self::with_limit(MAX_QUEUED_EVENTS, root)
    }

    fn with_limit(limit: usize, root: PathBuf) -> Self {
        assert!(limit > 0, "watcher queue limit must be positive");
        Self {
            events: Vec::new(),
            limit,
            root,
            collapsed: false,
        }
    }

    fn push(&mut self, event: WatcherEvent) {
        if let WatcherEvent::Overflow(scope) = &event {
            self.push_dirty_scope(scope.clone());
            return;
        }
        if self.collapsed {
            let scope = self.scope_for_event(&event);
            self.push_dirty_scope(scope);
            return;
        }
        if self.events.len() >= self.limit {
            let mut scopes: HashSet<PathBuf> = self
                .events
                .iter()
                .map(|queued| self.scope_for_event(queued))
                .collect();
            scopes.insert(self.scope_for_event(&event));
            self.events = scopes.into_iter().map(WatcherEvent::Overflow).collect();
            self.collapsed = true;
            return;
        }
        self.events.push(event);
    }

    fn push_dirty_scope(&mut self, scope: PathBuf) {
        if !self
            .events
            .iter()
            .any(|event| matches!(event, WatcherEvent::Overflow(existing) if existing == &scope))
        {
            self.events.push(WatcherEvent::Overflow(scope));
        }
    }

    fn scope_for_event(&self, event: &WatcherEvent) -> PathBuf {
        let path = match event {
            WatcherEvent::Added(path) | WatcherEvent::Removed(path) => path,
            WatcherEvent::Renamed { to, .. } => to,
            WatcherEvent::Overflow(scope) => return scope.clone(),
        };
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return self.root.clone();
        };
        let Some(first) = relative.components().next() else {
            return self.root.clone();
        };
        let top_level = self.root.join(first.as_os_str());
        if top_level.is_dir() {
            top_level
        } else {
            self.root.clone()
        }
    }

    fn take_all(&mut self) -> Vec<WatcherEvent> {
        self.collapsed = false;
        std::mem::take(&mut self.events)
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.events.clear();
        self.collapsed = false;
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.events.len()
    }

    #[cfg(test)]
    fn iter(&self) -> std::slice::Iter<'_, WatcherEvent> {
        self.events.iter()
    }
}

pub struct VolumeWatcher {
    #[allow(dead_code)]
    root: PathBuf,
    volume_id: String,
    #[allow(dead_code)]
    stop: Arc<AtomicBool>,
    #[allow(dead_code)]
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    shard_roots: Arc<Mutex<HashSet<PathBuf>>>,
    app_data_dir: PathBuf,
    event_queue: Arc<Mutex<WatcherEventQueue>>,
    /// Scan coordination: while a volume sweep runs, the periodic flusher
    /// defers and the sweep's own `flush_queue` drains the queue at the end.
    buffering: Arc<AtomicBool>,
    counts: IndexCounts,
    drive: String,
    on_updated: Arc<dyn Fn() + Send + Sync>,
    on_overflow: Arc<dyn Fn(String, PathBuf) + Send + Sync>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AppliedWatcherBatch {
    changed: bool,
    dirty_scopes: Vec<PathBuf>,
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
        on_updated: Arc<dyn Fn() + Send + Sync>,
        on_overflow: Arc<dyn Fn(String, PathBuf) + Send + Sync>,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let event_queue = Arc::new(Mutex::new(WatcherEventQueue::new(root.clone())));
        let handles = Arc::new(Mutex::new(Vec::new()));
        let shard_roots = Arc::new(Mutex::new(HashSet::new()));
        // Start deferred: a scan is about to run (the watcher is started just
        // before sweeps) and will flush the queue itself when it finishes.
        let buffering = Arc::new(AtomicBool::new(true));

        for (index, shard) in discover_watch_shards(&root).into_iter().enumerate() {
            let reader_handle = spawn_reader(
                format!("prism-watch-{volume_id}-{index}"),
                shard.root.clone(),
                shard.recursive,
                app_data_dir.clone(),
                stop.clone(),
                event_queue.clone(),
            )?;
            shard_roots.lock().unwrap().insert(shard.root);
            handles.lock().unwrap().push(reader_handle);
        }

        let flusher_handle = std::thread::Builder::new()
            .name(format!("prism-flush-{volume_id}"))
            .spawn({
                let db = db.clone();
                let stop = stop.clone();
                let queue = event_queue.clone();
                let buffering = buffering.clone();
                let counts = counts.clone();
                let drive = drive.clone();
                let on_updated = on_updated.clone();
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
                        on_updated,
                        on_overflow,
                    )
                }
            })
            .ok()?;
        handles.lock().unwrap().push(flusher_handle);

        Some(Self {
            root,
            volume_id,
            stop,
            handles,
            shard_roots,
            app_data_dir,
            event_queue,
            buffering,
            counts,
            drive,
            on_updated,
            on_overflow,
        })
    }

    pub fn set_buffering(&self, buffering: bool) {
        self.buffering.store(buffering, Ordering::SeqCst);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        for handle in self.handles.lock().unwrap().iter() {
            let thread = HANDLE(handle.as_raw_handle());
            let _ = unsafe { CancelSynchronousIo(thread) };
        }
    }

    pub fn shard_count(&self) -> usize {
        self.shard_roots
            .lock()
            .map(|roots| roots.len())
            .unwrap_or(0)
    }

    /// Attaches recursive readers for top-level directories that appeared
    /// after startup. Returned roots need a scoped baseline before they are
    /// considered converged.
    pub fn ensure_current_shards(&self) -> Vec<PathBuf> {
        let mut attached = Vec::new();
        for shard in discover_watch_shards(&self.root)
            .into_iter()
            .filter(|shard| shard.recursive)
        {
            let already_watched = self
                .shard_roots
                .lock()
                .map(|roots| {
                    roots
                        .iter()
                        .any(|root| !watcher_root_changed(root, &shard.root))
                })
                .unwrap_or(true);
            if already_watched {
                continue;
            }

            let index = self.shard_count();
            let Some(handle) = spawn_reader(
                format!("prism-watch-{}-{index}", self.volume_id),
                shard.root.clone(),
                true,
                self.app_data_dir.clone(),
                self.stop.clone(),
                self.event_queue.clone(),
            ) else {
                continue;
            };
            self.shard_roots.lock().unwrap().insert(shard.root.clone());
            self.handles.lock().unwrap().push(handle);
            attached.push(shard.root);
        }
        attached.sort_unstable();
        attached
    }

    /// Drains queued events and applies them as batched writes. Called by a
    /// finished volume sweep (which also releases the flush hold). An overflow
    /// observed anywhere in the drained batch is reported to the callback; the
    /// callback receives the exact shard roots that need reconciliation.
    pub fn flush_queue(&self, db: &Database) {
        self.buffering.store(false, Ordering::SeqCst);
        let applied = apply_queued_events(
            db,
            &self.event_queue,
            &self.volume_id,
            &self.counts,
            &self.drive,
        );
        if applied.changed {
            (self.on_updated)();
        }
        for scope in applied.dirty_scopes {
            (self.on_overflow)(self.volume_id.clone(), scope);
        }
    }

    #[allow(dead_code)]
    pub fn stop(self) {
        self.request_stop();
        let handles: Vec<JoinHandle<()>> = self
            .handles
            .lock()
            .map(|mut handles| handles.drain(..).collect())
            .unwrap_or_default();
        for handle in handles {
            let _ = handle.join();
        }
    }
}

fn spawn_reader(
    thread_name: String,
    root: PathBuf,
    recursive: bool,
    app_data_dir: PathBuf,
    stop: Arc<AtomicBool>,
    event_queue: Arc<Mutex<WatcherEventQueue>>,
) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || run_reader_loop(root, recursive, app_data_dir, stop, event_queue))
        .ok()
}

pub(crate) fn watcher_root_changed(current: &Path, next: &Path) -> bool {
    !current
        .as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&next.as_os_str().to_string_lossy())
}

/// The reader never applies events inline: it only pushes to the queue. That
/// keeps the time between `ReadDirectoryChangesW` calls nanosecond-scale, so
/// the kernel buffer rarely fills and bursts do not escalate into overflows.
fn run_reader_loop(
    root: PathBuf,
    recursive: bool,
    app_data_dir: PathBuf,
    stop: Arc<AtomicBool>,
    event_queue: Arc<Mutex<WatcherEventQueue>>,
) {
    let wide_root: Vec<u16> = root
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();

    let mut buffer = vec![0u8; BUFFER_SIZE];
    // Search membership changes only when a path is added, removed, or
    // renamed. Content, timestamp, and size writes do not affect filename
    // search and are far too noisy to mirror into the catalog.
    let notify_filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME;

    let app_data_normalized = app_data_dir.to_string_lossy().to_lowercase();
    let mut reconnect_backoff = ReconnectBackoff::default();

    while !stop.load(Ordering::Relaxed) {
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
            Ok(handle) => handle,
            Err(_) => {
                event_queue
                    .lock()
                    .unwrap()
                    .push(WatcherEvent::Overflow(root.clone()));
                sleep_with_stop(reconnect_backoff.next_delay(), &stop);
                continue;
            }
        };
        let mut pending_rename_old: Option<PathBuf> = None;

        while !stop.load(Ordering::Relaxed) {
            let mut bytes_returned = 0u32;
            let success = unsafe {
                ReadDirectoryChangesW(
                    dir_handle,
                    buffer.as_mut_ptr() as *mut _,
                    BUFFER_SIZE as u32,
                    recursive,
                    notify_filter,
                    Some(&mut bytes_returned),
                    None,
                    None,
                )
                .is_ok()
            };

            if stop.load(Ordering::Relaxed) {
                break;
            }

            if !success || bytes_returned == 0 {
                // The notification gap is scoped to this handle's shard. A
                // true buffer overflow can keep using the handle; a hard
                // error closes it and returns to the outer reopen loop.
                event_queue
                    .lock()
                    .unwrap()
                    .push(WatcherEvent::Overflow(root.clone()));
                if success {
                    reconnect_backoff.reset();
                    continue;
                }
                break;
            }
            reconnect_backoff.reset();

            let mut offset = 0;
            loop {
                if offset + std::mem::size_of::<FILE_NOTIFY_INFORMATION>() > bytes_returned as usize
                {
                    break;
                }

                let info =
                    unsafe { &*(buffer.as_ptr().add(offset) as *const FILE_NOTIFY_INFORMATION) };
                let name_len = (info.FileNameLength / 2) as usize;
                let name_slice = unsafe {
                    std::slice::from_raw_parts(
                        buffer.as_ptr().add(offset + 12) as *const u16,
                        name_len,
                    )
                };
                let relative_name = OsString::from_wide(name_slice)
                    .to_string_lossy()
                    .into_owned();
                let target_path = root.join(&relative_name);
                let target_normalized = target_path.to_string_lossy().to_lowercase();

                // Exclude Prism's own app data directory and the unsearchable
                // roots the scanner skips.
                if !target_normalized.starts_with(&app_data_normalized)
                    && !is_unsearchable_top_level(&relative_name)
                {
                    let event = match info.Action {
                        FILE_ACTION_ADDED => Some(WatcherEvent::Added(target_path)),
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
        if !stop.load(Ordering::Relaxed) {
            sleep_with_stop(reconnect_backoff.next_delay(), &stop);
        }
    }
}

fn sleep_with_stop(delay: Duration, stop: &AtomicBool) {
    let deadline = std::time::Instant::now() + delay;
    while !stop.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(READ_ERROR_BACKOFF));
    }
}

/// Folds queued events to a final state per path and applies them as a
/// handful of batched writes.
fn apply_queued_events(
    db: &Database,
    event_queue: &Mutex<WatcherEventQueue>,
    volume_id: &str,
    counts: &IndexCounts,
    drive: &str,
) -> AppliedWatcherBatch {
    let events: Vec<WatcherEvent> = {
        let mut q = event_queue.lock().unwrap();
        q.take_all()
    };

    let mut dirty_scopes: HashSet<PathBuf> = HashSet::new();
    let mut renames: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut touched: HashSet<PathBuf> = HashSet::new();
    let mut removed: HashSet<PathBuf> = HashSet::new();

    for event in events {
        match event {
            WatcherEvent::Overflow(scope) => {
                dirty_scopes.insert(scope);
            }
            WatcherEvent::Renamed { from, to } => renames.push((from, to)),
            WatcherEvent::Removed(path) => {
                touched.remove(&path);
                removed.insert(path);
            }
            WatcherEvent::Added(path) => {
                removed.remove(&path);
                touched.insert(path);
            }
        }
    }

    let mut delta = 0i64;
    let mut changed = false;

    for (from, to) in &renames {
        let old_normalized = from.to_string_lossy().to_lowercase();
        if let Some(item) = inspect_single_path(to) {
            if let Ok(rename_delta) = db.rename_file(volume_id, &old_normalized, &item) {
                delta += rename_delta;
                changed = true;
            }
        } else {
            // Target vanished before it could be inspected; treat as removal.
            if let Ok(removed) = db.remove_file(volume_id, &old_normalized, true) {
                delta -= removed;
                changed |= removed > 0;
            }
        }
    }

    let items: Vec<ScannedItem> = touched
        .iter()
        .filter_map(|path| inspect_single_path(path))
        .collect();
    for item in items.iter().filter(|item| item.is_directory) {
        dirty_scopes.insert(PathBuf::from(&item.display_path));
    }
    if let Ok(inserted) = db.insert_batch(volume_id, 0, &items) {
        delta += inserted as i64;
        changed |= inserted > 0;
    }

    for path in &removed {
        let normalized = path.to_string_lossy().to_lowercase();
        if let Ok(removed) = db.remove_file(volume_id, &normalized, true) {
            delta -= removed;
            changed |= removed > 0;
        }
    }

    counts.adjust(drive, delta);
    let mut dirty_scopes: Vec<PathBuf> = dirty_scopes.into_iter().collect();
    dirty_scopes.sort_unstable();
    AppliedWatcherBatch {
        changed,
        dirty_scopes,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_flusher_loop(
    db: Arc<Database>,
    stop: Arc<AtomicBool>,
    event_queue: Arc<Mutex<WatcherEventQueue>>,
    buffering: Arc<AtomicBool>,
    volume_id: String,
    counts: IndexCounts,
    drive: String,
    on_updated: Arc<dyn Fn() + Send + Sync>,
    on_overflow: Arc<dyn Fn(String, PathBuf) + Send + Sync>,
) {
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(FLUSH_INTERVAL);
        if buffering.load(Ordering::SeqCst) {
            continue;
        }
        let applied = apply_queued_events(&db, &event_queue, &volume_id, &counts, &drive);
        if applied.changed {
            on_updated();
        }
        for scope in applied.dirty_scopes {
            on_overflow(volume_id.clone(), scope);
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
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::sync::atomic::AtomicUsize;
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
    fn renaming_a_unicode_directory_preserves_searchable_descendant_paths() {
        let (db, dir) = temp_db("unicode-rename");
        let original = dir.join("資料");
        let renamed = dir.join("מסמכים");
        std::fs::create_dir_all(original.join("nested")).unwrap();
        std::fs::write(original.join("nested").join("report.txt"), "report").unwrap();
        let items: Vec<_> = [
            original.clone(),
            original.join("nested"),
            original.join("nested").join("report.txt"),
        ]
        .iter()
        .filter_map(|path| inspect_single_path(path))
        .collect();
        db.insert_batch("vol1", 1, &items).unwrap();
        std::fs::rename(&original, &renamed).unwrap();
        let queue = Mutex::new(WatcherEventQueue::with_limit(8, dir.clone()));
        queue.lock().unwrap().push(WatcherEvent::Renamed {
            from: original,
            to: renamed.clone(),
        });
        let applied = apply_queued_events(&db, &queue, "vol1", &IndexCounts::default(), r"C:\");
        assert!(applied.changed);
        let found = db.search_candidates("report", 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].display_path,
            renamed.join("nested").join("report.txt").to_string_lossy()
        );
        assert!(Path::new(&found[0].display_path).exists());
    }

    #[test]
    fn queued_events_fold_to_a_final_state_and_report_overflow() {
        let (db, dir) = temp_db("fold");
        std::fs::write(dir.join("kept.txt"), "kept").unwrap();
        std::fs::write(dir.join("gone.txt"), "gone").unwrap();
        std::fs::write(dir.join("moved-new.txt"), "moved").unwrap();

        let counts = IndexCounts::default();
        let queue = Mutex::new(WatcherEventQueue::with_limit(32, dir.clone()));
        {
            let mut q = queue.lock().unwrap();
            q.push(WatcherEvent::Added(dir.join("kept.txt")));
            q.push(WatcherEvent::Added(dir.join("kept.txt")));
            // Remove-then-add of the same path folds to a live upsert.
            q.push(WatcherEvent::Removed(dir.join("gone.txt")));
            q.push(WatcherEvent::Added(dir.join("gone.txt")));
            q.push(WatcherEvent::Renamed {
                from: dir.join("moved-old.txt"),
                to: dir.join("moved-new.txt"),
            });
            q.push(WatcherEvent::Overflow(dir.clone()));
        }

        let applied = apply_queued_events(&db, &queue, "vol1", &counts, "C:\\");
        assert!(applied.changed);
        assert_eq!(applied.dirty_scopes, vec![dir.clone()]);
        assert_eq!(counts.total(), 3, "kept + resurrected + renamed rows");
        assert_eq!(db.get_total_indexed_count().unwrap(), 3);

        // A second drain with nothing queued changes nothing and stays calm.
        let applied = apply_queued_events(&db, &queue, "vol1", &counts, "C:\\");
        assert!(!applied.changed);
        assert!(applied.dirty_scopes.is_empty());
        assert_eq!(counts.total(), 3);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn event_queue_collapses_overload_to_one_overflow_marker() {
        let mut queue = WatcherEventQueue::with_limit(2, PathBuf::from(r"C:\"));
        queue.push(WatcherEvent::Added(PathBuf::from(r"C:\one.txt")));
        queue.push(WatcherEvent::Added(PathBuf::from(r"C:\two.txt")));
        queue.push(WatcherEvent::Added(PathBuf::from(r"C:\three.txt")));
        queue.push(WatcherEvent::Added(PathBuf::from(r"C:\four.txt")));

        assert_eq!(queue.len(), 1);
        assert!(matches!(
            queue.iter().next(),
            Some(WatcherEvent::Overflow(_))
        ));

        let drained = queue.take_all();
        assert_eq!(drained.len(), 1);
        queue.push(WatcherEvent::Added(PathBuf::from(r"C:\after.txt")));
        assert_eq!(queue.len(), 1, "draining reopens the bounded queue");
    }

    #[test]
    fn reconnect_backoff_is_capped_and_resets_after_success() {
        let mut backoff = ReconnectBackoff::default();
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
        for _ in 0..20 {
            backoff.next_delay();
        }
        assert_eq!(backoff.next_delay(), Duration::from_secs(30));
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));
    }

    #[test]
    fn overflow_reports_only_its_shard_for_repair() {
        let (db, dir) = temp_db("overflow-scope");
        let shard = dir.join("Users");
        let queue = Mutex::new(WatcherEventQueue::with_limit(8, dir.clone()));
        queue
            .lock()
            .unwrap()
            .push(WatcherEvent::Overflow(shard.clone()));

        let applied = apply_queued_events(&db, &queue, "vol1", &IndexCounts::default(), r"C:\");

        assert_eq!(applied.dirty_scopes, vec![shard]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adding_a_directory_requests_a_scoped_repair() {
        let (db, dir) = temp_db("directory-add");
        let added = dir.join("already-populated");
        std::fs::create_dir_all(&added).unwrap();
        std::fs::write(added.join("child.txt"), "child").unwrap();
        let queue = Mutex::new(WatcherEventQueue::with_limit(8, dir.clone()));
        queue
            .lock()
            .unwrap()
            .push(WatcherEvent::Added(added.clone()));

        let applied = apply_queued_events(&db, &queue, "vol1", &IndexCounts::default(), r"C:\");

        assert_eq!(applied.dirty_scopes, vec![added]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn canonical_root_change_requires_watcher_replacement() {
        assert!(!watcher_root_changed(Path::new(r"D:\"), Path::new(r"d:\")));
        assert!(watcher_root_changed(Path::new(r"D:\"), Path::new(r"E:\")));
    }

    #[test]
    fn volume_watch_is_split_into_topology_root_and_recursive_children() {
        let dir = temp_db("shards").1;
        let users = dir.join("Users");
        let windows = dir.join("Windows");
        std::fs::create_dir_all(&users).unwrap();
        std::fs::create_dir_all(&windows).unwrap();
        std::fs::write(dir.join("root-file.txt"), "root").unwrap();

        let shards = discover_watch_shards(&dir);
        assert!(shards
            .iter()
            .any(|shard| shard.root == dir && !shard.recursive));
        assert!(shards
            .iter()
            .any(|shard| shard.root == users && shard.recursive));
        assert!(shards
            .iter()
            .any(|shard| shard.root == windows && shard.recursive));
        assert_eq!(shards.len(), 3, "files are covered by the root watcher");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn watcher_starts_one_reader_per_discovered_shard() {
        let (db, dir) = temp_db("shard-readers");
        std::fs::create_dir_all(dir.join("Users")).unwrap();
        std::fs::create_dir_all(dir.join("Windows")).unwrap();

        let watcher = VolumeWatcher::start(
            dir.clone(),
            "vol1".into(),
            Arc::new(db),
            dir.join("prism-data"),
            IndexCounts::default(),
            r"C:\".into(),
            Arc::new(|| {}),
            Arc::new(|_, _| {}),
        )
        .expect("watcher should start");

        assert_eq!(watcher.shard_count(), 3);
        watcher.stop();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn watcher_attaches_a_new_top_level_shard_without_restart() {
        let (db, dir) = temp_db("dynamic-shard");
        let watcher = VolumeWatcher::start(
            dir.clone(),
            "vol1".into(),
            Arc::new(db),
            dir.join("prism-data"),
            IndexCounts::default(),
            r"C:\".into(),
            Arc::new(|| {}),
            Arc::new(|_, _| {}),
        )
        .expect("watcher should start");
        assert_eq!(watcher.shard_count(), 1);

        let added = dir.join("new-root");
        std::fs::create_dir_all(&added).unwrap();
        let attached = watcher.ensure_current_shards();

        assert_eq!(attached, vec![added]);
        assert_eq!(watcher.shard_count(), 2);
        watcher.stop();
        let _ = std::fs::remove_dir_all(dir);
    }

    fn event_targets(event: &WatcherEvent, path: &Path) -> bool {
        match event {
            WatcherEvent::Added(target) | WatcherEvent::Removed(target) => target == path,
            WatcherEvent::Renamed { from, to } => from == path || to == path,
            WatcherEvent::Overflow(_) => false,
        }
    }

    fn wait_for_event(watcher: &VolumeWatcher, path: &Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if watcher
                .event_queue
                .lock()
                .unwrap()
                .iter()
                .any(|event| event_targets(event, path))
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("watcher did not report {}", path.display());
    }

    fn establish_watcher_ready(watcher: &VolumeWatcher, dir: &Path) {
        for attempt in 0..20 {
            let marker = dir.join(format!("ready-{attempt}.txt"));
            std::fs::write(&marker, "ready").unwrap();
            let deadline = std::time::Instant::now() + Duration::from_millis(100);
            while std::time::Instant::now() < deadline {
                if watcher
                    .event_queue
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event_targets(event, &marker))
                {
                    watcher.event_queue.lock().unwrap().clear();
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        panic!("watcher did not become ready");
    }

    #[test]
    fn content_writes_do_not_enqueue_catalog_updates() {
        let (db, dir) = temp_db("content-write");
        let existing = dir.join("existing.txt");
        std::fs::write(&existing, "before").unwrap();

        let watcher = VolumeWatcher::start(
            dir.clone(),
            "vol1".into(),
            Arc::new(db),
            dir.join("prism-data"),
            IndexCounts::default(),
            r"C:\".into(),
            Arc::new(|| {}),
            Arc::new(|_, _| {}),
        )
        .expect("watcher should start");

        // Establish that ReadDirectoryChangesW is waiting before changing the
        // existing file, then use a second name event as an ordering barrier.
        establish_watcher_ready(&watcher, &dir);

        OpenOptions::new()
            .append(true)
            .open(&existing)
            .unwrap()
            .write_all(b" after")
            .unwrap();
        let barrier = dir.join("barrier.txt");
        std::fs::write(&barrier, "barrier").unwrap();
        wait_for_event(&watcher, &barrier);

        let events = watcher.event_queue.lock().unwrap();
        assert!(
            !events.iter().any(|event| event_targets(event, &existing)),
            "content-only writes do not change a filename index"
        );
        drop(events);

        watcher.stop();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn filename_addition_updates_catalog_and_notifies_search() {
        let (db, dir) = temp_db("live-add");
        let documents = dir.join("Users").join("test-user").join("Documents");
        std::fs::create_dir_all(&documents).unwrap();
        let db = Arc::new(db);
        let updates = Arc::new(AtomicUsize::new(0));
        let update_counter = updates.clone();
        let watcher = VolumeWatcher::start(
            dir.clone(),
            "vol1".into(),
            db.clone(),
            dir.join("prism-data"),
            IndexCounts::default(),
            r"C:\".into(),
            Arc::new(move || {
                update_counter.fetch_add(1, Ordering::SeqCst);
            }),
            Arc::new(|_, _| {}),
        )
        .expect("watcher should start");

        establish_watcher_ready(&watcher, &documents);
        watcher.set_buffering(false);
        let added = documents.join("new searchable file.txt");
        std::fs::write(&added, "new").unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if updates.load(Ordering::SeqCst) > 0
                && db
                    .search_candidates("new searchable file", 20)
                    .unwrap()
                    .iter()
                    .any(|candidate| candidate.display_path == added.to_string_lossy())
            {
                watcher.stop();
                let _ = std::fs::remove_dir_all(dir);
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        watcher.stop();
        let _ = std::fs::remove_dir_all(dir);
        panic!("filename addition did not publish a searchable catalog update");
    }
}
