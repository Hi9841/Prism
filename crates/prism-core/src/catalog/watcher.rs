use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
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

use super::db::{Database, FlushRename, FlushWrites};
use super::types::ScannedItem;
use super::IndexCounts;

/// The kernel keeps a change buffer of this size per watch handle between
/// reads. With 64 KB, deleting a build folder (tens of thousands of names in a
/// second) overflowed it, and an overflow re-walks the whole shard (all of
/// C:\Users). The 64 KB limit applies only to network shares; the watched
/// roots are local volumes. A failed read still backs off (no hot spin).
const BUFFER_SIZE: usize = 512 * 1024;

/// How often queued events are folded and applied as one batched write.
/// Sub-second latency for search freshness, but a bounded number of
/// transactions per minute instead of one per filesystem event.
const FLUSH_INTERVAL: Duration = Duration::from_millis(750);

/// While the palette is hidden, queued events wait until the oldest is this
/// old. An entry that is added and removed in that time (a build's temp
/// files, a test's sandbox) then costs the catalog no write: the fold drops
/// it. On a developer's machine a fifth of all new entries live under 5 s.
/// Nobody reads the catalog while the palette is hidden; when it shows, the
/// next tick flushes.
const HIDDEN_FLUSH_AGE: Duration = Duration::from_secs(10);
/// A hidden flush does not wait past this many queued events: the queue
/// collapses into repair walks at `MAX_QUEUED_EVENTS`, and a burst must
/// still fit on top.
const HIDDEN_FLUSH_EVENTS: usize = MAX_QUEUED_EVENTS / 4;

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

/// Old names of renames that can wait for their new name in one reader.
const MAX_PENDING_RENAMES: usize = 32;

/// Bound userspace buffering independently of the kernel's 64 KB change
/// buffer. Once saturated, individual events are already incomplete, so keep
/// one overflow marker and let reconciliation restore the authoritative state.
const MAX_QUEUED_EVENTS: usize = 16 * 1024;

/// A collapsed queue keeps at most this many dirty scopes. Each scope is the
/// narrowest existing directory that covers the dropped events (a build's
/// output folder, not all of C:\Users); past the limit, the deepest scopes
/// merge into their parents.
const MAX_DIRTY_SCOPES: usize = 32;

/// A folder that holds this many events of one flush is under heavy churn (a
/// test suite's sandboxes, a build's output). Its events then cost the
/// catalog nothing until it is quiet: one repair walk settles the folder.
/// A per-event write costs more than the share of a walk that one event
/// causes, and the churn makes most of those rows short-lived. 512 events in
/// one flush are 700 a second at the visible tick; a normal save, download,
/// or small unpack stays far below that.
const HOT_SCOPE_EVENTS: usize = 512;

/// A deferred scope waits for this long without an event before its repair
/// walk. Visible: the user can search now, and a build rarely pauses for
/// 5 s inside one folder (a walk in a pause costs once; the folder defers
/// again if the churn goes on). Hidden: nobody reads the catalog, so wait
/// for a real end of the churn (a test suite pauses between its binaries);
/// 30 s is also the quiet time of a repair at a shard root.
const DEFERRED_QUIET_VISIBLE: Duration = Duration::from_secs(5);
const DEFERRED_QUIET_HIDDEN: Duration = Duration::from_secs(30);

/// A scope that never gets quiet (a log folder that always grows) still
/// repairs after this long, so the catalog is never stale for hours. It
/// defers again at the next hot flush. The same limit as a repair at a shard
/// root.
const DEFERRED_MAX_AGE: Duration = Duration::from_secs(5 * 60);

/// Deferred scopes at one time. Past the limit a hot folder is written per
/// event as before: no scope is ever dropped.
const MAX_DEFERRED_SCOPES: usize = 32;

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
    /// `existing_parent` results while collapsed: a burst repeats the same
    /// few parents, and each lookup is a file system call under the lock.
    parents: HashMap<PathBuf, PathBuf>,
    /// Folders under heavy churn. What happens below them never enters the
    /// queue; a repair walk settles them once they are quiet.
    deferred: Vec<DeferredScope>,
    /// Changes of the deferred set that the catalog does not have yet:
    /// normalized path -> (path, recorded). The flusher writes them in one
    /// transaction, so a write follows a change of the set, never an event.
    unrecorded: HashMap<String, (PathBuf, bool)>,
}

struct DeferredScope {
    path: PathBuf,
    since: Instant,
    last_event: Instant,
}

/// Parents remembered while collapsed, before the memo starts over.
const MAX_REMEMBERED_PARENTS: usize = 4096;

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
            parents: HashMap::new(),
            deferred: Vec::new(),
            unrecorded: HashMap::new(),
        }
    }

    fn push(&mut self, event: WatcherEvent) {
        if let WatcherEvent::Overflow(scope) = &event {
            self.push_dirty_scope(scope.clone());
            return;
        }
        let Some(event) = self.apply_deferral(event, Instant::now()) else {
            return;
        };
        if self.collapsed {
            for scope in self.scopes_for_event(&event) {
                self.push_dirty_scope(scope);
            }
            return;
        }
        if self.events.len() >= self.limit {
            let mut scopes: Vec<PathBuf> = Vec::new();
            for queued in std::mem::take(&mut self.events).iter().chain(Some(&event)) {
                scopes.extend(self.scopes_for_event(queued));
            }
            self.events.clear();
            self.collapsed = true;
            for scope in scopes {
                self.push_dirty_scope(scope);
            }
            return;
        }
        self.events.push(event);
    }

    /// Adds a dirty scope unless an existing one covers it; drops the scopes
    /// it covers; merges the deepest scopes past MAX_DIRTY_SCOPES.
    fn push_dirty_scope(&mut self, scope: PathBuf) {
        let covered = self.events.iter().any(
            |event| matches!(event, WatcherEvent::Overflow(existing) if scope.starts_with(existing)),
        );
        if covered {
            return;
        }
        self.events.retain(
            |event| !matches!(event, WatcherEvent::Overflow(existing) if existing.starts_with(&scope)),
        );
        self.events.push(WatcherEvent::Overflow(scope));
        while self.dirty_scope_count() > MAX_DIRTY_SCOPES {
            let Some(deepest) = self
                .events
                .iter()
                .filter_map(|event| match event {
                    WatcherEvent::Overflow(scope) => Some(scope.clone()),
                    _ => None,
                })
                .filter(|scope| scope != &self.root)
                .max_by_key(|scope| scope.components().count())
            else {
                break;
            };
            let parent = deepest
                .parent()
                .filter(|parent| parent.starts_with(&self.root))
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone());
            self.events.retain(
                |event| !matches!(event, WatcherEvent::Overflow(existing) if existing.starts_with(&parent)),
            );
            self.events.push(WatcherEvent::Overflow(parent));
        }
    }

    fn dirty_scope_count(&self) -> usize {
        self.events.iter().filter(|event| matches!(event, WatcherEvent::Overflow(_))).count()
    }

    /// The narrowest existing directories whose reconciliation covers `event`:
    /// the parent of each changed path (both sides of a rename).
    fn scopes_for_event(&mut self, event: &WatcherEvent) -> Vec<PathBuf> {
        match event {
            WatcherEvent::Added(path) | WatcherEvent::Removed(path) => vec![self.existing_parent(path)],
            WatcherEvent::Renamed { from, to } => vec![self.existing_parent(from), self.existing_parent(to)],
            WatcherEvent::Overflow(scope) => vec![scope.clone()],
        }
    }

    fn existing_parent(&mut self, path: &Path) -> PathBuf {
        let Some(parent) = path.parent() else { return self.root.clone() };
        if let Some(known) = self.parents.get(parent) {
            return known.clone();
        }
        let found = self.find_existing_parent(path);
        if self.parents.len() >= MAX_REMEMBERED_PARENTS {
            self.parents.clear();
        }
        self.parents.insert(parent.to_path_buf(), found.clone());
        found
    }

    fn find_existing_parent(&self, path: &Path) -> PathBuf {
        let mut current = path.parent();
        while let Some(dir) = current {
            if !dir.starts_with(&self.root) {
                break;
            }
            if dir.is_dir() {
                return dir.to_path_buf();
            }
            current = dir.parent();
        }
        self.root.clone()
    }

    /// The queued events, and the time of the take: an entry made later has
    /// its event in a later take.
    /// Folders that this take finds under heavy churn become deferred first,
    /// and their events leave the batch.
    fn take_all(&mut self) -> (Vec<WatcherEvent>, std::time::SystemTime) {
        self.collapsed = false;
        self.parents.clear();
        let mut events = std::mem::take(&mut self.events);
        if events.len() >= HOT_SCOPE_EVENTS {
            self.defer_hot_scopes(&mut events, Instant::now());
        }
        (events, std::time::SystemTime::now())
    }

    fn defer_hot_scopes(&mut self, events: &mut Vec<WatcherEvent>, now: Instant) {
        // The volume root and the shard roots (its folders) never defer: a
        // walk of one is a walk of a whole shard.
        let min_depth = self.root.components().count() + 2;
        let hot = hot_scopes(events, min_depth);
        if hot.is_empty() {
            return;
        }
        for scope in hot {
            self.defer(scope, now);
        }
        let taken = std::mem::take(events);
        *events = taken.into_iter().filter_map(|event| self.apply_deferral(event, now)).collect();
    }

    fn defer(&mut self, scope: PathBuf, now: Instant) {
        if self.deferred.iter().any(|deferred| scope.starts_with(&deferred.path)) {
            return;
        }
        // The new scope takes over the ones below it, and their age.
        let takes_over = self.deferred.iter().any(|deferred| deferred.path.starts_with(&scope));
        if !takes_over && self.deferred.len() >= MAX_DEFERRED_SCOPES {
            return;
        }
        let mut since = now;
        let mut taken = Vec::new();
        self.deferred.retain(|deferred| {
            let below = deferred.path.starts_with(&scope);
            if below {
                since = since.min(deferred.since);
                taken.push(deferred.path.clone());
            }
            !below
        });
        for path in taken {
            self.note_unrecorded(path, false);
        }
        self.note_unrecorded(scope.clone(), true);
        self.deferred.push(DeferredScope { path: scope, since, last_event: now });
    }

    fn note_unrecorded(&mut self, path: PathBuf, recorded: bool) {
        self.unrecorded.insert(path.to_string_lossy().to_lowercase(), (path, recorded));
    }

    /// The changes of the deferred set that the catalog does not have yet.
    fn take_unrecorded(&mut self) -> Vec<(String, bool)> {
        self.unrecorded
            .drain()
            .map(|(_, (path, recorded))| (path.to_string_lossy().into_owned(), recorded))
            .collect()
    }

    /// Puts back changes whose write failed. A newer change of the same path wins.
    fn restore_unrecorded(&mut self, changes: Vec<(String, bool)>) {
        for (path, recorded) in changes {
            self.unrecorded.entry(path.to_lowercase()).or_insert((PathBuf::from(path), recorded));
        }
    }

    /// The event as the catalog should see it, or `None` when it happens below
    /// a deferred scope (the scope's walk covers it; its time moves). Only the
    /// entries below a scope are covered, not the scope itself: a walk of a
    /// folder that is gone repairs nothing. A rename across the boundary keeps
    /// its outside half: the entry that left is added, the entry that came in
    /// is removed.
    fn apply_deferral(&mut self, event: WatcherEvent, now: Instant) -> Option<WatcherEvent> {
        if self.deferred.is_empty() {
            return Some(event);
        }
        match event {
            WatcherEvent::Overflow(_) => Some(event),
            WatcherEvent::Added(ref path) | WatcherEvent::Removed(ref path) => {
                (!self.touch_deferred(path, now)).then_some(event)
            }
            WatcherEvent::Renamed { from, to } => {
                // A scope inside a renamed folder moves with it.
                let mut moved = Vec::new();
                for deferred in &mut self.deferred {
                    if let Ok(rest) = deferred.path.strip_prefix(&from) {
                        let new_path = if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) };
                        moved.push((std::mem::replace(&mut deferred.path, new_path.clone()), new_path));
                    }
                }
                for (old_path, new_path) in moved {
                    self.note_unrecorded(old_path, false);
                    self.note_unrecorded(new_path, true);
                }
                match (self.touch_deferred(&from, now), self.touch_deferred(&to, now)) {
                    (true, true) => None,
                    (true, false) => Some(WatcherEvent::Added(to)),
                    (false, true) => Some(WatcherEvent::Removed(from)),
                    (false, false) => Some(WatcherEvent::Renamed { from, to }),
                }
            }
        }
    }

    /// True when `path` is below a deferred scope; that scope has an event now.
    fn touch_deferred(&mut self, path: &Path, now: Instant) -> bool {
        let Some(parent) = path.parent() else { return false };
        match self.deferred.iter_mut().find(|deferred| parent.starts_with(&deferred.path)) {
            Some(deferred) => {
                deferred.last_event = now;
                true
            }
            None => false,
        }
    }

    /// The deferred scopes that had no event for `quiet`, or that wait longer
    /// than `DEFERRED_MAX_AGE`. Each leaves the queue: events below it count
    /// again, and a hot flush defers it anew.
    fn take_quiet_scopes(&mut self, now: Instant, quiet: Duration) -> Vec<PathBuf> {
        let mut due = Vec::new();
        self.deferred.retain(|deferred| {
            let is_due = now.saturating_duration_since(deferred.last_event) >= quiet
                || now.saturating_duration_since(deferred.since) >= DEFERRED_MAX_AGE;
            if is_due {
                due.push(deferred.path.clone());
            }
            !is_due
        });
        due
    }

    /// Every deferred scope, for a watcher that stops.
    fn take_deferred_scopes(&mut self) -> Vec<PathBuf> {
        self.deferred.drain(..).map(|deferred| deferred.path).collect()
    }
}

/// Writes the changes of the deferred set to the catalog. A failed write is
/// tried again at the next call. The scopes leave the catalog only when their
/// repair walk is done (`FileIndex::settle_scope_repair`), so a process that
/// ends before the walk leaves them for the next start.
fn record_deferred_scopes(db: &Database, queue: &Mutex<WatcherEventQueue>, volume_id: &str) {
    let changes = queue.lock().unwrap().take_unrecorded();
    if changes.is_empty() {
        return;
    }
    if let Err(error) = db.update_deferred_scopes(volume_id, &changes) {
        eprintln!("[Prism Catalog] Cannot record the deferred scopes of {volume_id}: {error}");
        queue.lock().unwrap().restore_unrecorded(changes);
    }
}

/// The folders that hold `HOT_SCOPE_EVENTS` events or more, deepest first. A
/// folder counts only the events that no deeper chosen folder holds: a build
/// folder with one stray file in it does not take the folders below it. A
/// folder with fewer than `min_depth` path components never qualifies.
fn hot_scopes(events: &[WatcherEvent], min_depth: usize) -> Vec<PathBuf> {
    let mut counts: HashMap<&Path, usize> = HashMap::new();
    for event in events {
        let (first, second) = match event {
            WatcherEvent::Added(path) | WatcherEvent::Removed(path) => (path, None),
            WatcherEvent::Renamed { from, to } => (from, Some(to)),
            WatcherEvent::Overflow(_) => continue,
        };
        for path in std::iter::once(first).chain(second) {
            let depth = path.components().count();
            // Deepest folder first: stop at the first one that is too high.
            for (up, folder) in path.ancestors().enumerate().skip(1) {
                if depth - up < min_depth {
                    break;
                }
                *counts.entry(folder).or_default() += 1;
            }
        }
    }
    let mut folders: Vec<(&Path, usize)> =
        counts.into_iter().filter(|(_, count)| *count >= HOT_SCOPE_EVENTS).collect();
    folders.sort_by_key(|(folder, _)| std::cmp::Reverse(folder.components().count()));
    let mut claimed: HashMap<&Path, usize> = HashMap::new();
    let mut hot = Vec::new();
    for (folder, count) in folders {
        let own = count - claimed.get(folder).copied().unwrap_or(0);
        if own < HOT_SCOPE_EVENTS {
            continue;
        }
        for above in folder.ancestors().skip(1) {
            *claimed.entry(above).or_default() += own;
        }
        hot.push(folder.to_path_buf());
    }
    hot
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

pub fn watcher_root_changed(current: &Path, next: &Path) -> bool {
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
        // Old names that wait for their new name. Renames that run at one
        // time in different folders interleave their halves (old A, old B,
        // new A, new B), and a rename keeps its folder (a move to another
        // folder is reported as a removal and an addition): a new name takes
        // the waiting old name of its own folder.
        let mut pending_rename_old: Vec<PathBuf> = Vec::new();

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

            let returned = bytes_returned as usize;
            let mut offset = 0usize;
            // `FILE_NOTIFY_INFORMATION` is 12 bytes before `FileName`. `size_of`
            // is 16 because of tail padding, so a 14-byte final record is valid.
            const HEADER: usize = 12;
            loop {
                let Some(header_end) = offset.checked_add(HEADER) else {
                    break;
                };
                if header_end > returned || offset % 4 != 0 {
                    break;
                }

                // SAFETY: `header_end` is inside the bytes `ReadDirectoryChangesW`
                // wrote, and `offset` is 4-byte aligned. The name slice below
                // stays inside those bytes.
                let info =
                    unsafe { &*(buffer.as_ptr().add(offset) as *const FILE_NOTIFY_INFORMATION) };
                let name_bytes = info.FileNameLength as usize;
                let Some(name_end) = header_end.checked_add(name_bytes) else {
                    break;
                };
                if name_bytes % 2 != 0 || name_end > returned {
                    break;
                }
                let name_slice = unsafe {
                    std::slice::from_raw_parts(
                        buffer.as_ptr().add(header_end) as *const u16,
                        name_bytes / 2,
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
                            pending_rename_old.push(target_path);
                            // An old name whose new name never came is gone.
                            (pending_rename_old.len() > MAX_PENDING_RENAMES)
                                .then(|| WatcherEvent::Removed(pending_rename_old.remove(0)))
                        }
                        FILE_ACTION_RENAMED_NEW_NAME => {
                            let waiting = pending_rename_old
                                .iter()
                                .position(|old| old.parent() == target_path.parent());
                            match waiting {
                                Some(index) => Some(WatcherEvent::Renamed {
                                    from: pending_rename_old.remove(index),
                                    to: target_path,
                                }),
                                None => Some(WatcherEvent::Added(target_path)),
                            }
                        }
                        _ => None,
                    };
                    if let Some(event) = event {
                        event_queue.lock().unwrap().push(event);
                    }
                }

                if info.NextEntryOffset == 0 {
                    break;
                }
                let next = info.NextEntryOffset as usize;
                let Some(next_offset) = offset.checked_add(next) else {
                    break;
                };
                if next < HEADER || next % 4 != 0 || next_offset > returned {
                    break;
                }
                offset = next_offset;
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

/// Runs a closure when dropped (a trace at every return of a function).
struct TraceOnDrop<F: FnMut()>(F);

impl<F: FnMut()> Drop for TraceOnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// The events of one flush as the writes that apply them: the removals that
/// a rename needs, the renames in order, the additions, the other removals.
/// The paths order by component, so the entries below a folder are one range.
#[derive(Default)]
struct EventFold {
    renames: Vec<(PathBuf, PathBuf)>,
    touched: BTreeSet<PathBuf>,
    removed: BTreeSet<PathBuf>,
    /// Removed entries below a path that a later rename took: their rows go
    /// before the renames, or the renamed rows collide with them.
    removed_first: Vec<PathBuf>,
}

impl EventFold {
    /// The events before a rename name paths as they were before it. An entry
    /// added below `from` is now below `to` (a session folder gets its name
    /// when its files are written): under the old name it would be dropped as
    /// missing. An entry removed below `from` has its row below `to` after
    /// the rename. A removal of `to` itself is over: the name exists again (a
    /// safe save removes the document and renames a temp file to it).
    fn rename(&mut self, from: PathBuf, to: PathBuf) {
        self.removed.remove(&to);
        self.removed_first.extend(take_below(&mut self.removed, &to));
        self.touched.remove(&from);
        for paths in [&mut self.touched, &mut self.removed] {
            for path in take_below(paths, &from) {
                if let Ok(rest) = path.strip_prefix(&from) {
                    paths.insert(to.join(rest));
                }
            }
        }
        // `a -> b`, then `b -> c`: `b` is gone when the flush applies them.
        match self.renames.iter_mut().rev().find(|(_, target)| *target == from) {
            Some(earlier) => earlier.1 = to,
            None => self.renames.push((from, to)),
        }
    }
}

/// Removes and returns the paths below `folder` (not `folder` itself).
fn take_below(paths: &mut BTreeSet<PathBuf>, folder: &Path) -> Vec<PathBuf> {
    let below = paths
        .range::<Path, _>((std::ops::Bound::Excluded(folder), std::ops::Bound::Unbounded))
        .take_while(|path| path.starts_with(folder))
        .cloned()
        .collect::<Vec<_>>();
    for path in &below {
        paths.remove(path);
    }
    below
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
    let (events, taken_at) = {
        let mut q = event_queue.lock().unwrap();
        q.take_all()
    };
    // The events below a new scope are gone now: record the scope first.
    record_deferred_scopes(db, event_queue, volume_id);
    let started = std::time::Instant::now();
    let event_count = events.len();
    let _trace = TraceOnDrop(move || {
        if super::catalog_debug_enabled() && event_count > 0 {
            eprintln!("[catalog] watcher flush events={event_count} ms={}", started.elapsed().as_millis());
        }
    });

    let mut dirty_scopes: HashSet<PathBuf> = HashSet::new();
    let mut fold = EventFold::default();
    for event in events {
        match event {
            WatcherEvent::Overflow(scope) => {
                dirty_scopes.insert(scope);
            }
            WatcherEvent::Renamed { from, to } => fold.rename(from, to),
            WatcherEvent::Removed(path) => {
                fold.touched.remove(&path);
                fold.removed.insert(path);
            }
            WatcherEvent::Added(path) => {
                fold.removed.remove(&path);
                fold.touched.insert(path);
            }
        }
    }
    let EventFold { renames, touched, removed, removed_first } = fold;

    let removal_keys = |paths: &mut dyn Iterator<Item = &PathBuf>| {
        paths.map(|path| (path.to_string_lossy().to_lowercase(), true)).collect::<Vec<_>>()
    };
    let removed_first = removal_keys(&mut removed_first.iter());
    let removed = removal_keys(&mut removed.iter());

    // Each rename moves its rows to the inspected target. A target that
    // vanished before it could be inspected is a removal.
    let old_names: Vec<String> =
        renames.iter().map(|(from, _)| from.to_string_lossy().to_lowercase()).collect();
    let targets: Vec<Option<ScannedItem>> =
        renames.iter().map(|(_, to)| inspect_single_path(to)).collect();
    let rename_writes: Vec<FlushRename<'_>> = old_names
        .iter()
        .zip(&targets)
        .map(|(old_normalized, target)| match target {
            Some(item) => FlushRename::Move { old_normalized, item },
            None => FlushRename::Remove { old_normalized },
        })
        .collect();

    let items: Vec<ScannedItem> = touched
        .iter()
        .filter_map(|path| inspect_single_path(path))
        .collect();
    // A folder that arrived with content (moved from another folder, which
    // is reported as a removal and an addition; restored from the Recycle
    // Bin) sent no events for its children: a walk finds them. A folder that
    // was made and filled in place needs no walk: each child has its own
    // event. Before, every new folder got a walk: a build or a test suite
    // makes thousands of folders a minute.
    for item in items.iter().filter(|item| item.is_directory) {
        let folder = PathBuf::from(&item.display_path);
        if holds_unreported_entry(&folder, &touched, taken_at) {
            dirty_scopes.insert(folder);
        }
    }

    // One transaction for the whole flush. If it fails (the database is
    // busy), nothing is applied and `changed` stays false: the next sweep or
    // repair walk corrects the catalog.
    let mut delta = 0i64;
    let mut changed = false;
    let writes = FlushWrites {
        removed_first: &removed_first,
        renames: &rename_writes,
        items: &items,
        removed: &removed,
    };
    if let Ok(applied) = db.apply_flush(volume_id, &writes) {
        delta = applied.delta;
        changed = applied.changed;
        for (((_, to), target), kept) in renames.iter().zip(&targets).zip(&applied.renames_kept) {
            let Some(item) = target else { continue };
            if *kept {
                // An earlier flush can have read an entry of this folder
                // after the rename, under its old name, and dropped it as
                // missing: then the folder holds an entry with no event
                // here, and a walk finds it.
                if item.is_directory && holds_unreported_entry(to, &touched, taken_at) {
                    dirty_scopes.insert(to.clone());
                }
            } else {
                // A row already has the new path: a walk of the folder that
                // holds the target settles what the catalog has there.
                dirty_scopes.insert(to.parent().unwrap_or(to).to_path_buf());
            }
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
    // When the flusher first saw the events that wait now.
    let mut waiting_since: Option<std::time::Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(FLUSH_INTERVAL);
        if buffering.load(Ordering::SeqCst) {
            continue;
        }
        // No host (a tool, not the app) counts as visible: flush at each tick.
        let hidden = crate::host::get().is_some_and(|host| !host.palette_visible());
        let quiet = if hidden { DEFERRED_QUIET_HIDDEN } else { DEFERRED_QUIET_VISIBLE };
        // A rename can move a scope with no flush.
        record_deferred_scopes(&db, &event_queue, &volume_id);
        let repairs = event_queue.lock().unwrap().take_quiet_scopes(Instant::now(), quiet);
        for scope in repairs {
            on_overflow(volume_id.clone(), scope);
        }
        let (queued, collapsed) = {
            let queue = event_queue.lock().unwrap();
            (queue.events.len(), queue.collapsed)
        };
        if queued == 0 {
            waiting_since = None;
            continue;
        }
        let waited = waiting_since.get_or_insert_with(std::time::Instant::now).elapsed();
        if hidden && !collapsed && queued < HIDDEN_FLUSH_EVENTS && waited < HIDDEN_FLUSH_AGE {
            continue;
        }
        waiting_since = None;
        let applied = apply_queued_events(&db, &event_queue, &volume_id, &counts, &drive);
        if applied.changed {
            on_updated();
        }
        for scope in applied.dirty_scopes {
            on_overflow(volume_id.clone(), scope);
        }
    }
    // The events below a deferred scope are gone: the scope still needs its
    // repair. (Queued events are lost at a stop as well; the scan of a new
    // watcher covers them. The repair does nothing when no watcher is left.)
    record_deferred_scopes(&db, &event_queue, &volume_id);
    let deferred = event_queue.lock().unwrap().take_deferred_scopes();
    for scope in deferred {
        on_overflow(volume_id.clone(), scope);
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

/// True when `folder` holds an entry that this flush has no addition for and
/// that was there at the take (`taken_at`): the catalog would not get it
/// without a walk. An entry made after the take has its event in a later
/// flush. A folder that cannot be listed gets the walk.
fn holds_unreported_entry(folder: &Path, touched: &BTreeSet<PathBuf>, taken_at: std::time::SystemTime) -> bool {
    let Ok(children) = std::fs::read_dir(folder) else {
        return true;
    };
    children.into_iter().any(|child| {
        let Ok(child) = child else { return true };
        if touched.contains(&child.path()) {
            return false;
        }
        // On Windows the listing carries the times: no call for each entry.
        child
            .metadata()
            .and_then(|metadata| metadata.created())
            .map_or(true, |created| created < taken_at)
    })
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

    fn queue() -> WatcherEventQueue {
        WatcherEventQueue::new(PathBuf::from(r"C:\v"))
    }

    fn path(text: &str) -> PathBuf {
        PathBuf::from(format!(r"C:\v\work\{text}"))
    }

    fn add_files(queue: &mut WatcherEventQueue, folder: &str, count: usize) {
        for number in 0..count {
            queue.push(WatcherEvent::Added(path(&format!(r"{folder}\f{number}"))));
        }
    }

    fn scopes(queue: &WatcherEventQueue) -> Vec<PathBuf> {
        let mut scopes: Vec<PathBuf> = queue.deferred.iter().map(|scope| scope.path.clone()).collect();
        scopes.sort();
        scopes
    }

    #[test]
    fn a_burst_under_one_folder_becomes_one_deferred_scope() {
        let mut queue = queue();
        for number in 0..HOT_SCOPE_EVENTS {
            queue.push(WatcherEvent::Added(path(&format!(r"cit\box{number}"))));
            queue.push(WatcherEvent::Added(path(&format!(r"cit\box{number}\file"))));
        }
        let (events, _) = queue.take_all();
        assert!(events.is_empty(), "no event reaches the writes: {events:?}");
        assert_eq!(scopes(&queue), vec![path("cit")]);
        // Later events below the scope never enter the queue.
        add_files(&mut queue, r"cit\box1", 10);
        queue.push(WatcherEvent::Removed(path(r"cit\box2")));
        assert!(queue.events.is_empty());
    }

    #[test]
    fn a_quiet_scope_is_repaired_once() {
        let mut queue = queue();
        add_files(&mut queue, "cit", HOT_SCOPE_EVENTS);
        queue.take_all();
        let quiet = Duration::from_secs(5);
        assert!(queue.take_quiet_scopes(Instant::now() + Duration::from_secs(4), quiet).is_empty());
        // An event below the scope moves its time.
        queue.push(WatcherEvent::Added(path(r"cit\late")));
        let after = Instant::now() + Duration::from_secs(5);
        assert_eq!(queue.take_quiet_scopes(after, quiet), vec![path("cit")]);
        assert!(queue.take_quiet_scopes(after + quiet, quiet).is_empty());
        assert!(queue.deferred.is_empty());
        // The scope can defer again.
        add_files(&mut queue, "cit", HOT_SCOPE_EVENTS);
        queue.take_all();
        assert_eq!(scopes(&queue), vec![path("cit")]);
    }

    #[test]
    fn a_scope_that_never_gets_quiet_repairs_after_the_maximum_age() {
        let mut queue = queue();
        add_files(&mut queue, "cit", HOT_SCOPE_EVENTS);
        queue.take_all();
        let hour = Duration::from_secs(3600);
        assert!(queue.take_quiet_scopes(Instant::now() + DEFERRED_MAX_AGE - Duration::from_secs(5), hour).is_empty());
        let due = queue.take_quiet_scopes(Instant::now() + DEFERRED_MAX_AGE + Duration::from_secs(5), hour);
        assert_eq!(due, vec![path("cit")]);
    }

    #[test]
    fn events_outside_stay_per_event() {
        let mut queue = queue();
        add_files(&mut queue, "cit", HOT_SCOPE_EVENTS);
        add_files(&mut queue, "docs", 3);
        queue.push(WatcherEvent::Removed(path("old.txt")));
        let (events, _) = queue.take_all();
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|event| matches!(event, WatcherEvent::Added(p) if p.starts_with(path("docs")))
            || matches!(event, WatcherEvent::Removed(p) if *p == path("old.txt"))));
        assert_eq!(scopes(&queue), vec![path("cit")]);
    }

    #[test]
    fn fewer_events_than_the_threshold_defer_nothing() {
        let mut queue = queue();
        add_files(&mut queue, "cit", HOT_SCOPE_EVENTS - 1);
        let (events, _) = queue.take_all();
        assert_eq!(events.len(), HOT_SCOPE_EVENTS - 1);
        assert!(queue.deferred.is_empty());
    }

    #[test]
    fn the_scope_itself_and_overflows_stay_in_the_queue() {
        let mut queue = queue();
        add_files(&mut queue, "cit", HOT_SCOPE_EVENTS);
        queue.take_all();
        // The folder itself: a walk of a folder that is gone repairs nothing.
        queue.push(WatcherEvent::Removed(path("cit")));
        queue.push(WatcherEvent::Overflow(path(r"cit\sub")));
        let (events, _) = queue.take_all();
        assert_eq!(events.len(), 2, "{events:?}");
    }

    #[test]
    fn a_rename_across_the_boundary_keeps_its_outside_half() {
        let mut queue = queue();
        add_files(&mut queue, "cit", HOT_SCOPE_EVENTS);
        queue.take_all();
        queue.push(WatcherEvent::Renamed { from: path(r"cit\a"), to: path("out") });
        queue.push(WatcherEvent::Renamed { from: path("in"), to: path(r"cit\b") });
        queue.push(WatcherEvent::Renamed { from: path(r"cit\c"), to: path(r"cit\sub\d") });
        queue.push(WatcherEvent::Renamed { from: path("x"), to: path("y") });
        let (events, _) = queue.take_all();
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(matches!(&events[0], WatcherEvent::Added(p) if *p == path("out")));
        assert!(matches!(&events[1], WatcherEvent::Removed(p) if *p == path("in")));
        assert!(matches!(&events[2], WatcherEvent::Renamed { from, .. } if *from == path("x")));
    }

    #[test]
    fn a_rename_of_the_folder_or_a_parent_moves_the_scope() {
        let mut queue = queue();
        add_files(&mut queue, r"a\cit", HOT_SCOPE_EVENTS);
        queue.take_all();
        queue.push(WatcherEvent::Renamed { from: path("a"), to: path("b") });
        queue.push(WatcherEvent::Renamed { from: path(r"b\cit"), to: path(r"b\cit2") });
        let (events, _) = queue.take_all();
        assert_eq!(events.len(), 2, "both renames reach the writes");
        assert_eq!(scopes(&queue), vec![path(r"b\cit2")]);
    }

    #[test]
    fn the_deepest_folder_with_enough_events_defers() {
        let mut queue = queue();
        // Two hot folders below a parent that holds a few events of its own.
        add_files(&mut queue, r"a\b\c", HOT_SCOPE_EVENTS);
        add_files(&mut queue, r"a\b\d", HOT_SCOPE_EVENTS);
        add_files(&mut queue, r"a\b", 5);
        let (events, _) = queue.take_all();
        assert_eq!(scopes(&queue), vec![path(r"a\b\c"), path(r"a\b\d")]);
        assert_eq!(events.len(), 5);
        // Folders that are cool alone and hot together: their parent.
        let mut queue = self::queue();
        for folder in 0..8 {
            add_files(&mut queue, &format!(r"a\b\f{folder}"), HOT_SCOPE_EVENTS / 8);
        }
        let (events, _) = queue.take_all();
        assert_eq!(scopes(&queue), vec![path(r"a\b")]);
        assert!(events.is_empty());
    }

    #[test]
    fn the_volume_root_and_the_shard_roots_never_defer() {
        let mut queue = queue();
        for number in 0..HOT_SCOPE_EVENTS * 2 {
            queue.push(WatcherEvent::Added(PathBuf::from(format!(r"C:\v\f{number}"))));
            queue.push(WatcherEvent::Added(path(&format!("f{number}"))));
        }
        let (events, _) = queue.take_all();
        assert_eq!(events.len(), HOT_SCOPE_EVENTS * 4);
        assert!(queue.deferred.is_empty());
    }

    #[test]
    fn a_hot_parent_takes_over_the_scopes_below_it() {
        let mut queue = queue();
        add_files(&mut queue, r"a\b\c", HOT_SCOPE_EVENTS);
        queue.take_all();
        assert_eq!(scopes(&queue), vec![path(r"a\b\c")]);
        for folder in 0..8 {
            add_files(&mut queue, &format!(r"a\x{folder}"), HOT_SCOPE_EVENTS / 8);
        }
        queue.take_all();
        assert_eq!(scopes(&queue), vec![path("a")]);
    }

    #[test]
    fn deferred_scopes_have_a_limit_and_none_is_lost() {
        let mut queue = queue();
        let mut written = 0;
        for number in 0..MAX_DEFERRED_SCOPES + 3 {
            add_files(&mut queue, &format!(r"cit{number}\x"), HOT_SCOPE_EVENTS);
            written += queue.take_all().0.len();
        }
        // The folders past the limit are written per event.
        assert_eq!(written, 3 * HOT_SCOPE_EVENTS);
        assert_eq!(queue.take_deferred_scopes().len(), MAX_DEFERRED_SCOPES);
        assert!(queue.deferred.is_empty());
    }

    fn changes(queue: &mut WatcherEventQueue) -> Vec<(PathBuf, bool)> {
        let mut changes: Vec<(PathBuf, bool)> =
            queue.take_unrecorded().into_iter().map(|(path, recorded)| (PathBuf::from(path), recorded)).collect();
        changes.sort();
        changes
    }

    #[test]
    fn a_change_of_the_deferred_set_is_recorded_once_not_per_event() {
        let mut queue = queue();
        add_files(&mut queue, r"a\b\c", HOT_SCOPE_EVENTS);
        queue.take_all();
        assert_eq!(changes(&mut queue), vec![(path(r"a\b\c"), true)]);
        // Many events below the scope change nothing.
        add_files(&mut queue, r"a\b\c\x", 100);
        queue.take_all();
        assert!(changes(&mut queue).is_empty());
        // A parent takes the scope over: the child leaves the record.
        for folder in 0..8 {
            add_files(&mut queue, &format!(r"a\x{folder}"), HOT_SCOPE_EVENTS / 8);
        }
        queue.take_all();
        assert_eq!(changes(&mut queue), vec![(path("a"), true), (path(r"a\b\c"), false)]);
        // A rename moves the record.
        queue.push(WatcherEvent::Renamed { from: path("a"), to: path("z") });
        assert_eq!(changes(&mut queue), vec![(path("a"), false), (path("z"), true)]);
    }

    #[test]
    fn a_failed_write_keeps_the_changes_and_a_newer_change_wins() {
        let mut queue = queue();
        add_files(&mut queue, "cit", HOT_SCOPE_EVENTS);
        queue.take_all();
        let failed = queue.take_unrecorded();
        queue.restore_unrecorded(failed);
        assert_eq!(changes(&mut queue), vec![(path("cit"), true)]);
        // The scope moved while the write ran: the new state stays.
        let failed = vec![(path("cit").to_string_lossy().into_owned(), true)];
        queue.note_unrecorded(path("cit"), false);
        queue.restore_unrecorded(failed);
        assert_eq!(changes(&mut queue), vec![(path("cit"), false)]);
    }

    #[test]
    fn deferred_scopes_reach_the_catalog_and_a_restart_reads_them_back() {
        let dir = std::env::temp_dir().join(format!("prism-watch-test-{}-record", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db_path = dir.join("catalog.db");
        let mut queue = queue();
        add_files(&mut queue, "cit", HOT_SCOPE_EVENTS);
        add_files(&mut queue, "build", HOT_SCOPE_EVENTS);
        queue.take_all();
        let queue = Mutex::new(queue);

        let db = Database::open(&db_path).unwrap();
        record_deferred_scopes(&db, &queue, "vol");
        assert!(queue.lock().unwrap().unrecorded.is_empty(), "the set is written once");
        // The process ends here: the flusher never hands the scopes over.
        drop(db);

        let db = Database::open(&db_path).unwrap();
        let mut found = db.deferred_scopes("vol").unwrap();
        found.sort();
        assert_eq!(found, vec![path("build").to_string_lossy().into_owned(), path("cit").to_string_lossy().into_owned()]);
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }
}
