pub mod db;
pub mod scanner;
pub mod search;
pub mod types;
pub mod volume;
pub mod watcher;

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;
use tauri::Emitter;
use windows::core::GUID;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Profile, FOLDERID_Videos, SHGetKnownFolderPath,
};

pub use types::{FileSearchResponse, QuickAccessEntry, VolumeCoverage, VolumeState};

use self::db::Database;
use self::watcher::VolumeWatcher;

const VOLUME_POLL_INTERVAL: Duration = Duration::from_secs(5);
const RECONCILIATION_INTERVAL: Duration = Duration::from_secs(30 * 60); // 30 mins

#[derive(Clone)]
pub struct FileIndex {
    db: Arc<RwLock<Option<Arc<Database>>>>,
    search_generation: Arc<AtomicU64>,
    volumes: Arc<RwLock<Vec<VolumeCoverage>>>,
    indexing: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
    total_indexed: Arc<AtomicU64>,
    watchers: Arc<Mutex<HashMap<String, VolumeWatcher>>>,
    scan_cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    app_data_dir: Arc<RwLock<PathBuf>>,
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
    scan_generation: Arc<AtomicU64>,
}

impl Default for FileIndex {
    fn default() -> Self {
        Self {
            db: Arc::new(RwLock::new(None)),
            search_generation: Arc::new(AtomicU64::new(0)),
            volumes: Arc::new(RwLock::new(Vec::new())),
            indexing: Arc::new(AtomicBool::new(false)),
            ready: Arc::new(AtomicBool::new(false)),
            total_indexed: Arc::new(AtomicU64::new(0)),
            watchers: Arc::new(Mutex::new(HashMap::new())),
            scan_cancels: Arc::new(Mutex::new(HashMap::new())),
            app_data_dir: Arc::new(RwLock::new(PathBuf::from("."))),
            app_handle: Arc::new(Mutex::new(None)),
            scan_generation: Arc::new(AtomicU64::new(1)),
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
                if let Ok(mut vols) = self.volumes.write() {
                    *vols = covs;
                }
            }
            if let Ok(total) = db_arc.get_total_indexed_count() {
                self.total_indexed.store(total, Ordering::SeqCst);
                if total > 0 {
                    self.ready.store(true, Ordering::SeqCst);
                }
            }
            *self.db.write().unwrap() = Some(db_arc);
        }
        *self.app_data_dir.write().unwrap() = app_data_dir.to_path_buf();
    }

    pub fn search(&self, query: &str, limit: Option<usize>) -> FileSearchResponse {
        let volumes = self.volumes.read().unwrap().clone();
        let total_indexed = self.total_indexed.load(Ordering::Relaxed);
        let indexing = self.indexing.load(Ordering::Relaxed);
        let ready = self.ready.load(Ordering::Relaxed);

        let db = self.db.read().unwrap().clone();
        let Some(ref db) = db else {
            return search::browse_path(query.trim(), limit.unwrap_or(10))
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

    pub fn rebuild(&self) {
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            this.cancel_all_scans();
            let db = this.db.read().unwrap().clone();
            if let Some(ref db) = db {
                let _ = db.finish_volume_scan("", 0, 0);
            }
            this.total_indexed.store(0, Ordering::SeqCst);
            this.ready.store(false, Ordering::SeqCst);
            this.rescan_all_volumes().await;
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

    fn emit_updated(&self) {
        if let Some(app) = self.app_handle.lock().unwrap().as_ref() {
            let _ = app.emit("file-index-updated", ());
        }
    }

    async fn rescan_all_volumes(&self) {
        let db_opt = self.db.read().unwrap().clone();
        let Some(ref db) = db_opt else { return };
        self.indexing.store(true, Ordering::SeqCst);
        self.emit_updated();

        let discovered = tauri::async_runtime::spawn_blocking(volume::discover_volumes)
            .await
            .unwrap_or_default();

        let app_data = self.app_data_dir.read().unwrap().clone();

        for vol in discovered {
            let volume_id = vol.volume_id.clone();
            let mount_path = match vol.mount_paths.first() {
                Some(p) => p.clone(),
                None => continue,
            };

            let _ = db.upsert_volume(&vol, VolumeState::Indexing);
            self.update_volume_coverage(&mount_path.to_string_lossy(), VolumeState::Indexing);

            // Start watcher if not running
            {
                let mut watchers = self.watchers.lock().unwrap();
                if !watchers.contains_key(&volume_id) {
                    let db_clone = db.clone();
                    let app_data_clone = app_data.clone();
                    let index_clone = self.clone();
                    let vol_id_overflow = volume_id.clone();
                    if let Some(w) = VolumeWatcher::start(
                        mount_path.clone(),
                        volume_id.clone(),
                        db_clone,
                        app_data_clone,
                        move |_| {
                            let idx = index_clone.clone();
                            let v_id = vol_id_overflow.clone();
                            tauri::async_runtime::spawn(async move {
                                idx.reconcile_volume(&v_id).await;
                            });
                        },
                    ) {
                        watchers.insert(volume_id.clone(), w);
                    }
                } else if let Some(w) = watchers.get(&volume_id) {
                    w.set_buffering(true);
                }
            }

            let cancel = Arc::new(AtomicBool::new(false));
            {
                self.scan_cancels
                    .lock()
                    .unwrap()
                    .insert(volume_id.clone(), cancel.clone());
            }

            let db_for_scan = db.clone();
            let gen = self.scan_generation.fetch_add(1, Ordering::SeqCst);
            let app_data_scan = app_data.clone();
            let index_for_progress = self.clone();
            let mount_str = mount_path.to_string_lossy().into_owned();
            let vol_id_scan = volume_id.clone();

            let scan_result = tauri::async_runtime::spawn_blocking(move || {
                scanner::scan_volume(
                    &mount_path,
                    &vol_id_scan,
                    gen,
                    db_for_scan,
                    &app_data_scan,
                    cancel,
                    move |count| {
                        index_for_progress.update_progress(&mount_str, count);
                    },
                )
            })
            .await;

            // Flush buffered watcher events
            {
                let watchers = self.watchers.lock().unwrap();
                if let Some(w) = watchers.get(&volume_id) {
                    w.flush_queue(db);
                }
            }

            match scan_result {
                Ok(Ok(count)) => {
                    let _ = db.finish_volume_scan(&volume_id, gen, count);
                    self.update_volume_coverage(
                        &vol.mount_paths[0].to_string_lossy(),
                        VolumeState::Ready,
                    );
                }
                Ok(Err(e)) => {
                    eprintln!("[Prism Catalog] Scan for volume {volume_id} error: {e}");
                    let _ = db.set_volume_state(&volume_id, VolumeState::Error);
                    self.update_volume_coverage(
                        &vol.mount_paths[0].to_string_lossy(),
                        VolumeState::Error,
                    );
                }
                Err(_) => {
                    let _ = db.set_volume_state(&volume_id, VolumeState::Error);
                }
            }
        }

        self.refresh_totals_and_status();
        self.indexing.store(false, Ordering::SeqCst);
        self.ready.store(true, Ordering::SeqCst);
        self.emit_updated();
    }

    async fn reconcile_volume(&self, volume_id: &str) {
        let db_opt = self.db.read().unwrap().clone();
        let Some(ref db) = db_opt else { return };
        let app_data = self.app_data_dir.read().unwrap().clone();
        let volumes = tauri::async_runtime::spawn_blocking(volume::discover_volumes)
            .await
            .unwrap_or_default();

        let Some(vol) = volumes.into_iter().find(|v| v.volume_id == volume_id) else {
            return;
        };

        let mount_path = match vol.mount_paths.first() {
            Some(p) => p.clone(),
            None => return,
        };

        let cancel = Arc::new(AtomicBool::new(false));
        {
            self.scan_cancels
                .lock()
                .unwrap()
                .insert(volume_id.to_string(), cancel.clone());
        }

        let db_for_scan = db.clone();
        let gen = self.scan_generation.fetch_add(1, Ordering::SeqCst);
        let vol_id = volume_id.to_string();

        let _ = tauri::async_runtime::spawn_blocking(move || {
            let _ = scanner::scan_volume(
                &mount_path,
                &vol_id,
                gen,
                db_for_scan,
                &app_data,
                cancel,
                |_| {},
            );
        })
        .await;

        self.refresh_totals_and_status();
        self.emit_updated();
    }

    fn update_progress(&self, drive: &str, count: u64) {
        if let Ok(mut vols) = self.volumes.write() {
            for v in vols.iter_mut() {
                if v.drive.eq_ignore_ascii_case(drive) {
                    v.indexed_count = count;
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
                    indexed_count: 0,
                    total_progress: None,
                });
            }
        }
    }

    fn refresh_totals_and_status(&self) {
        let db_opt = self.db.read().unwrap().clone();
        if let Some(ref db) = db_opt {
            if let Ok(covs) = db.get_volume_coverages() {
                if let Ok(mut vols) = self.volumes.write() {
                    *vols = covs;
                }
            }
            if let Ok(total) = db.get_total_indexed_count() {
                self.total_indexed.store(total, Ordering::SeqCst);
                if total > 0 {
                    self.ready.store(true, Ordering::SeqCst);
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
                self.refresh_totals_and_status();
            }
        }
    }
}

pub fn warm(index: FileIndex, app_data_dir: PathBuf, app: tauri::AppHandle) {
    index.set_app_handle(app.clone());
    let legacy_cache_path = app_data_dir.join("files.json");
    index.try_migrate_legacy_cache(&legacy_cache_path);

    tauri::async_runtime::spawn(async move {
        index.rescan_all_volumes().await;

        let mut last_reconcile = tokio::time::Instant::now();
        let mut known_volume_ids: Vec<String> = Vec::new();

        loop {
            tokio::time::sleep(VOLUME_POLL_INTERVAL).await;

            let current_volumes = tauri::async_runtime::spawn_blocking(volume::discover_volumes)
                .await
                .unwrap_or_default();

            let current_ids: Vec<String> = current_volumes
                .iter()
                .map(|v| v.volume_id.clone())
                .collect();

            if current_ids != known_volume_ids
                || last_reconcile.elapsed() >= RECONCILIATION_INTERVAL
            {
                known_volume_ids = current_ids;
                index.rescan_all_volumes().await;
                last_reconcile = tokio::time::Instant::now();
            }
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

pub fn file_thumbnail(path: &str) -> Option<String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.is_dir() {
        return None;
    }
    image_thumbnail(path)
}

fn image_thumbnail(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_string_lossy().to_lowercase();
    if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
    ) {
        return None;
    }
    if std::fs::metadata(path).ok()?.len() > THUMBNAIL_MAX_BYTES {
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
