pub mod backend;
pub mod db;
pub mod ntfs;
pub mod scanner;
pub mod search;
pub mod types;
pub mod volume;
pub mod watcher;

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;
use tauri::Emitter;
use tauri::Manager;
use windows::core::GUID;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Profile, FOLDERID_Videos, SHGetKnownFolderPath,
};

pub use types::{FileSearchResponse, QuickAccessEntry, VolumeCoverage, VolumeInfo, VolumeState};

use self::backend::{select_backend, BackendKind};
use self::db::Database;
use self::ntfs::NtfsBackend;
use self::watcher::{watcher_root_changed, VolumeWatcher};

/// Journal catch-up cadence while the palette is visible. This is the
/// freshness window for NTFS-backed file results while the user is searching.
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Journal catch-up cadence while hidden - the launcher's normal state. The
/// poll opens the raw volume device and issues an ioctl per NTFS drive, so the
/// idle cadence dominates the app's steady-state background cost.
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(30);
/// Mount topology (usb plug, network mount) changes are rare; the discovery
/// pass calls GetVolumeInformationW on every volume, which can block for
/// seconds on disconnected network drives. Only re-enumerate this often and
/// reuse the previous snapshot in between.
const VOLUME_DISCOVERY_INTERVAL: Duration = Duration::from_secs(15);
/// Long sleeps are sliced so a hidden -> visible transition resumes the fast
/// cadence (and journal freshness) within a second of opening the palette.
const POLL_WAKE_SLICE: Duration = Duration::from_secs(1);
// Sweeps walk the tree but write only rows that actually changed, so the
// periodic reconcile catches watcher misses cheaply without the disk churn of
// the old full re-index.
const RECONCILIATION_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60); // 6 hours
                                                                            // At startup the persisted index is served directly: volumes scanned within
                                                                            // this window are not re-walked (the watcher keeps them fresh while running,
                                                                            // and results are verified against disk before being shown). Only volumes
                                                                            // that are new, never scanned, or stale get a sweep.
const STARTUP_SWEEP_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

pub(crate) fn catalog_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("PRISM_CATALOG_DEBUG")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// In-memory per-volume file counts. Seeded from the database at startup,
/// updated by scan progress and watcher deltas, and re-seeded from
/// `scanned_entries` after every completed scan. Keeps every status path O(1)
/// - counting millions of rows on every palette open was a major stall.
#[derive(Clone, Default)]
pub struct IndexCounts {
    inner: Arc<Mutex<HashMap<String, u64>>>,
}

#[derive(Default)]
struct ScopeRepairCoordinator {
    /// Presence means one worker owns the scope; true requests another pass.
    running: HashMap<String, bool>,
}

impl ScopeRepairCoordinator {
    fn request(&mut self, key: String) -> bool {
        if let Some(pending) = self.running.get_mut(&key) {
            *pending = true;
            false
        } else {
            self.running.insert(key, false);
            true
        }
    }

    fn finish_pass(&mut self, key: &str) -> bool {
        match self.running.get_mut(key) {
            Some(pending) if *pending => {
                *pending = false;
                true
            }
            Some(_) => {
                self.running.remove(key);
                false
            }
            None => false,
        }
    }
}

#[derive(Default)]
struct SourceWorkerCoordinator {
    /// Presence means a worker is active. Some(value) requests another pass;
    /// false wins because a forced rebuild cannot be weakened by a fresh pass.
    running: HashMap<String, Option<bool>>,
}

impl SourceWorkerCoordinator {
    fn request(&mut self, volume_id: String, fresh_ok: bool) -> bool {
        if let Some(pending) = self.running.get_mut(&volume_id) {
            *pending = Some(pending.unwrap_or(true) && fresh_ok);
            false
        } else {
            self.running.insert(volume_id, None);
            true
        }
    }

    fn finish_pass(&mut self, volume_id: &str) -> Option<bool> {
        let pending = self.running.get_mut(volume_id)?.take();
        if pending.is_none() {
            self.running.remove(volume_id);
        }
        pending
    }

    fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    fn is_running(&self, volume_id: &str) -> bool {
        self.running.contains_key(volume_id)
    }
}

impl IndexCounts {
    pub fn set(&self, drive: &str, count: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.insert(drive.to_string(), count);
        }
    }

    pub fn adjust(&self, drive: &str, delta: i64) {
        if let Ok(mut inner) = self.inner.lock() {
            let entry = inner.entry(drive.to_string()).or_insert(0);
            *entry = (*entry as i64 + delta).max(0) as u64;
        }
    }

    pub fn get(&self, drive: &str) -> u64 {
        self.inner
            .lock()
            .ok()
            .and_then(|m| m.get(drive).copied())
            .unwrap_or(0)
    }

    pub fn total(&self) -> u64 {
        self.inner.lock().map(|m| m.values().sum()).unwrap_or(0)
    }

    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.clear();
        }
    }
}

#[derive(Clone)]
pub struct FileIndex {
    db: Arc<RwLock<Option<Arc<Database>>>>,
    search_generation: Arc<AtomicU64>,
    volumes: Arc<RwLock<Vec<VolumeCoverage>>>,
    indexing: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
    counts: IndexCounts,
    watchers: Arc<Mutex<HashMap<String, VolumeWatcher>>>,
    scan_cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    app_data_dir: Arc<RwLock<PathBuf>>,
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
    scan_generation: Arc<AtomicU64>,
    scope_repairs: Arc<Mutex<ScopeRepairCoordinator>>,
    source_workers: Arc<Mutex<SourceWorkerCoordinator>>,
    scan_slots: Arc<tokio::sync::Semaphore>,
}

impl Default for FileIndex {
    fn default() -> Self {
        Self {
            db: Arc::new(RwLock::new(None)),
            search_generation: Arc::new(AtomicU64::new(0)),
            volumes: Arc::new(RwLock::new(Vec::new())),
            indexing: Arc::new(AtomicBool::new(false)),
            ready: Arc::new(AtomicBool::new(false)),
            counts: IndexCounts::default(),
            watchers: Arc::new(Mutex::new(HashMap::new())),
            scan_cancels: Arc::new(Mutex::new(HashMap::new())),
            app_data_dir: Arc::new(RwLock::new(PathBuf::from("."))),
            app_handle: Arc::new(Mutex::new(None)),
            scan_generation: Arc::new(AtomicU64::new(1)),
            scope_repairs: Arc::new(Mutex::new(ScopeRepairCoordinator::default())),
            source_workers: Arc::new(Mutex::new(SourceWorkerCoordinator::default())),
            scan_slots: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }
}

impl FileIndex {
    #[allow(dead_code)]
    pub fn new(app_data_dir: &Path) -> Self {
        let index = Self::default();
        index.init(app_data_dir);
        index
    }

    pub fn init(&self, app_data_dir: &Path) {
        let db_path = app_data_dir.join("catalog.db");
        if let Ok(d) = Database::open(&db_path) {
            let db_arc = Arc::new(d);
            if let Ok(covs) = db_arc.get_volume_coverages() {
                for cov in &covs {
                    self.counts.set(&cov.drive, cov.indexed_count);
                }
                if let Ok(mut vols) = self.volumes.write() {
                    *vols = covs;
                }
            }
            if self.counts.total() > 0 {
                self.ready.store(true, Ordering::SeqCst);
            }
            *self.db.write().unwrap() = Some(db_arc);
        }
        *self.app_data_dir.write().unwrap() = app_data_dir.to_path_buf();
    }

    pub fn search(&self, query: &str, limit: Option<usize>) -> FileSearchResponse {
        let volumes = self.volumes.read().unwrap().clone();
        let total_indexed = self.counts.total();
        let indexing = self.indexing.load(Ordering::Relaxed);
        let ready = self.ready.load(Ordering::Relaxed);

        let db = self.db.read().unwrap().clone();
        let Some(ref db) = db else {
            return search::browse_path(query.trim(), search::clamp_limit(limit))
                .map(|items| FileSearchResponse {
                    items,
                    ready: false,
                    indexing: false,
                    path_browse: true,
                    volumes,
                    total_indexed: 0,
                })
                .unwrap_or_else(|| FileSearchResponse {
                    items: Vec::new(),
                    ready: false,
                    indexing: false,
                    path_browse: false,
                    volumes: Vec::new(),
                    total_indexed: 0,
                });
        };

        search::search(
            query,
            limit,
            db,
            &self.search_generation,
            &volumes,
            total_indexed,
            indexing,
            ready,
        )
    }

    /// Wipes the catalog and rebuilds it from scratch (used by the UI's
    /// "rebuild index" action).
    pub fn rebuild(&self) {
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            this.cancel_all_scans();
            let db = this.db.read().unwrap().clone();
            if let Some(ref db) = db {
                let _ = db.clear_catalog();
            }
            this.counts.clear();
            this.ready.store(false, Ordering::SeqCst);
            this.scan_all_volumes(false).await;
        });
    }

    fn cancel_all_scans(&self) {
        let cancels = self.scan_cancels.lock().unwrap();
        for (_, cancel) in cancels.iter() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn set_app_handle(&self, app: tauri::AppHandle) {
        *self.app_handle.lock().unwrap() = Some(app);
    }

    /// True when the palette window is shown. The background poll loop uses
    /// this to poll journal catch-up fast while the user is searching and
    /// slowly while the launcher sits hidden.
    fn palette_visible(&self) -> bool {
        self.app_handle
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|app| app.get_webview_window("main"))
            .map(|window| window.is_visible().unwrap_or(true))
            .unwrap_or(true)
    }

    fn emit_updated(&self) {
        if let Some(app) = self.app_handle.lock().unwrap().as_ref() {
            let _ = app.emit("file-index-updated", ());
        }
    }

    /// Synchronizes every connected volume. Local fixed NTFS volumes use the
    /// persisted FRN catalog and USN cursor; all other volumes retain the
    /// recursive scanner/watcher fallback. `fresh_ok` only short-circuits the
    /// fallback path.
    async fn scan_all_volumes(&self, fresh_ok: bool) {
        let db_opt = self.db.read().unwrap().clone();
        let Some(ref db) = db_opt else { return };

        let discovered = tauri::async_runtime::spawn_blocking(volume::discover_volumes)
            .await
            .unwrap_or_default();

        // Drop duplicate volume identities left by earlier builds (the same
        // mount path discovered under a GUID id and a serial id): their rows
        // duplicate the real volume's files.
        if let Ok(known) = db.get_volume_ids() {
            for (db_id, db_mount) in known {
                let claimed = discovered.iter().find(|v| {
                    v.normalized_mount_paths().iter().any(|path| {
                        path.as_os_str()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&db_mount)
                    })
                });
                if let Some(vol) = claimed {
                    if vol.volume_id != db_id {
                        eprintln!(
                            "[Prism Catalog] Removing duplicate volume identity {db_id} (claimed by {})",
                            vol.volume_id
                        );
                        let _ = db.remove_volume(&db_id);
                    }
                }
            }
        }

        // Persist discovery and mount changes before scheduling source work.
        // Each source owns its subsequent lifecycle, so a slow share cannot
        // hold back another discovered volume.
        for vol in &discovered {
            let Some(mount_path) = vol.canonical_mount_path() else {
                continue;
            };
            let initial_state = if vol.accessible {
                VolumeState::Indexing
            } else {
                VolumeState::Offline
            };
            let update = db.upsert_volume(vol, initial_state);
            let fresh = vol.accessible
                && fresh_ok
                && db
                    .is_volume_fresh_at_mount(
                        &vol.volume_id,
                        &mount_path,
                        STARTUP_SWEEP_WINDOW.as_secs(),
                    )
                    .unwrap_or(false);
            if fresh {
                let _ = db.set_volume_state(&vol.volume_id, VolumeState::Ready);
            }
            if let Ok(update) = &update {
                if let Some(previous_mount) = &update.previous_mount_path {
                    if !previous_mount.eq_ignore_ascii_case(&mount_path.to_string_lossy()) {
                        if let Some(watcher) = self.watchers.lock().unwrap().remove(&vol.volume_id)
                        {
                            watcher.request_stop();
                        }
                    }
                }
            }
            debug_volume_plan(db, vol, &mount_path, fresh, update.as_ref().ok());
        }

        for volume in discovered.into_iter().filter(|volume| volume.accessible) {
            let volume_id = volume.volume_id.clone();
            let should_start = self
                .source_workers
                .lock()
                .unwrap()
                .request(volume_id.clone(), fresh_ok);
            if !should_start {
                continue;
            }

            let this = self.clone();
            let slots = self.scan_slots.clone();
            tauri::async_runtime::spawn(async move {
                let mut pass_fresh_ok = fresh_ok;
                loop {
                    if let Ok(permit) = slots.clone().acquire_owned().await {
                        let index = this.clone();
                        let pass_volume = volume.clone();
                        let _ = tauri::async_runtime::spawn_blocking(move || {
                            index.scan_one_volume_sync(&pass_volume, pass_fresh_ok);
                        })
                        .await;
                        drop(permit);
                    }

                    let pending = this.source_workers.lock().unwrap().finish_pass(&volume_id);
                    let Some(next_fresh_ok) = pending else {
                        break;
                    };
                    pass_fresh_ok = next_fresh_ok;
                }

                this.refresh_totals_and_status();
                let active = !this.source_workers.lock().unwrap().is_empty();
                this.indexing.store(active, Ordering::SeqCst);
                this.ready.store(true, Ordering::SeqCst);
                this.emit_updated();
            });
        }

        self.refresh_totals_and_status();
        let active = !self.source_workers.lock().unwrap().is_empty();
        self.indexing.store(active, Ordering::SeqCst);
        if !active {
            self.ready.store(true, Ordering::SeqCst);
        }
        self.emit_updated();
    }

    /// The whole per-volume scan is synchronous; the async wrappers only move
    /// it off the runtime threads. Keeping the overflow handler fully sync
    /// (spawn_blocking with no nested future) also avoids the type-level cycle
    /// of an async closure that awaits back into this module's own futures.
    fn scan_one_volume_sync(&self, vol: &VolumeInfo, fresh_ok: bool) {
        let db_opt = self.db.read().unwrap().clone();
        let Some(db) = db_opt else { return };

        let volume_id = vol.volume_id.clone();
        let mount_path = match vol.canonical_mount_path() {
            Some(p) => p.clone(),
            None => return,
        };
        let drive = mount_path.to_string_lossy().into_owned();

        // A watcher means this volume already selected the directory backend
        // for the current process. Do not race it with an NTFS generation
        // swap; raw access will be probed again on the next launch.
        let fallback_active = self.watchers.lock().unwrap().contains_key(&volume_id);
        let mut directory_fallback = select_backend(vol, true) == BackendKind::Directory;
        if !fallback_active && !directory_fallback {
            let had_ready_generation = db.get_ntfs_checkpoint(&volume_id).ok().flatten().is_some();
            match NtfsBackend::open(vol) {
                Ok(mut backend) => {
                    self.update_volume_coverage(&drive, VolumeState::Indexing);
                    match backend.synchronize(vol, &db) {
                        Ok(stats) => {
                            if let Ok(exact) = db.get_scanned_entries(&volume_id) {
                                self.counts.set(&drive, exact);
                            }
                            let _ = db.checkpoint();
                            self.update_volume_coverage(&drive, VolumeState::Ready);
                            debug_ntfs_complete(&db, vol, &drive, stats.records, None);
                            self.emit_updated();
                            return;
                        }
                        Err(error) => {
                            if had_ready_generation {
                                eprintln!(
                                    "[Prism Catalog] NTFS synchronization deferred for {volume_id}; preserving the ready generation: {error}"
                                );
                                let _ = db.mark_ntfs_ready(&volume_id);
                                self.update_volume_coverage(&drive, VolumeState::Ready);
                                debug_ntfs_complete(&db, vol, &drive, 0, Some(&error));
                                return;
                            }
                            eprintln!(
                                "[Prism Catalog] NTFS backend failed for {volume_id}; using directory fallback: {error}"
                            );
                            directory_fallback = true;
                        }
                    }
                }
                Err(error) => {
                    // Raw access can be denied to a normal user. The directory
                    // backend remains fully functional and the UI never needs
                    // to run elevated.
                    if catalog_debug_enabled() {
                        eprintln!(
                            "{}",
                            serde_json::json!({
                                "event": "ntfs_open_failed",
                                "volume_id": volume_id,
                                "mount_path": drive,
                                "error": error,
                                "selected_backend": "directory",
                            })
                        );
                    }
                    directory_fallback = true;
                }
            }
        }

        if directory_fallback {
            // Preserve a valid NTFS generation while the fallback walk builds
            // path rows. finish_volume_scan performs the atomic handover.
            let _ = db.mark_directory_backend(&volume_id);
        }

        let skip_scan = fresh_ok
            && db
                .is_volume_fresh_at_mount(&volume_id, &mount_path, STARTUP_SWEEP_WINDOW.as_secs())
                .unwrap_or(false);

        // Repair only the shard that lost events, on the blocking pool so the
        // watcher thread is never stalled.
        let overflow_index = self.clone();
        let overflow_volume = volume_id.clone();
        let overflow_cb: Arc<dyn Fn(String, PathBuf) + Send + Sync> = Arc::new(move |_, scope| {
            let idx = overflow_index.clone();
            let v_id = overflow_volume.clone();
            idx.request_scope_repair(v_id, scope);
        });
        let update_index = self.clone();
        let update_cb: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            update_index.emit_updated();
        });

        // Start the watcher if needed, otherwise buffer its events so the scan
        // and the watcher never race on the same rows.
        {
            let mut watchers = self.watchers.lock().unwrap();
            let replace = watchers
                .get(&volume_id)
                .is_some_and(|watcher| watcher_root_changed(watcher.root(), &mount_path));
            if replace {
                if let Some(watcher) = watchers.remove(&volume_id) {
                    watcher.request_stop();
                }
            }
            if !watchers.contains_key(&volume_id) {
                let db_clone = db.clone();
                let app_data_clone = self.app_data_dir.read().unwrap().clone();
                let counts_clone = self.counts.clone();
                let drive_clone = drive.clone();
                if let Some(w) = VolumeWatcher::start(
                    mount_path.clone(),
                    volume_id.clone(),
                    db_clone,
                    app_data_clone,
                    counts_clone,
                    drive_clone,
                    update_cb,
                    overflow_cb,
                ) {
                    watchers.insert(volume_id.clone(), w);
                }
            } else if let Some(w) = watchers.get(&volume_id) {
                w.set_buffering(true);
            }
        }

        if skip_scan {
            // The persisted index is fresh: serve it as-is and let the
            // watcher apply events directly (it was just started in buffering
            // mode above). Drain events captured between the last scan and the
            // watcher becoming live before releasing the buffer.
            let watchers = self.watchers.lock().unwrap();
            if let Some(w) = watchers.get(&volume_id) {
                w.flush_queue(&db);
            }
            return;
        }

        self.indexing.store(true, Ordering::SeqCst);
        self.update_volume_coverage(&drive, VolumeState::Indexing);

        let cancel = Arc::new(AtomicBool::new(false));
        {
            self.scan_cancels
                .lock()
                .unwrap()
                .insert(volume_id.clone(), cancel.clone());
        }

        let gen = self.scan_generation.fetch_add(1, Ordering::SeqCst);
        let app_data_scan = self.app_data_dir.read().unwrap().clone();
        let index_for_progress = self.clone();
        let drive_progress = drive.clone();

        let scan_result = scanner::scan_volume(
            &mount_path,
            &volume_id,
            gen,
            db.clone(),
            &app_data_scan,
            cancel,
            move |count| {
                index_for_progress.update_progress(&drive_progress, count);
            },
        );

        match &scan_result {
            Ok(_) => {
                self.update_volume_coverage(&drive, VolumeState::Ready);
            }
            Err(e) => {
                eprintln!("[Prism Catalog] Scan for volume {volume_id} error: {e}");
                let _ = db.set_volume_state(&volume_id, VolumeState::Error);
                self.update_volume_coverage(&drive, VolumeState::Error);
            }
        }

        debug_scan_complete(&db, vol, &drive, &scan_result);

        // Exact count as scanned, then apply buffered watcher deltas on top.
        if let Ok(exact) = db.get_scanned_entries(&volume_id) {
            self.counts.set(&drive, exact);
        }

        // The baseline is complete before queued name events are replayed.
        // A directory addition found during replay may therefore schedule a
        // scoped repair immediately without being mistaken for a competing
        // volume scan.
        self.scan_cancels.lock().unwrap().remove(&volume_id);

        // Flush buffered watcher events (also releases buffering); deltas are
        // applied on top of the exact scanned count.
        {
            let watchers = self.watchers.lock().unwrap();
            if let Some(w) = watchers.get(&volume_id) {
                w.flush_queue(&db);
            }
        }

        if let Ok(mut vols) = self.volumes.write() {
            if let Some(v) = vols
                .iter_mut()
                .find(|v| v.drive.eq_ignore_ascii_case(&drive))
            {
                v.indexed_count = self.counts.get(&drive);
            }
        }

        // The sweep (and its FTS rebuild) left most of the WAL behind; fold
        // it back into the catalog so the next launch does not pay recovery.
        let _ = db.checkpoint();
        self.emit_updated();
    }

    async fn catch_up_ntfs_volumes(&self, volumes: &[VolumeInfo]) {
        for volume in volumes {
            if select_backend(volume, true) != BackendKind::Ntfs
                || self
                    .source_workers
                    .lock()
                    .unwrap()
                    .is_running(&volume.volume_id)
                || self
                    .watchers
                    .lock()
                    .unwrap()
                    .contains_key(&volume.volume_id)
            {
                continue;
            }
            let this = self.clone();
            // The blocking task must own its volume (one small clone per NTFS
            // drive per poll tick).
            let volume = volume.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let Some(db) = this.db.read().unwrap().clone() else {
                    return;
                };
                let mount = volume
                    .canonical_mount_path()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Ok(mut backend) = NtfsBackend::open(&volume) {
                    if backend.synchronize(&volume, &db).is_ok() {
                        if let Ok(count) = db.get_scanned_entries(&volume.volume_id) {
                            this.counts.set(&mount, count);
                        }
                    }
                }
            })
            .await;
        }
        self.refresh_totals_and_status();
    }

    fn request_scope_repair(&self, volume_id: String, scope: PathBuf) {
        let key = format!("{}|{}", volume_id, scope.to_string_lossy().to_lowercase());
        if !self.scope_repairs.lock().unwrap().request(key.clone()) {
            return;
        }

        let index = self.clone();
        tauri::async_runtime::spawn_blocking(move || loop {
            index.reconcile_scope_sync(&volume_id, &scope);
            if !index.scope_repairs.lock().unwrap().finish_pass(&key) {
                break;
            }
        });
    }

    fn reconcile_scope_sync(&self, volume_id: &str, requested_scope: &Path) {
        let Some(db) = self.db.read().unwrap().clone() else {
            return;
        };
        let (volume_root, attached) = {
            let watchers = self.watchers.lock().unwrap();
            let Some(watcher) = watchers.get(volume_id) else {
                return;
            };
            (
                watcher.root().to_path_buf(),
                watcher.ensure_current_shards(),
            )
        };

        if requested_scope != volume_root && !requested_scope.starts_with(&volume_root) {
            return;
        }

        let mut scopes: HashSet<PathBuf> = attached.into_iter().collect();
        scopes.insert(requested_scope.to_path_buf());
        let app_data_dir = self.app_data_dir.read().unwrap().clone();
        for scope in scopes {
            let generation = self.scan_generation.fetch_add(1, Ordering::SeqCst);
            let cancel = Arc::new(AtomicBool::new(false));
            let result = if scope == volume_root {
                scanner::reconcile_directory(
                    &scope,
                    volume_id,
                    generation,
                    db.clone(),
                    &app_data_dir,
                    cancel,
                )
            } else {
                scanner::reconcile_scope(
                    &scope,
                    volume_id,
                    generation,
                    db.clone(),
                    &app_data_dir,
                    cancel,
                )
            };
            if let Err(error) = result {
                eprintln!(
                    "[Prism Catalog] Scoped repair for {} failed: {}",
                    scope.display(),
                    error
                );
            }
        }
        let _ = db.checkpoint();
        self.emit_updated();
    }

    fn update_progress(&self, drive: &str, count: u64) {
        // Progress only ever grows here; a sweep of a few changed directories
        // must not knock the displayed count back down. The exact count is
        // restored from the database when the scan finishes.
        let current = self.counts.get(drive);
        if count > current {
            self.counts.set(drive, count);
        }
        if let Ok(mut vols) = self.volumes.write() {
            for v in vols.iter_mut() {
                if v.drive.eq_ignore_ascii_case(drive) {
                    v.indexed_count = v.indexed_count.max(count);
                }
            }
        }
    }

    fn update_volume_coverage(&self, drive: &str, state: VolumeState) {
        if let Ok(mut vols) = self.volumes.write() {
            if let Some(v) = vols
                .iter_mut()
                .find(|v| v.drive.eq_ignore_ascii_case(drive))
            {
                v.state = state;
            } else {
                vols.push(VolumeCoverage {
                    drive: drive.to_string(),
                    state,
                    indexed_count: self.counts.get(drive),
                    total_progress: None,
                });
            }
        }
    }

    fn refresh_totals_and_status(&self) {
        let db_opt = self.db.read().unwrap().clone();
        if let Some(ref db) = db_opt {
            if let Ok(covs) = db.get_volume_coverages() {
                self.counts.clear();
                for coverage in &covs {
                    self.counts.set(&coverage.drive, coverage.indexed_count);
                }
                if let Ok(mut vols) = self.volumes.write() {
                    *vols = covs;
                }
            }
        }
    }

    /// Migrates temporary bootstrap entries from old files.json if present
    fn try_migrate_legacy_cache(&self, legacy_cache_path: &Path) {
        let db_opt = self.db.read().unwrap().clone();
        let Some(ref db) = db_opt else { return };
        if let Ok(total) = db.get_total_indexed_count() {
            if total > 0 {
                return; // Already has indexed data
            }
        }

        if !legacy_cache_path.exists() {
            return;
        }

        if let Ok(text) = std::fs::read_to_string(legacy_cache_path) {
            #[derive(Deserialize)]
            struct OldCache {
                entries: Vec<OldEntry>,
            }
            #[derive(Deserialize)]
            struct OldEntry {
                p: String,
                #[serde(default)]
                d: bool,
            }

            if let Ok(old) = serde_json::from_str::<OldCache>(&text) {
                let items: Vec<types::ScannedItem> = old
                    .entries
                    .into_iter()
                    .filter_map(|e| {
                        let path = Path::new(&e.p);
                        let name = path.file_name()?.to_string_lossy().into_owned();
                        let lower_name = name.to_lowercase();
                        let parent = path
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let extension = if e.d {
                            None
                        } else {
                            path.extension()
                                .map(|ext| ext.to_string_lossy().to_lowercase())
                        };

                        Some(types::ScannedItem {
                            normalized_path: e.p.to_lowercase(),
                            display_path: e.p,
                            name,
                            lower_name,
                            parent,
                            is_directory: e.d,
                            extension,
                            modified_at: 0,
                            size: 0,
                        })
                    })
                    .collect();

                let _ = db.insert_batch("C", 0, &items);
            }
        }
    }
}

pub fn warm(index: FileIndex, app_data_dir: PathBuf, app: tauri::AppHandle) {
    index.set_app_handle(app.clone());
    // Open the persisted catalog. This was previously only reachable through
    // the dead `FileIndex::new` helper - without it the index is never loaded
    // or built and filename searches return nothing.
    index.init(&app_data_dir);
    let legacy_cache_path = app_data_dir.join("files.json");
    index.try_migrate_legacy_cache(&legacy_cache_path);

    tauri::async_runtime::spawn(async move {
        // Startup: serve the persisted index directly. Volumes whose index is
        // fresh are not re-walked (the watcher keeps them live), so a normal
        // launch has results immediately - no indexing phase.
        index.scan_all_volumes(true).await;

        let mut last_reconcile = tokio::time::Instant::now();
        let mut last_discovery = tokio::time::Instant::now();
        // Seed the known set so the first poll does not immediately re-sweep.
        let mut current_volumes = tauri::async_runtime::spawn_blocking(volume::discover_volumes)
            .await
            .unwrap_or_default();
        let mut known_topology = volume_topology_fingerprint(&current_volumes);

        loop {
            // Fast cadence while the palette is open (search freshness), slow
            // cadence while hidden (steady-state background cost). The sleep is
            // sliced so showing the palette resumes the fast cadence within a
            // second instead of after the full idle interval.
            let visible = index.palette_visible();
            let deadline = tokio::time::Instant::now()
                + if visible {
                    ACTIVE_POLL_INTERVAL
                } else {
                    IDLE_POLL_INTERVAL
                };
            while tokio::time::Instant::now() < deadline {
                let slice = deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .min(POLL_WAKE_SLICE);
                if slice.is_zero() {
                    break;
                }
                tokio::time::sleep(slice).await;
                if index.palette_visible() != visible {
                    break;
                }
            }

            if last_discovery.elapsed() >= VOLUME_DISCOVERY_INTERVAL {
                last_discovery = tokio::time::Instant::now();
                current_volumes = tauri::async_runtime::spawn_blocking(volume::discover_volumes)
                    .await
                    .unwrap_or_default();

                let current_topology = volume_topology_fingerprint(&current_volumes);
                let volume_set_changed = current_topology != known_topology;
                let reconcile_due = last_reconcile.elapsed() >= RECONCILIATION_INTERVAL;
                if volume_set_changed || reconcile_due {
                    known_topology = current_topology;
                    // A mount change only needs newly discovered or stale volumes;
                    // healthy NTFS volumes catch up from USN and live fallback
                    // watchers cover existing directory-backed volumes.
                    index
                        .scan_all_volumes(volume_set_changed && !reconcile_due)
                        .await;
                    last_reconcile = tokio::time::Instant::now();
                    // scan_all_volumes discovers volumes itself; refresh the
                    // snapshot so the next tick does not see a phantom change.
                    current_volumes =
                        tauri::async_runtime::spawn_blocking(volume::discover_volumes)
                            .await
                            .unwrap_or_default();
                    continue;
                }
            }

            // Healthy NTFS volumes never receive periodic recursive
            // reconciliation. Their persisted cursor advances from the
            // journal every poll while directory backends stay watcher-led.
            index.catch_up_ntfs_volumes(&current_volumes).await;
        }
    });
}

fn volume_topology_fingerprint(volumes: &[VolumeInfo]) -> Vec<String> {
    let mut fingerprints: Vec<String> = volumes
        .iter()
        .map(|volume| {
            let canonical = volume
                .canonical_mount_path()
                .map(|path| path.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let aliases = volume
                .normalized_mount_paths()
                .iter()
                .map(|path| path.to_string_lossy().to_lowercase())
                .collect::<Vec<_>>()
                .join("|");
            format!(
                "{};{};{};{};{}",
                volume.volume_id.to_lowercase(),
                canonical,
                aliases,
                volume.fs_type.to_lowercase(),
                volume.drive_type
            )
        })
        .collect();
    fingerprints.sort_unstable();
    fingerprints.dedup();
    fingerprints
}

fn debug_volume_plan(
    db: &Database,
    volume: &VolumeInfo,
    mount_path: &Path,
    fresh: bool,
    update: Option<&self::db::VolumeUpdate>,
) {
    if !catalog_debug_enabled() {
        return;
    }
    let eligible_backend = match select_backend(volume, true) {
        BackendKind::Ntfs => "ntfs-probe-pending",
        BackendKind::Directory => "directory",
    };
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "volume_scan_plan",
            "sources": volume.discovery_sources,
            "raw_volume_guid_path": volume.volume_guid_path,
            "volume_id": volume.volume_id,
            "mount_paths": volume.normalized_mount_paths().iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            "canonical_mount_path": mount_path.to_string_lossy(),
            "drive_letter": volume.drive_letter,
            "drive_type": volume.drive_type,
            "file_system": volume.fs_type,
            "label": volume.label,
            "accessible": volume.accessible,
            "selected_backend": eligible_backend,
            "existing_database_mount_path": update.and_then(|value| value.previous_mount_path.as_deref()),
            "volume_change": update.map(|value| format!("{:?}", value.change)),
            "fresh": fresh,
            "scan_action": if !volume.accessible { "skipped-offline" } else if fresh { "backend-sync-or-watcher" } else { "started" },
            "database_status": db.volume_debug_status(&volume.volume_id).ok().map(|status| serde_json::json!({
                "mount_path": status.0,
                "backend": status.1,
                "state": status.2,
                "scanned_entries": status.3,
                "last_scanned_at": status.4,
                "index_status": status.5,
            })),
        })
    );
}

fn debug_scan_complete(
    db: &Database,
    volume: &VolumeInfo,
    mount_path: &str,
    result: &Result<u64, String>,
) {
    if !catalog_debug_enabled() {
        return;
    }
    let status = db.volume_debug_status(&volume.volume_id).ok();
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "volume_scan_complete",
            "volume_id": volume.volume_id,
            "mount_path": mount_path,
            "scan_count": result.as_ref().ok(),
            "scan_error": result.as_ref().err(),
            "database_backend": status.as_ref().map(|value| value.1.as_str()),
            "database_state": status.as_ref().map(|value| value.2.as_str()),
            "database_scanned_entries": status.as_ref().map(|value| value.3),
            "last_scanned_at": status.as_ref().map(|value| value.4),
            "index_status": status.as_ref().map(|value| value.5.as_str()),
        })
    );
}

fn debug_ntfs_complete(
    db: &Database,
    volume: &VolumeInfo,
    mount_path: &str,
    records: u64,
    deferred_error: Option<&str>,
) {
    if !catalog_debug_enabled() {
        return;
    }
    let status = db.volume_debug_status(&volume.volume_id).ok();
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "volume_ntfs_sync_complete",
            "volume_id": volume.volume_id,
            "mount_path": mount_path,
            "records": records,
            "deferred_error": deferred_error,
            "database_backend": status.as_ref().map(|value| value.1.as_str()),
            "database_state": status.as_ref().map(|value| value.2.as_str()),
            "database_scanned_entries": status.as_ref().map(|value| value.3),
            "last_scanned_at": status.as_ref().map(|value| value.4),
            "index_status": status.as_ref().map(|value| value.5.as_str()),
        })
    );
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

fn known_locations() -> Vec<(&'static str, PathBuf, &'static str)> {
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
/// Bounds the per-request decode work; one result page is at most ~20 files.
const THUMBNAIL_BATCH_CAP: usize = 64;

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

    fn topology_volume(id: &str, mount: &str) -> VolumeInfo {
        VolumeInfo {
            volume_id: id.into(),
            volume_guid_path: Some(format!(r"\\?\Volume{{{id}}}\")),
            discovery_sources: vec!["test".into()],
            drive_letter: volume::drive_letter_from_mount_path(Path::new(mount)),
            mount_paths: vec![PathBuf::from(mount)],
            drive_type: windows::Win32::System::WindowsProgramming::DRIVE_FIXED,
            label: String::new(),
            fs_type: "NTFS".into(),
            accessible: true,
        }
    }

    #[test]
    fn topology_fingerprint_detects_mount_and_alias_changes() {
        let d = topology_volume("same", r"D:\");
        let e = topology_volume("same", r"E:\");
        assert_ne!(
            volume_topology_fingerprint(std::slice::from_ref(&d)),
            volume_topology_fingerprint(&[e])
        );

        let mut alias_changed = d.clone();
        alias_changed
            .mount_paths
            .push(PathBuf::from(r"C:\Mounts\Data"));
        assert_ne!(
            volume_topology_fingerprint(&[d]),
            volume_topology_fingerprint(&[alias_changed])
        );
    }

    #[test]
    fn scoped_repairs_coalesce_without_losing_a_pending_pass() {
        let mut coordinator = ScopeRepairCoordinator::default();
        let key = "vol1|c:\\users".to_string();

        assert!(
            coordinator.request(key.clone()),
            "first request starts work"
        );
        assert!(
            !coordinator.request(key.clone()),
            "second request joins active work"
        );
        assert!(
            coordinator.finish_pass(&key),
            "pending work gets another pass"
        );
        assert!(!coordinator.finish_pass(&key), "quiet scope stops running");
        assert!(
            coordinator.request(key),
            "a later request starts fresh work"
        );
    }

    #[test]
    fn source_workers_are_independent_and_preserve_forced_reruns() {
        let mut coordinator = SourceWorkerCoordinator::default();

        assert!(coordinator.request("c".into(), true));
        assert!(coordinator.request("nas".into(), true));
        assert!(coordinator.is_running("c"));
        assert!(coordinator.is_running("nas"));
        assert!(!coordinator.request("c".into(), false));
        assert_eq!(coordinator.finish_pass("c"), Some(false));
        assert_eq!(coordinator.finish_pass("c"), None);
        assert_eq!(coordinator.finish_pass("nas"), None);
        assert!(coordinator.is_empty());
    }
}
