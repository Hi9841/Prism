//! The file index: one in-memory `FileStore` (`store.rs`), fed by a walk of
//! each volume (`walk.rs`) and by the NTFS change journal (`journal.rs`), and
//! saved to a snapshot file (`snapshot.rs`).
//!
//! Threads:
//! - `warm` runs one async task. It loads the snapshot, finds the volumes
//!   every 15 s, starts or stops one volume thread for each, and saves the
//!   snapshot when the store changed.
//! - One OS thread per volume. A journal volume builds (or catches up from
//!   its checkpoint), then reads the journal in a loop. A walk volume walks
//!   at start and again every 30 minutes. A volume thread owns all writes to
//!   its volume in the store, so no two threads change one volume.
//! - Searches run on the blocking pool and only read the store.

pub mod journal;
pub mod search;
pub mod snapshot;
pub mod store;
pub mod types;
pub mod volume;
pub mod walk;

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine;
use windows::core::GUID;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Profile, FOLDERID_Videos, SHGetKnownFolderPath,
};

pub use types::{
    FileSearchError, FileSearchErrorKind, FileSearchResponse, QuickAccessEntry, VolumeCoverage,
    VolumeInfo, VolumeState,
};

use self::journal::{journal_continuity, JournalContinuity, JournalError, JournalVolume};
use self::store::{FileStore, FsChange, FsNode, VolumeBackend, VolumeMeta, VolumeSlot};
use self::types::JournalCheckpoint;
use self::walk::Exclusions;

/// Mount topology (usb plug, network mount) changes are rare; the discovery
/// pass calls GetVolumeInformationW on every volume, which can block for
/// seconds on disconnected network drives. Only re-enumerate this often.
const VOLUME_DISCOVERY_INTERVAL: Duration = Duration::from_secs(15);
/// A volume with no journal is walked again this often while it is mounted.
const WALK_VOLUME_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// A walk volume whose walk failed tries again after this time.
const WALK_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// The snapshot is written at most this often while the store changes.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// A snapshot is also written for a journal cursor that moved with no entry
/// change, but not more often than this (a save of a clean store costs
/// 100 MB or more of writes).
const CURSOR_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// The reader gives its cursor to the store at most this often when no
/// change comes with it.
const CURSOR_PUBLISH_GAP: Duration = Duration::from_secs(5);
/// The pause after a read that found no record. Reads do not wait in the
/// kernel (see `JournalVolume::read_with`), so this is the search latency of a
/// new file and the poll cost: one read per second per volume.
const EMPTY_READ_PAUSE: Duration = Duration::from_secs(1);
/// A volume that the discovery misses keeps its entries (state Offline) for
/// this many passes (about 1 minute). Then its entries go.
const VOLUME_MISSING_PASSES: u32 = 4;
/// At exit the save waits this long for a folder walk to end.
const EXIT_WALK_WAIT: Duration = Duration::from_secs(3);
/// A failed journal read is tried again after 1 s, then 2 s, ... up to this.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// `file-index-updated` is sent at most this often while changes arrive.
const EMIT_GAP: Duration = Duration::from_secs(2);
/// Nodes of a walk that one write-lock step applies.
const APPLY_CHUNK: usize = 4096;
/// Nodes of a walk that go to the store in one message.
const WALK_BATCH: usize = 4096;
/// A walk of a new folder that is faster than this is not logged: the log
/// would fill with small walks.
const WALK_LOG_MIN: Duration = Duration::from_secs(1);

const SNAPSHOT_FILE: &str = "file-index.bin";
const EXCLUSIONS_FILE: &str = "file-index-exclusions.txt";
/// The files of the old SQLite catalog. They go when the new index is built.
const LEGACY_CATALOG_FILES: [&str; 3] = ["catalog.db", "catalog.db-wal", "catalog.db-shm"];

pub fn catalog_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("PRISM_CATALOG_DEBUG")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

// ---------------------------------------------------------------------------
// Walk log
// ---------------------------------------------------------------------------

/// One line of the always-on walk log (`catalog-walks`).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WalkRecord<'a> {
    unix_ms: u128,
    /// `volume` (a full walk of a volume) or `folder` (a folder that became
    /// visible).
    kind: &'static str,
    scope: &'a str,
    entries: u64,
    ms: u64,
}

/// Appends a record for a walk. A `folder` walk is logged only when it took
/// `WALK_LOG_MIN` or more. File I/O: call it off the UI thread.
fn log_walk(kind: &'static str, scope: &Path, entries: u64, elapsed: Duration) {
    if kind == "folder" && elapsed < WALK_LOG_MIN {
        return;
    }
    let Some(dir) = crate::logs::dir() else { return };
    let scope = scope.to_string_lossy();
    let record = WalkRecord {
        unix_ms: crate::logs::unix_ms(),
        kind,
        scope: &scope,
        entries,
        ms: elapsed.as_millis() as u64,
    };
    crate::logs::append(&dir, "catalog-walks", &record);
}

// ---------------------------------------------------------------------------
// Exclusion list file
// ---------------------------------------------------------------------------

const EXCLUSIONS_HEADER: &str = "\
# Prism file index: folders that Prism does not index.
# One item on each line. Blank lines and lines that start with # are ignored.
# A folder name (node_modules) matches a folder of that name anywhere.
# An absolute path (C:\\Work\\cache) matches only that folder.
# <home> stands for your user folder.
# Prism reads this file when it starts. A changed list starts a new full walk.
";

/// The items of an exclusion file: one for each line that is not blank and
/// not a comment.
fn parse_exclusion_list(text: &str) -> Vec<String> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The text of an exclusion file: the format header and one item for each line.
fn format_exclusion_file(items: &[String]) -> String {
    let mut text = String::from(EXCLUSIONS_HEADER);
    for item in items {
        text.push_str(item);
        text.push('\n');
    }
    text
}

/// A number for a list that does not change with the order or the case of
/// its items (FNV-1a over the sorted lowercase items).
fn exclusion_fingerprint(items: &[String]) -> u64 {
    let mut lowered: Vec<String> = items.iter().map(|item| item.trim().to_lowercase()).collect();
    lowered.sort();
    lowered.dedup();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for item in &lowered {
        for byte in item.bytes().chain(std::iter::once(b'\n')) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Reads the exclusion file of the app data folder. When it does not exist,
/// writes the default list there and returns it. A file that cannot be read
/// gives the default list for this run.
fn load_exclusion_list(app_data_dir: &Path) -> Vec<String> {
    let path = app_data_dir.join(EXCLUSIONS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_exclusion_list(&text),
        Err(error) => {
            let items = Exclusions::default_entries(app_data_dir);
            if error.kind() == std::io::ErrorKind::NotFound {
                let _ = std::fs::create_dir_all(app_data_dir);
                if let Err(error) = std::fs::write(&path, format_exclusion_file(&items)) {
                    eprintln!("[Prism Catalog] cannot write {}: {error}", path.display());
                }
            } else {
                eprintln!("[Prism Catalog] cannot read {}: {error}", path.display());
            }
            items
        }
    }
}

/// The exclusion list of this run, as the walk and the journal use it.
struct ExclusionSet {
    matcher: Exclusions,
    fingerprint: u64,
}

impl ExclusionSet {
    fn new(items: &[String]) -> Self {
        Self { matcher: Exclusions::new(items), fingerprint: exclusion_fingerprint(items) }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers of the journal reader
// ---------------------------------------------------------------------------

/// The folders in `changes` that were not live, shown folders before the
/// changes: a folder that came in from the recycle bin, an excluded folder,
/// another volume, or that was an excluded folder with another name. The
/// journal gives no record for what is inside them, so the caller walks
/// them. A folder whose parent is also in the list is left out: the walk of
/// the parent covers it. `was_accepted` is asked before the changes are
/// applied.
fn folders_to_walk(changes: &[FsChange], was_accepted: &dyn Fn(u64) -> bool) -> Vec<u64> {
    let new: Vec<(u64, u64)> = changes
        .iter()
        .filter_map(|change| match change {
            FsChange::Upsert(node)
                if node.is_dir && !node.excluded && !node.link && !was_accepted(node.record) =>
            {
                Some((node.record, node.parent_record))
            }
            _ => None,
        })
        .collect();
    let records: HashSet<u64> = new.iter().map(|(record, _)| *record).collect();
    new.into_iter()
        .filter(|(_, parent)| !records.contains(parent))
        .map(|(record, _)| record)
        .collect()
}

/// The nodes of a walk of folder `root` as upserts, each folder before its
/// contents (the walk can send them in any order). A node whose parent is
/// not below `root` is dropped.
fn order_subtree(root: u64, nodes: Vec<FsNode>) -> Vec<FsChange> {
    let mut by_parent: HashMap<u64, Vec<FsNode>> = HashMap::new();
    for node in nodes {
        by_parent.entry(node.parent_record).or_default().push(node);
    }
    let mut ordered = Vec::new();
    let mut parents = vec![root];
    let mut next = 0;
    while next < parents.len() {
        let parent = parents[next];
        next += 1;
        for child in by_parent.remove(&parent).unwrap_or_default() {
            if child.is_dir && !child.excluded {
                parents.push(child.record);
            }
            ordered.push(FsChange::Upsert(child));
        }
    }
    ordered
}

/// Deletes the files of the old SQLite catalog. Returns how many it deleted.
fn delete_legacy_catalog(app_data_dir: &Path) -> usize {
    let mut deleted = 0;
    for name in LEGACY_CATALOG_FILES {
        let path = app_data_dir.join(name);
        if !path.exists() {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => deleted += 1,
            Err(error) => eprintln!("[Prism Catalog] cannot delete {}: {error}", path.display()),
        }
    }
    deleted
}

// ---------------------------------------------------------------------------
// Volume threads
// ---------------------------------------------------------------------------

/// What a volume thread shares with the status code.
struct RunShared {
    /// A `VolumeState` as a number.
    state: AtomicU8,
    /// Entries a running walk has found.
    progress: AtomicU64,
    /// The first build (or catch-up) of the volume ended.
    built: AtomicBool,
}

impl RunShared {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(Self::code(VolumeState::Indexing)),
            progress: AtomicU64::new(0),
            built: AtomicBool::new(false),
        }
    }

    fn code(state: VolumeState) -> u8 {
        match state {
            VolumeState::Ready => 0,
            VolumeState::Indexing => 1,
            VolumeState::Offline => 2,
            VolumeState::Error => 3,
        }
    }

    fn set_state(&self, state: VolumeState) {
        self.state.store(Self::code(state), Ordering::Relaxed);
    }

    fn state(&self) -> VolumeState {
        match self.state.load(Ordering::Relaxed) {
            0 => VolumeState::Ready,
            1 => VolumeState::Indexing,
            2 => VolumeState::Offline,
            _ => VolumeState::Error,
        }
    }
}

/// One volume as its thread sees it.
struct VolumeCtl {
    volume_id: String,
    mount: PathBuf,
    /// The mount path with a trailing backslash (`C:\`): the name in the
    /// coverage list and in the snapshot.
    mount_text: String,
    ntfs: bool,
    /// Set to stop the thread (volume removed, rebuild, exit).
    stop: AtomicBool,
    shared: RunShared,
}

impl VolumeCtl {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Sleeps for `total`, in short steps. Returns false when stopped.
    fn sleep(&self, total: Duration) -> bool {
        let end = Instant::now() + total;
        loop {
            if self.stopped() {
                return false;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(Duration::from_millis(250)));
        }
    }
}

struct Run {
    ctl: Arc<VolumeCtl>,
    thread: Option<JoinHandle<()>>,
    /// Discovery passes in a row that did not find the volume. 0: the volume
    /// is mounted and its thread runs. More: its thread is stopped and its
    /// entries stay (state Offline) until `VOLUME_MISSING_PASSES`.
    missing: u32,
}

/// A run that reconcile takes out of the map or stops.
struct Leaving {
    ctl: Arc<VolumeCtl>,
    thread: Option<JoinHandle<()>>,
    /// The entries of the volume go. When false the volume stays in the
    /// store (it is Offline, or it starts again).
    remove_entries: bool,
    /// The run stays in the map as Offline.
    offline: bool,
}

/// Counts a running build for the `indexing` flag.
struct BuildGuard<'a> {
    builds: &'a AtomicUsize,
}

impl<'a> BuildGuard<'a> {
    fn new(builds: &'a AtomicUsize, ctl: &VolumeCtl) -> Self {
        builds.fetch_add(1, Ordering::SeqCst);
        ctl.shared.set_state(VolumeState::Indexing);
        Self { builds }
    }
}

impl Drop for BuildGuard<'_> {
    fn drop(&mut self) {
        self.builds.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Counts the folder walks of a journal batch that are not finished: the
/// snapshot is not saved while one runs (see `save_snapshot`).
struct WalkGuard<'a> {
    pending: &'a AtomicUsize,
}

impl<'a> WalkGuard<'a> {
    fn new(pending: &'a AtomicUsize) -> Self {
        pending.fetch_add(1, Ordering::SeqCst);
        Self { pending }
    }
}

impl Drop for WalkGuard<'_> {
    fn drop(&mut self) {
        self.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How the journal read loop ended.
enum LoopEnd {
    Stopped,
    /// The journal lost its history: walk the volume again.
    Rebuild,
}

struct Core {
    store: RwLock<Arc<FileStore>>,
    search_generation: AtomicU64,
    app_data_dir: RwLock<PathBuf>,
    exclusions: RwLock<Arc<ExclusionSet>>,
    runs: Mutex<HashMap<String, Run>>,
    /// Held while volume threads start or stop.
    reconcile: Mutex<()>,
    /// The snapshot is loaded (or known to be missing).
    loaded: AtomicBool,
    /// `warm` has not found the volumes yet.
    warming: AtomicBool,
    /// Builds that run now.
    builds: AtomicUsize,
    /// Journal batches whose folder walks run now.
    walks_pending: AtomicUsize,
    ready: AtomicBool,
    /// A good snapshot file exists: loaded or saved.
    snapshot_ok: AtomicBool,
    /// A journal checkpoint moved since the last save, with no entry change.
    cursor_moved: AtomicBool,
    legacy_done: AtomicBool,
    save_lock: Mutex<()>,
    last_save: Mutex<Instant>,
}

#[derive(Clone)]
pub struct FileIndex {
    core: Arc<Core>,
}

impl Default for FileIndex {
    fn default() -> Self {
        let no_exclusions = Arc::new(ExclusionSet::new(&[]));
        Self {
            core: Arc::new(Core {
                store: RwLock::new(Arc::new(FileStore::new())),
                search_generation: AtomicU64::new(0),
                app_data_dir: RwLock::new(PathBuf::from(".")),
                exclusions: RwLock::new(no_exclusions),
                runs: Mutex::new(HashMap::new()),
                reconcile: Mutex::new(()),
                loaded: AtomicBool::new(false),
                warming: AtomicBool::new(false),
                builds: AtomicUsize::new(0),
                walks_pending: AtomicUsize::new(0),
                ready: AtomicBool::new(false),
                snapshot_ok: AtomicBool::new(false),
                cursor_moved: AtomicBool::new(false),
                legacy_done: AtomicBool::new(false),
                save_lock: Mutex::new(()),
                last_save: Mutex::new(Instant::now()),
            }),
        }
    }
}

impl FileIndex {
    /// Loads the snapshot and the exclusion list of `app_data_dir`. A missing
    /// or bad snapshot gives an empty store (the volumes are built by walks).
    pub fn init(&self, app_data_dir: &Path) {
        *self.core.app_data_dir.write().unwrap_or_else(|e| e.into_inner()) = app_data_dir.to_path_buf();
        let path = app_data_dir.join(SNAPSHOT_FILE);
        let store = match FileStore::load(&path) {
            Ok(store) => {
                self.core.snapshot_ok.store(true, Ordering::SeqCst);
                store
            }
            Err(error) => {
                if path.exists() {
                    eprintln!("[Prism Catalog] the snapshot is not usable, building again: {error}");
                } else {
                    eprintln!("[Prism Catalog] no snapshot yet, building the index");
                }
                FileStore::new()
            }
        };
        self.load_exclusions();
        if self.total_indexed(&store) > 0 {
            self.core.ready.store(true, Ordering::SeqCst);
        }
        *self.core.store.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(store);
        self.core.loaded.store(true, Ordering::SeqCst);
    }

    fn load_exclusions(&self) {
        let dir = self.core.app_data_dir.read().unwrap_or_else(|e| e.into_inner()).clone();
        let items = load_exclusion_list(&dir);
        *self.core.exclusions.write().unwrap_or_else(|e| e.into_inner()) =
            Arc::new(ExclusionSet::new(&items));
    }

    /// The store, for checks and probes (`examples/index_probe.rs`).
    #[doc(hidden)]
    pub fn store_handle(&self) -> Arc<FileStore> {
        self.store()
    }

    fn store(&self) -> Arc<FileStore> {
        self.core.store.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn exclusions(&self) -> Arc<ExclusionSet> {
        self.core.exclusions.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn app_data_dir(&self) -> PathBuf {
        self.core.app_data_dir.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn total_indexed(&self, store: &FileStore) -> u64 {
        store
            .volumes()
            .iter()
            .filter_map(|meta| store.volume_slot(&meta.volume_id))
            .map(|slot| store.live_count(slot))
            .sum()
    }

    pub fn begin_search(&self) -> u64 {
        self.core.search_generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn indexing(&self) -> bool {
        self.core.warming.load(Ordering::Relaxed) || self.core.builds.load(Ordering::Relaxed) > 0
    }

    /// The coverage of each volume and the number of entries of the store.
    fn status(&self, store: &FileStore) -> (Vec<VolumeCoverage>, u64) {
        let mut volumes: Vec<VolumeCoverage> = self
            .core
            .runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|run| {
                let ctl = &run.ctl;
                let stored = store
                    .volume_slot(&ctl.volume_id)
                    .map_or(0, |slot| store.live_count(slot));
                let state = ctl.shared.state();
                let indexed_count = if state == VolumeState::Ready {
                    stored
                } else {
                    stored.max(ctl.shared.progress.load(Ordering::Relaxed))
                };
                VolumeCoverage {
                    drive: ctl.mount_text.clone(),
                    state,
                    indexed_count,
                    total_progress: None,
                }
            })
            .collect();
        volumes.sort_by(|a, b| a.drive.cmp(&b.drive));
        (volumes, self.total_indexed(store))
    }

    pub fn search_with_generation(
        &self,
        query: &str,
        limit: Option<usize>,
        generation: u64,
    ) -> FileSearchResponse {
        let store = self.store();
        let (volumes, total_indexed) = self.status(&store);
        let indexing = self.indexing();
        let ready = self.core.ready.load(Ordering::Relaxed) || total_indexed > 0;
        search::search_with_generation(
            query,
            limit,
            &store,
            &self.core.search_generation,
            generation,
            &volumes,
            total_indexed,
            indexing,
            ready,
        )
    }

    /// Builds every volume again from a walk (the UI's "rebuild index"
    /// action). The old entries stay searchable until the new ones are ready.
    pub fn rebuild(&self) {
        let this = self.clone();
        crate::runtime::spawn(async move {
            let _ = crate::runtime::spawn_blocking(move || {
                this.load_exclusions();
                let volumes = volume::discover_volumes();
                this.reconcile_volumes(volumes, true);
            })
            .await;
        });
    }

    /// Saves the snapshot when something changed. Call it before the process
    /// exits, off the UI thread. It stops the volume threads first, so the
    /// checkpoints stay as they are.
    pub fn flush_for_exit(&self) {
        for run in self.core.runs.lock().unwrap_or_else(|e| e.into_inner()).values() {
            run.ctl.stop.store(true, Ordering::Relaxed);
        }
        // A stopped folder walk takes its folders out of the store again.
        // Wait for it, so the file never holds a folder with no contents.
        let end = Instant::now() + EXIT_WALK_WAIT;
        while self.core.walks_pending.load(Ordering::SeqCst) > 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.save_snapshot("exit", true);
    }

    fn emit_updated(&self) {
        crate::host::emit(crate::host::CoreEvent::FileIndexUpdated);
    }

    // -----------------------------------------------------------------------
    // Snapshot
    // -----------------------------------------------------------------------

    /// Writes the snapshot file. With `only_if_changed` it does nothing when
    /// neither an entry nor a checkpoint changed since the last save. Call it
    /// on a thread that may block.
    fn save_snapshot(&self, why: &str, only_if_changed: bool) {
        // A store that was never loaded would replace a good file.
        if !self.core.loaded.load(Ordering::SeqCst) {
            return;
        }
        let _guard = self.core.save_lock.lock().unwrap_or_else(|e| e.into_inner());
        let store = self.store();
        if store.volumes().is_empty() {
            return;
        }
        if only_if_changed && !store.is_dirty() && !self.core.cursor_moved.load(Ordering::SeqCst) {
            return;
        }
        // A folder walk of a journal batch runs: the store holds the folder
        // but not its contents yet, and the checkpoint is before the batch.
        // A file saved now would lose the contents at the next start.
        if why != "exit" && self.core.walks_pending.load(Ordering::SeqCst) > 0 {
            return;
        }
        self.core.cursor_moved.store(false, Ordering::SeqCst);
        let path = self.app_data_dir().join(SNAPSHOT_FILE);
        let started = Instant::now();
        match store.save(&path) {
            Ok(bytes) => {
                self.core.snapshot_ok.store(true, Ordering::SeqCst);
                *self.core.last_save.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                eprintln!(
                    "[Prism Catalog] snapshot saved ({why}): {} MB in {} ms",
                    bytes / 1_048_576,
                    started.elapsed().as_millis()
                );
            }
            Err(error) => {
                self.core.cursor_moved.store(true, Ordering::SeqCst);
                eprintln!("[Prism Catalog] cannot save the snapshot ({why}): {error}");
            }
        }
    }

    /// The store changed and the last save is `SNAPSHOT_INTERVAL` old, or
    /// only the journal cursor moved and the last save is
    /// `CURSOR_SNAPSHOT_INTERVAL` old. A file with an old cursor makes the
    /// next start read a long journal, or walk the volume.
    fn snapshot_due(&self) -> bool {
        let since_save = self.core.last_save.lock().unwrap_or_else(|e| e.into_inner()).elapsed();
        (self.store().is_dirty() && since_save >= SNAPSHOT_INTERVAL)
            || (self.core.cursor_moved.load(Ordering::SeqCst) && since_save >= CURSOR_SNAPSHOT_INTERVAL)
    }

    /// When every volume has built and a snapshot exists, the old SQLite
    /// catalog is not needed any more: deletes it, once.
    fn finish_migration_if_ready(&self) {
        if self.core.legacy_done.load(Ordering::SeqCst) || !self.core.snapshot_ok.load(Ordering::SeqCst) {
            return;
        }
        // Every volume that is still mounted has built well (a volume in
        // Error does not set `built`).
        let all_built = {
            let runs = self.core.runs.lock().unwrap_or_else(|e| e.into_inner());
            let mut mounted = runs.values().filter(|run| run.missing == 0).peekable();
            mounted.peek().is_some() && mounted.all(|run| run.ctl.shared.built.load(Ordering::SeqCst))
        };
        if !all_built || self.core.legacy_done.swap(true, Ordering::SeqCst) {
            return;
        }
        let deleted = delete_legacy_catalog(&self.app_data_dir());
        if deleted > 0 {
            eprintln!("[Prism Catalog] deleted {deleted} file(s) of the old catalog");
        }
    }

    // -----------------------------------------------------------------------
    // Volumes
    // -----------------------------------------------------------------------

    /// Starts a thread for each new volume and stops the thread of each
    /// volume that is gone (its entries stay for a short time, then go).
    /// With `force` every thread restarts and builds its volume from a new
    /// walk.
    fn reconcile_volumes(&self, discovered: Vec<VolumeInfo>, force: bool) {
        let _reconcile = self.core.reconcile.lock().unwrap_or_else(|e| e.into_inner());

        // The wanted volumes: accessible, one for each id and each mount.
        let mut wanted: Vec<(VolumeInfo, PathBuf)> = Vec::new();
        for info in discovered.into_iter().filter(|info| info.accessible) {
            let Some(mount) = info.canonical_mount_path() else { continue };
            let key = mount.to_string_lossy().to_lowercase();
            let taken = wanted.iter().any(|(held, held_mount)| {
                held.volume_id == info.volume_id || held_mount.to_string_lossy().to_lowercase() == key
            });
            if !taken {
                wanted.push((info, mount));
            }
        }

        // Phase 1: take out the threads that must stop. A volume that the
        // discovery does not find stays in the map as Offline with its
        // entries, until it is missing for `VOLUME_MISSING_PASSES` passes in
        // a row.
        let mut leaving: Vec<Leaving> = Vec::new();
        {
            let mut runs = self.core.runs.lock().unwrap_or_else(|e| e.into_inner());
            let ids: Vec<String> = runs.keys().cloned().collect();
            for id in ids {
                let kept = wanted.iter().find(|(info, _)| info.volume_id == id);
                let Some(run) = runs.get_mut(&id) else { continue };
                let Some((_, mount)) = kept else {
                    run.missing += 1;
                    if run.missing >= VOLUME_MISSING_PASSES {
                        if let Some(run) = runs.remove(&id) {
                            leaving.push(Leaving {
                                ctl: run.ctl,
                                thread: run.thread,
                                remove_entries: true,
                                offline: false,
                            });
                        }
                    } else if run.missing == 1 {
                        leaving.push(Leaving {
                            ctl: Arc::clone(&run.ctl),
                            thread: run.thread.take(),
                            remove_entries: false,
                            offline: true,
                        });
                    }
                    continue;
                };
                let same_mount = mount_text(mount).eq_ignore_ascii_case(&run.ctl.mount_text);
                let finished = run.thread.as_ref().is_none_or(JoinHandle::is_finished);
                // An Offline volume that is back starts again like a new one:
                // the journal decides between a catch-up and a walk.
                if force || !same_mount || finished || run.missing > 0 {
                    if let Some(run) = runs.remove(&id) {
                        leaving.push(Leaving {
                            ctl: run.ctl,
                            thread: run.thread,
                            remove_entries: false,
                            offline: false,
                        });
                    }
                }
            }
        }

        // Phase 2: stop them and wait. The store changes only after the
        // thread is gone: a late write could reach a slot that another
        // volume took.
        let store = self.store();
        for leave in &leaving {
            leave.ctl.stop.store(true, Ordering::Relaxed);
        }
        for mut leave in leaving {
            if let Some(thread) = leave.thread.take() {
                let _ = thread.join();
            }
            if leave.offline {
                eprintln!("[Prism Catalog] volume {} is offline", leave.ctl.mount_text);
                leave.ctl.shared.set_state(VolumeState::Offline);
            }
            if leave.remove_entries {
                eprintln!("[Prism Catalog] volume {} is gone", leave.ctl.mount_text);
                store.remove_volume(&leave.ctl.volume_id);
            }
        }
        // Entries of volumes that this run does not know (a snapshot of
        // a drive that is not mounted now). An Offline volume keeps them.
        if !wanted.is_empty() {
            let runs = self.core.runs.lock().unwrap_or_else(|e| e.into_inner());
            for meta in store.volumes() {
                if !wanted.iter().any(|(info, _)| info.volume_id == meta.volume_id)
                    && !runs.contains_key(&meta.volume_id)
                {
                    store.remove_volume(&meta.volume_id);
                }
            }
        }

        // Phase 3: start the new threads. The lock stays held, so a fast
        // thread that checks "every volume built" sees all of them.
        let mut runs = self.core.runs.lock().unwrap_or_else(|e| e.into_inner());
        for (info, mount) in wanted {
            if runs.contains_key(&info.volume_id) {
                continue;
            }
            let ctl = Arc::new(VolumeCtl {
                volume_id: info.volume_id.clone(),
                mount_text: mount_text(&mount),
                mount,
                ntfs: info.fs_type.eq_ignore_ascii_case("NTFS"),
                stop: AtomicBool::new(false),
                shared: RunShared::new(),
            });
            let index = self.clone();
            let thread_ctl = Arc::clone(&ctl);
            let spawned = std::thread::Builder::new()
                .name(format!("catalog-{}", ctl.mount_text))
                .spawn(move || index.volume_main(&thread_ctl, force));
            match spawned {
                Ok(thread) => {
                    runs.insert(info.volume_id, Run { ctl, thread: Some(thread), missing: 0 });
                }
                Err(error) => eprintln!("[Prism Catalog] cannot start the thread of {}: {error}", ctl.mount_text),
            }
        }
    }

    /// The body of a volume thread.
    fn volume_main(&self, ctl: &VolumeCtl, force: bool) {
        let journal = if ctl.ntfs {
            JournalVolume::open(&ctl.mount)
                .ok()
                .filter(|journal| journal.query().is_ok())
        } else {
            None
        };
        match journal {
            Some(journal) => self.journal_loop(ctl, &journal, force),
            None => self.walk_loop(ctl),
        }
    }

    /// The first build of a volume ended (well or not): the status, the
    /// snapshot, the migration. A build that ended in Error does not count for
    /// the migration.
    fn finish_build(&self, ctl: &VolumeCtl, state: VolumeState, rebuilt: bool) {
        ctl.shared.set_state(state);
        self.core.ready.store(true, Ordering::SeqCst);
        if rebuilt && state == VolumeState::Ready {
            self.save_snapshot("build", false);
        }
        if state != VolumeState::Error {
            ctl.shared.built.store(true, Ordering::SeqCst);
        }
        self.emit_updated();
        self.finish_migration_if_ready();
    }

    /// Walks `ctl.mount` into the store with `meta`. The walk and the build of
    /// the new arrays run at the same time, so the nodes are never all in
    /// memory. `Ok(None)`: stopped. The old entries stay when the walk fails.
    fn walk_into_store(
        &self,
        ctl: &VolumeCtl,
        meta: VolumeMeta,
        synthetic: bool,
    ) -> Result<Option<VolumeSlot>, String> {
        let store = self.store();
        let exclusions = self.exclusions();
        ctl.shared.progress.store(0, Ordering::Relaxed);
        let failed = AtomicBool::new(false);
        let started = Instant::now();
        let (slot, walked) = std::thread::scope(|scope| {
            let (sender, receiver) = sync_channel::<Vec<FsNode>>(16);
            let (matcher, stop, progress, failed_flag) =
                (&exclusions.matcher, &ctl.stop, &ctl.shared.progress, &failed);
            let root = ctl.mount.as_path();
            let walker = scope.spawn(move || {
                let mut batch: Vec<FsNode> = Vec::with_capacity(WALK_BATCH);
                let mut sink = |node: FsNode| {
                    progress.fetch_add(1, Ordering::Relaxed);
                    batch.push(node);
                    if batch.len() >= WALK_BATCH {
                        let _ = sender.send(std::mem::replace(&mut batch, Vec::with_capacity(WALK_BATCH)));
                    }
                };
                let result = if synthetic {
                    walk::walk_volume_synthetic(root, matcher, stop, &mut sink)
                } else {
                    walk::walk_volume(root, matcher, stop, &mut sink)
                };
                if result.is_err() {
                    failed_flag.store(true, Ordering::SeqCst);
                }
                if !batch.is_empty() {
                    let _ = sender.send(batch);
                }
                result
            });
            let slot = store.replace_volume_checked(meta, receiver.into_iter().flatten(), || {
                !failed.load(Ordering::SeqCst)
            });
            (slot, walker.join())
        });
        let walked = walked.map_err(|_| "the walk thread failed".to_string())?;
        match (slot, walked) {
            (Some(slot), Ok(stats)) => {
                log_walk("volume", &ctl.mount, stats.entries, started.elapsed());
                eprintln!(
                    "[Prism Catalog] walked {}: {} entries, {} folders, {} not read, {} ms",
                    ctl.mount_text, stats.entries, stats.folders, stats.denied, stats.ms
                );
                Ok(Some(slot))
            }
            (_, Err(_)) if ctl.stopped() => Ok(None),
            (_, Err(error)) => Err(error),
            (None, Ok(_)) => Err("the walk did not finish".to_string()),
        }
    }

    fn volume_meta(&self, ctl: &VolumeCtl, backend: VolumeBackend, root_record: u64) -> VolumeMeta {
        VolumeMeta {
            volume_id: ctl.volume_id.clone(),
            mount_path: ctl.mount_text.clone(),
            root_record,
            backend,
            journal_id: 0,
            next_usn: 0,
            exclusions: self.exclusions().fingerprint,
        }
    }

    // -----------------------------------------------------------------------
    // Walk volumes
    // -----------------------------------------------------------------------

    /// A volume with no journal: walk now, and again every 30 minutes.
    fn walk_loop(&self, ctl: &VolumeCtl) {
        loop {
            let result = {
                let _build = BuildGuard::new(&self.core.builds, ctl);
                let meta = self.volume_meta(ctl, VolumeBackend::Walk, 0);
                self.walk_into_store(ctl, meta, true)
            };
            let wait = match result {
                Ok(Some(_)) => {
                    self.finish_build(ctl, VolumeState::Ready, true);
                    WALK_VOLUME_INTERVAL
                }
                Ok(None) => return,
                Err(error) => {
                    eprintln!("[Prism Catalog] walk of {} failed: {error}", ctl.mount_text);
                    self.finish_build(ctl, VolumeState::Error, true);
                    WALK_RETRY_INTERVAL
                }
            };
            if !ctl.sleep(wait) {
                return;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Journal volumes
    // -----------------------------------------------------------------------

    /// A volume with a journal: build or catch up, then read the journal.
    fn journal_loop(&self, ctl: &VolumeCtl, journal: &JournalVolume, force: bool) {
        let mut force = force;
        let mut backoff = BACKOFF_START;
        loop {
            if ctl.stopped() {
                return;
            }
            let started = match self.journal_start(ctl, journal, force) {
                Ok(Some(started)) => started,
                Ok(None) => return,
                Err(JournalError::NotActive(message)) => {
                    // The journal was deleted: this volume can only be walked.
                    eprintln!("[Prism Catalog] {}: {message}", ctl.mount_text);
                    self.walk_loop(ctl);
                    return;
                }
                Err(error) => {
                    eprintln!("[Prism Catalog] cannot start {}: {error}", ctl.mount_text);
                    self.finish_build(ctl, VolumeState::Error, false);
                    if !ctl.sleep(backoff) {
                        return;
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    continue;
                }
            };
            backoff = BACKOFF_START;
            self.finish_build(ctl, VolumeState::Ready, started.rebuilt);
            match self.read_loop(ctl, journal, &started) {
                LoopEnd::Stopped => return,
                // The history is lost: the next start walks the volume.
                LoopEnd::Rebuild => force = true,
            }
        }
    }

    /// Chooses where the journal reader starts. A saved checkpoint that the
    /// journal still holds is used. Any other case walks the volume: the USN
    /// is recorded BEFORE the walk, so the reader replays what changed
    /// during the walk.
    fn journal_start(
        &self,
        ctl: &VolumeCtl,
        journal: &JournalVolume,
        force: bool,
    ) -> Result<Option<JournalStart>, JournalError> {
        let query = journal.query()?;
        let store = self.store();
        let fingerprint = self.exclusions().fingerprint;
        let stored = if force {
            None
        } else {
            store.volumes().into_iter().find(|meta| {
                meta.volume_id == ctl.volume_id
                    && meta.backend == VolumeBackend::Journal
                    && meta.mount_path.eq_ignore_ascii_case(&ctl.mount_text)
                    && meta.exclusions == fingerprint
            })
        };
        let checkpoint = stored
            .as_ref()
            .map(|meta| JournalCheckpoint { journal_id: meta.journal_id, next_usn: meta.next_usn });
        match journal_continuity(checkpoint, query) {
            JournalContinuity::Current | JournalContinuity::CatchUp => {
                let (Some(meta), Some(slot)) = (stored, store.volume_slot(&ctl.volume_id)) else {
                    return Err(JournalError::Failed("the saved volume is missing".to_string()));
                };
                Ok(Some(JournalStart {
                    slot,
                    journal_id: meta.journal_id,
                    usn: meta.next_usn,
                    rebuilt: false,
                }))
            }
            JournalContinuity::Rebuild(reason) => {
                eprintln!("[Prism Catalog] walking {} ({reason:?})", ctl.mount_text);
                let _build = BuildGuard::new(&self.core.builds, ctl);
                let root = walk::root_record(&ctl.mount).map_err(JournalError::Failed)?;
                let mut meta = self.volume_meta(ctl, VolumeBackend::Journal, root);
                meta.journal_id = query.journal_id;
                meta.next_usn = query.next_usn;
                let slot = self
                    .walk_into_store(ctl, meta, false)
                    .map_err(JournalError::Failed)?;
                Ok(slot.map(|slot| JournalStart {
                    slot,
                    journal_id: query.journal_id,
                    usn: query.next_usn,
                    rebuilt: true,
                }))
            }
        }
    }

    /// Reads the journal and applies it, until the thread stops or the
    /// journal loses its history. The cursor is local. The store gets it with
    /// each batch that has records, and at most every `CURSOR_PUBLISH_GAP`
    /// otherwise (and when the loop stops).
    fn read_loop(&self, ctl: &VolumeCtl, journal: &JournalVolume, start: &JournalStart) -> LoopEnd {
        let store = self.store();
        let exclusions = self.exclusions();
        let (slot, journal_id) = (start.slot, start.journal_id);
        let mut cursor = start.usn;
        let mut published = cursor;
        let mut last_publish = Instant::now();
        let mut buffer: Vec<u8> = Vec::new();
        let mut backoff = BACKOFF_START;
        let mut unannounced = false;
        let mut last_emit = Instant::now();
        loop {
            if ctl.stopped() {
                if cursor != published {
                    self.publish_cursor(&store, slot, journal_id, cursor);
                }
                return LoopEnd::Stopped;
            }
            match journal.read_with(&mut buffer, cursor, journal_id) {
                Ok(batch) => {
                    if batch.records.is_empty() {
                        backoff = BACKOFF_START;
                        cursor = batch.next_usn;
                        if cursor != published && last_publish.elapsed() >= CURSOR_PUBLISH_GAP {
                            self.publish_cursor(&store, slot, journal_id, cursor);
                            published = cursor;
                            last_publish = Instant::now();
                        }
                        if unannounced && last_emit.elapsed() >= EMIT_GAP {
                            unannounced = false;
                            last_emit = Instant::now();
                            self.emit_updated();
                        }
                        // Poll: a stop ends the pause, and the loop handles it.
                        ctl.sleep(EMPTY_READ_PAUSE);
                        continue;
                    }
                    let accepts = |record: u64| store.accepts_children(slot, record);
                    let changes = journal.changes(&batch, &accepts, &exclusions.matcher);
                    let folders = folders_to_walk(&changes, &accepts);
                    if folders.is_empty() {
                        store.apply_and_checkpoint(slot, &changes, journal_id, batch.next_usn);
                    } else {
                        // The cursor moves only after the walks end well. When
                        // a walk fails or stops, the folders go out of the
                        // store again and the next read gives the batch
                        // again: the folders are new then, and are walked.
                        let _walking = WalkGuard::new(&self.core.walks_pending);
                        store.apply(slot, &changes);
                        if !self.walk_new_folders(ctl, &store, slot, &folders, &changes, &exclusions.matcher) {
                            if ctl.stopped() {
                                continue;
                            }
                            eprintln!(
                                "[Prism Catalog] a new folder of {} was not walked, trying again",
                                ctl.mount_text
                            );
                            if !ctl.sleep(backoff) {
                                continue;
                            }
                            backoff = (backoff * 2).min(BACKOFF_MAX);
                            continue;
                        }
                        store.advance_checkpoint(slot, journal_id, batch.next_usn);
                    }
                    backoff = BACKOFF_START;
                    self.core.cursor_moved.store(true, Ordering::SeqCst);
                    cursor = batch.next_usn;
                    published = cursor;
                    last_publish = Instant::now();
                    unannounced = true;
                    if last_emit.elapsed() >= EMIT_GAP {
                        unannounced = false;
                        last_emit = Instant::now();
                        self.emit_updated();
                    }
                }
                Err(JournalError::HistoryLost(message) | JournalError::NotActive(message)) => {
                    eprintln!("[Prism Catalog] {}: {message}", ctl.mount_text);
                    return LoopEnd::Rebuild;
                }
                Err(JournalError::Failed(message)) => {
                    eprintln!("[Prism Catalog] journal read of {} failed: {message}", ctl.mount_text);
                    if !ctl.sleep(backoff) {
                        return LoopEnd::Stopped;
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }

    /// Gives the cursor to the store. The next save keeps it, with or
    /// without an entry change.
    fn publish_cursor(&self, store: &FileStore, slot: VolumeSlot, journal_id: u64, cursor: i64) {
        store.advance_checkpoint(slot, journal_id, cursor);
        self.core.cursor_moved.store(true, Ordering::SeqCst);
    }

    /// Walks the new `folders` of a batch whose `changes` are applied. Returns
    /// false when a walk was stopped: the folders from that one on are
    /// removed from the store again (a walk applies its contents only when it
    /// ends well).
    fn walk_new_folders(
        &self,
        ctl: &VolumeCtl,
        store: &FileStore,
        slot: VolumeSlot,
        folders: &[u64],
        changes: &[FsChange],
        exclusions: &Exclusions,
    ) -> bool {
        for (index, &record) in folders.iter().enumerate() {
            if self.walk_new_folder(ctl, store, slot, record, exclusions) {
                continue;
            }
            let undo: Vec<FsChange> = changes
                .iter()
                .filter_map(|change| match change {
                    FsChange::Upsert(node) if folders[index..].contains(&node.record) => {
                        Some(FsChange::Remove { record: node.record, seq: node.seq })
                    }
                    _ => None,
                })
                .collect();
            store.apply(slot, &undo);
            return false;
        }
        true
    }

    /// A folder became visible with its contents (moved in, or an excluded
    /// folder renamed): the journal has no record for the contents, so the
    /// folder is walked and its subtree applied. Returns false only when the walk
    /// was stopped.
    fn walk_new_folder(
        &self,
        ctl: &VolumeCtl,
        store: &FileStore,
        slot: VolumeSlot,
        record: u64,
        exclusions: &Exclusions,
    ) -> bool {
        let Some(path) = store.folder_path(slot, record) else { return true };
        let started = Instant::now();
        let mut nodes = Vec::new();
        // One thread: a build makes thousands of new folders, and most hold a
        // few entries. Three threads per folder would cost more than the walk.
        let result = walk::walk_volume_threads(
            Path::new(&path),
            exclusions,
            &ctl.stop,
            1,
            false,
            &mut |node| nodes.push(node),
        );
        let stats = match result {
            Ok(stats) => stats,
            // Stopped (exit, rebuild): the caller reads the batch again.
            Err(_) if ctl.stopped() => return false,
            // Any other error does not go away on a retry (the folder is gone
            // again, or it cannot be read): a retry would hold the reader on
            // this batch. The folder entry stays; a full walk reads it later.
            Err(error) => {
                eprintln!("[Prism Catalog] walk of new folder {path} failed: {error}");
                return true;
            }
        };
        let changes = order_subtree(record, nodes);
        for chunk in changes.chunks(APPLY_CHUNK) {
            store.apply(slot, chunk);
        }
        log_walk("folder", Path::new(&path), stats.entries, started.elapsed());
        true
    }
}

/// Where a journal reader starts.
struct JournalStart {
    slot: VolumeSlot,
    journal_id: u64,
    usn: i64,
    /// The volume was walked now (the snapshot has to be saved).
    rebuilt: bool,
}

/// The mount path with a trailing backslash.
fn mount_text(mount: &Path) -> String {
    let text = mount.to_string_lossy();
    format!("{}\\", text.trim_end_matches(['\\', '/']))
}

/// Starts the file index in the background: loads the snapshot, then builds
/// or updates every volume and keeps them current. Never on the startup path.
pub fn warm(index: FileIndex, app_data_dir: PathBuf) {
    index.core.warming.store(true, Ordering::SeqCst);
    crate::runtime::spawn(async move {
        let init_index = index.clone();
        let _ = crate::runtime::spawn_blocking(move || init_index.init(&app_data_dir)).await;

        loop {
            let volumes = crate::runtime::spawn_blocking(volume::discover_volumes)
                .await
                .unwrap_or_default();
            let reconcile_index = index.clone();
            let _ = crate::runtime::spawn_blocking(move || reconcile_index.reconcile_volumes(volumes, false)).await;
            index.core.warming.store(false, Ordering::SeqCst);

            if index.snapshot_due() {
                let save_index = index.clone();
                let _ = crate::runtime::spawn_blocking(move || save_index.save_snapshot("timer", false)).await;
            }
            tokio::time::sleep(VOLUME_DISCOVERY_INTERVAL).await;
        }
    });
}

pub fn quick_access() -> Vec<QuickAccessEntry> {
    known_locations()
        .into_iter()
        .filter(|(_, path, _)| path.is_dir())
        .map(|(name, path, kind)| QuickAccessEntry {
            name: name.to_string(),
            path: path.to_string_lossy().into_owned(),
            kind: kind.to_string(),
        })
        .collect()
}

pub(super) fn known_locations() -> Vec<(&'static str, PathBuf, &'static str)> {
    [
        ("Home", &FOLDERID_Profile, "home"),
        ("Desktop", &FOLDERID_Desktop, "desktop"),
        ("Downloads", &FOLDERID_Downloads, "downloads"),
        ("Documents", &FOLDERID_Documents, "documents"),
        ("Pictures", &FOLDERID_Pictures, "pictures"),
        ("Music", &FOLDERID_Music, "music"),
        ("Videos", &FOLDERID_Videos, "videos"),
    ]
    .into_iter()
    .filter_map(|(name, id, kind)| known_folder(id).map(|path| (name, path, kind)))
    .collect()
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let path = SHGetKnownFolderPath(id, Default::default(), None).ok()?;
        let text = path.to_string().unwrap_or_default();
        CoTaskMemFree(Some(path.as_ptr() as *const c_void));
        if text.is_empty() {
            None
        } else {
            Some(PathBuf::from(text))
        }
    }
}

const THUMBNAIL_MAX_BYTES: u64 = 32 * 1024 * 1024;
const THUMBNAIL_SIZE: u32 = 64;
/// The preview is only 64px, but `image::decode` first allocates a full bitmap
/// sized by the header. Cap the declared dimensions so a small decompression
/// bomb (huge width/height in a tiny file) cannot force a large allocation
/// before the preview is produced. Mirrors the custom-icon decoder.
const THUMBNAIL_MAX_EDGE: u32 = 8_192;
const THUMBNAIL_MAX_PIXELS: u64 = 16_777_216; // 8_192 * 2_048
/// Bounds the per-request decode work; callers split longer lists into
/// batches of this size.
pub const THUMBNAIL_BATCH_CAP: usize = 64;

fn thumbnail_dimensions_within_limit(width: u32, height: u32) -> bool {
    width != 0
        && height != 0
        && width.max(height) <= THUMBNAIL_MAX_EDGE
        && u64::from(width) * u64::from(height) <= THUMBNAIL_MAX_PIXELS
}

fn file_thumbnail(path: &str) -> Option<String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.is_dir() {
        return None;
    }
    image_thumbnail(path, metadata.len())
}

/// Thumbnails for a batch of paths, returned in input order. One IPC round
/// trip per result page replaces a per-file request storm.
pub fn file_thumbnails(paths: Vec<String>) -> Vec<Option<String>> {
    paths
        .into_iter()
        .take(THUMBNAIL_BATCH_CAP)
        .map(|path| file_thumbnail(&path))
        .collect()
}

fn image_thumbnail(path: &Path, len: u64) -> Option<String> {
    let extension = path.extension()?.to_string_lossy().to_lowercase();
    if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
    ) {
        return None;
    }
    if len > THUMBNAIL_MAX_BYTES {
        return None;
    }
    // Probe the declared dimensions from the header before decoding: the full
    // decode allocates a bitmap sized by those dimensions, so reject anything
    // outside the cap before that allocation happens.
    let (width, height) = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    if !thumbnail_dimensions_within_limit(width, height) {
        return None;
    }
    let image = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let preview = image.thumbnail(THUMBNAIL_SIZE, THUMBNAIL_SIZE);
    let mut bytes = Cursor::new(Vec::new());
    preview.write_to(&mut bytes, image::ImageFormat::Png).ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the repo's `target/tmp` (the shell cannot write to `%TEMP%`).
    fn data_dir(label: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("prism-index-test-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn node(record: u64, parent: u64, name: &str, is_dir: bool, excluded: bool) -> FsNode {
        FsNode { record, seq: 1, parent_record: parent, name: name.into(), is_dir, excluded, link: false }
    }

    fn folder(record: u64, parent: u64) -> FsChange {
        FsChange::Upsert(node(record, parent, "f", true, false))
    }

    // --- exclusion file ---

    #[test]
    fn the_exclusion_file_text_reads_back_as_the_same_items() {
        let items = vec![
            "node_modules".to_string(),
            r"C:\Work\cache".to_string(),
            r"<home>\.cargo\registry".to_string(),
        ];
        assert_eq!(parse_exclusion_list(&format_exclusion_file(&items)), items);
    }

    #[test]
    fn comments_blank_lines_spaces_and_a_bom_are_ignored() {
        let text = "\u{feff}# header\n\n  .git  \r\n   # indented comment\nnode_modules\n\n";
        assert_eq!(parse_exclusion_list(text), vec![".git", "node_modules"]);
        assert!(parse_exclusion_list("# only a comment\n").is_empty());
    }

    #[test]
    fn a_missing_exclusion_file_is_written_with_the_default_list() {
        let dir = data_dir("excl-default");
        let items = load_exclusion_list(&dir);
        assert_eq!(items, Exclusions::default_entries(&dir));
        let text = std::fs::read_to_string(dir.join(EXCLUSIONS_FILE)).unwrap();
        assert!(text.starts_with("# Prism file index"));
        // The second start reads the file back: the same list.
        assert_eq!(load_exclusion_list(&dir), items);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_edited_exclusion_file_replaces_the_default_list() {
        let dir = data_dir("excl-edited");
        let _ = load_exclusion_list(&dir);
        std::fs::write(dir.join(EXCLUSIONS_FILE), "# mine\nbuild\n").unwrap();
        assert_eq!(load_exclusion_list(&dir), vec!["build"]);
        // An empty list is a list: nothing is excluded.
        std::fs::write(dir.join(EXCLUSIONS_FILE), "# nothing\n").unwrap();
        assert!(load_exclusion_list(&dir).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_fingerprint_ignores_order_and_case_and_sees_a_change() {
        let a = vec!["Node_Modules".to_string(), ".git".to_string()];
        let b = vec![".GIT".to_string(), "node_modules".to_string()];
        assert_eq!(exclusion_fingerprint(&a), exclusion_fingerprint(&b));
        let c = vec![".git".to_string()];
        assert_ne!(exclusion_fingerprint(&a), exclusion_fingerprint(&c));
        assert_ne!(exclusion_fingerprint(&c), exclusion_fingerprint(&[]));
    }

    // --- newly visible folders ---

    #[test]
    fn a_folder_that_was_not_shown_before_is_walked() {
        let changes = vec![folder(10, 5)];
        assert_eq!(folders_to_walk(&changes, &|_| false), vec![10]);
        // A folder that was a live, shown folder (a plain rename) is not.
        assert!(folders_to_walk(&changes, &|record| record == 10).is_empty());
    }

    #[test]
    fn files_removes_and_excluded_folders_are_not_walked() {
        let changes = vec![
            FsChange::Upsert(node(10, 5, "a.txt", false, false)),
            FsChange::Upsert(node(11, 5, "node_modules", true, true)),
            FsChange::Remove { record: 12, seq: 1 },
        ];
        assert!(folders_to_walk(&changes, &|_| false).is_empty());
    }

    #[test]
    fn a_new_junction_or_symlink_folder_is_not_walked() {
        let link = FsChange::Upsert(FsNode { link: true, ..node(10, 5, "x", true, false) });
        assert!(folders_to_walk(&[link, folder(11, 5)], &|_| false) == vec![11]);
    }

    #[test]
    fn a_folder_below_another_new_folder_is_left_to_the_walk_of_its_parent() {
        let changes = vec![folder(10, 5), folder(11, 10), folder(12, 11), folder(20, 5)];
        assert_eq!(folders_to_walk(&changes, &|_| false), vec![10, 20]);
        // When the parent was shown before, the child needs its own walk.
        assert_eq!(folders_to_walk(&changes, &|record| record == 10), vec![11, 20]);
    }

    #[test]
    fn a_walked_subtree_is_ordered_with_each_folder_before_its_contents() {
        let nodes = vec![
            node(32, 31, "deep.txt", false, false),
            node(31, 30, "inner", true, false),
            node(33, 30, "skip", true, true),
            node(30, 99, "stray", false, false),
            node(34, 30, "top.txt", false, false),
        ];
        let ordered = order_subtree(30, nodes);
        let records: Vec<u64> = ordered
            .iter()
            .map(|change| match change {
                FsChange::Upsert(node) => node.record,
                FsChange::Remove { record, .. } => *record,
            })
            .collect();
        // The node whose parent is not below the root (30 under 99) is dropped.
        assert_eq!(records.len(), 4);
        let at = |record: u64| records.iter().position(|&held| held == record).unwrap();
        assert!(at(31) < at(32));
        assert!(!records.contains(&30));
    }

    // --- migration ---

    #[test]
    fn the_old_catalog_files_are_deleted_and_other_files_stay() {
        let dir = data_dir("legacy");
        for name in ["catalog.db", "catalog.db-wal", "catalog.db-shm", "file-index.bin", "files.json"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        assert_eq!(delete_legacy_catalog(&dir), 3);
        for name in LEGACY_CATALOG_FILES {
            assert!(!dir.join(name).exists(), "{name}");
        }
        assert!(dir.join("file-index.bin").exists());
        assert!(dir.join("files.json").exists());
        // Nothing left to delete, and a missing folder is not an error.
        assert_eq!(delete_legacy_catalog(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(delete_legacy_catalog(&dir), 0);
    }

    // --- FileIndex ---

    fn start(dir: &Path) -> FileIndex {
        let index = FileIndex::default();
        index.init(dir);
        index
    }

    #[test]
    fn a_start_with_no_snapshot_has_an_empty_store_and_writes_the_exclusion_file() {
        let dir = data_dir("no-snapshot");
        let index = start(&dir);
        let (volumes, total) = index.status(&index.store());
        assert!(volumes.is_empty());
        assert_eq!(total, 0);
        assert!(!index.search_with_generation("plan", None, 0).ready);
        assert!(dir.join(EXCLUSIONS_FILE).exists());
        // An empty store is never written over a snapshot.
        index.save_snapshot("test", false);
        assert!(!dir.join(SNAPSHOT_FILE).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_saved_snapshot_is_loaded_at_the_next_start() {
        let dir = data_dir("snapshot");
        let first = start(&dir);
        let meta = VolumeMeta {
            volume_id: "vol-test".into(),
            mount_path: r"C:\".into(),
            root_record: 5,
            backend: VolumeBackend::Journal,
            journal_id: 3,
            next_usn: 40,
            exclusions: first.exclusions().fingerprint,
        };
        first.store().replace_volume(
            meta,
            vec![node(6, 5, "docs", true, false), node(7, 6, "plan.txt", false, false)],
        );
        first.save_snapshot("test", false);
        assert!(dir.join(SNAPSHOT_FILE).exists());
        drop(first);

        let second = start(&dir);
        let (_, total) = second.status(&second.store());
        assert_eq!(total, 2);
        let volumes = second.store().volumes();
        assert_eq!((volumes[0].journal_id, volumes[0].next_usn), (3, 40));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_bad_snapshot_gives_an_empty_store() {
        let dir = data_dir("bad-snapshot");
        std::fs::write(dir.join(SNAPSHOT_FILE), b"not an index").unwrap();
        let index = start(&dir);
        assert_eq!(index.status(&index.store()).1, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_cursor_that_moved_makes_a_snapshot_due_only_after_an_hour() {
        let index = FileIndex::default();
        let ago = |minutes: u64| Instant::now().checked_sub(Duration::from_secs(minutes * 60));
        let Some(old) = ago(61) else { return };
        let recent = ago(30).unwrap_or_else(Instant::now);
        // Nothing changed: never due.
        *index.core.last_save.lock().unwrap() = old;
        assert!(!index.snapshot_due());
        // The cursor moved: due when the last save is over an hour old.
        index.core.cursor_moved.store(true, Ordering::SeqCst);
        assert!(index.snapshot_due());
        *index.core.last_save.lock().unwrap() = recent;
        assert!(!index.snapshot_due());
    }

    #[test]
    fn the_mount_text_has_one_trailing_backslash() {
        assert_eq!(mount_text(Path::new(r"C:\")), r"C:\");
        assert_eq!(mount_text(Path::new("C:")), r"C:\");
        assert_eq!(mount_text(Path::new(r"D:\Mounts\Disk1")), r"D:\Mounts\Disk1\");
    }
}
