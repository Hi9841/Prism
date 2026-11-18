//! The commands of the palette backend (RULEBOOK A4). Each public method is
//! one former `#[tauri::command]` body, and the UI calls it directly. Effects
//! that need the window or the event loop go through [`crate::host`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::settings::{
    migrate_state_value, startup_shortcut, startup_taskbar_alignment, validate_shortcut,
    validate_state,
};
use prism_vocab::settings::{STATE_VERSION, WIN_SHORTCUT};
use crate::{
    apps, audio_hook, audio_osd, catalog, perf, power, query, start_menu, taskbar,
    taskbar_alignment, taskbar_customization, theme, win_key, windows_tools,
};

const STARTUP_SHELL_RETRY_DELAY: Duration = Duration::from_millis(250);
/// Two minutes of attempts while Explorer runs; a launch at sign-in can come
/// long before Explorer's taskbar, and that wait does not count.
const STARTUP_SHELL_RETRY_ATTEMPTS: usize = 480;
const STARTUP_ALIGNMENT_DELAY: Duration = Duration::from_secs(1);

pub struct Backend {
    apps_cache: Mutex<Option<Vec<apps::AppEntry>>>,
    /// When the in-memory index was scanned. Compared with shortcut and
    /// install-folder mtimes so a long-running session can pick up new apps.
    apps_indexed_at: Mutex<Option<SystemTime>>,
    apps_scan_lock: tokio::sync::Mutex<()>,
    file_index: catalog::FileIndex,
    intent: query::IntentStore,
    launches: query::LaunchStore,
    shortcut: Mutex<String>,
    shortcut_generation: AtomicU64,
}

static BACKEND: OnceLock<Backend> = OnceLock::new();

/// The process-wide backend.
pub fn backend() -> &'static Backend {
    BACKEND.get_or_init(|| Backend {
        apps_cache: Mutex::new(None),
        apps_indexed_at: Mutex::new(None),
        apps_scan_lock: tokio::sync::Mutex::new(()),
        file_index: catalog::FileIndex::default(),
        intent: query::IntentStore::new(),
        launches: query::LaunchStore::new(),
        shortcut: Mutex::new(String::new()),
        shortcut_generation: AtomicU64::new(0),
    })
}

/// Handles the internal crash-recovery mode before the app starts. Returns
/// true when this process was launched as the watchdog.
pub fn run_start_restore_watchdog_if_requested() -> bool {
    start_menu::run_watchdog_from_args()
}

pub fn launched_for_autostart() -> bool {
    std::env::args_os()
        .skip(1)
        .any(|argument| argument == std::ffi::OsStr::new("--autostart"))
}

/// Task Manager's Startup apps switch for Prism's Run entry. Windows keeps it
/// in `StartupApproved\Run`: an odd first byte means disabled. The sign-in
/// task starts Prism outside the Run queue, so Prism honors the switch itself.
pub fn autostart_disabled_by_user() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    let mut data = [0u8; 64];
    let mut size = data.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"),
            w!("Prism"),
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    status.is_ok() && size > 0 && data[0] & 1 == 1
}

fn apps_cache_path() -> PathBuf {
    crate::host::data_dir().join("apps.json")
}

fn join_error(context: &str, error: tokio::task::JoinError) -> String {
    format!("{context}: {error}")
}

impl Backend {
    /// The backend part of app setup. Requires an installed host.
    pub fn start(&'static self, persisted: Option<&serde_json::Value>) {
        let _ = start_menu::recover_stale();
        // Repair the taskbar band if a previous instance crashed while
        // the palette was open.
        crate::runtime::spawn_blocking(taskbar::recover);
        let alignment = startup_taskbar_alignment(persisted);
        let _ = taskbar_alignment::initialize(&alignment);
        schedule_startup_taskbar_alignment();
        crate::runtime::spawn_blocking(taskbar_customization::init);
        audio_hook::init();
        theme::watch();
        // Install the persisted shortcut first: Prism must own the Win key as
        // soon as it runs, and nothing below is needed for it. A fresh,
        // corrupt, or incomplete state defaults natively to Win, so first-run
        // activation never waits on the UI. The Explorer bridge is
        // initialized off the UI thread.
        self.apply_startup_shortcut(startup_shortcut(persisted));
        let app_data_dir = crate::host::data_dir();
        self.intent.set_path(app_data_dir.join("intent-cache.db"));
        self.launches.set_path(app_data_dir.join("launch-counts.json"));
        catalog::warm(self.file_index.clone(), app_data_dir);
        crate::runtime::spawn_blocking(windows_tools::warm);
        crate::runtime::spawn_blocking(crate::settings_index::warm);
        self.warm_apps();
    }

    fn warm_apps(&'static self) {
        crate::runtime::spawn(async move {
            let _scan_guard = self.apps_scan_lock.lock().await;
            if self
                .apps_cache
                .lock()
                .ok()
                .and_then(|cache| cache.as_ref().map(|_| ()))
                .is_some()
            {
                return;
            }
            let cache_path = apps_cache_path();
            let result = crate::runtime::spawn_blocking(move || apps::scan(&cache_path)).await;
            let Ok(Ok(index)) = result else {
                return;
            };
            self.publish_app_index(index);
        });
    }

    fn publish_app_index(&self, index: apps::AppIndex) -> Vec<apps::AppEntry> {
        if let Ok(mut indexed_at) = self.apps_indexed_at.lock() {
            *indexed_at = Some(index.scanned_at);
        }
        let apps = index.apps;
        if let Ok(mut cache) = self.apps_cache.lock() {
            *cache = Some(apps.clone());
        }
        apps
    }

    /// On palette open, rescan when a shortcut or install folder changed, or
    /// an indexed app was updated in place, after the in-memory index was
    /// built. The check is off the Win-key path; the UI hears `apps-updated`
    /// once the new list is published.
    pub fn schedule_installed_apps_refresh(&'static self) {
        crate::runtime::spawn(async move {
            let since = self.apps_indexed_at.lock().ok().and_then(|guard| *guard);
            let Some(since) = since else {
                return;
            };
            let changed = self.apps_changed_since(since).await;
            let Ok(true) = changed else {
                return;
            };
            let _scan_guard = self.apps_scan_lock.lock().await;
            let since = self.apps_indexed_at.lock().ok().and_then(|guard| *guard);
            let Some(since) = since else {
                return;
            };
            let changed = self.apps_changed_since(since).await;
            let Ok(true) = changed else {
                return;
            };
            let cache_path = apps_cache_path();
            let scanned =
                crate::runtime::spawn_blocking(move || apps::scan_force(&cache_path)).await;
            let Ok(Ok(index)) = scanned else {
                return;
            };
            self.publish_app_index(index);
            crate::host::emit(crate::host::CoreEvent::AppsUpdated);
        });
    }

    async fn apps_changed_since(
        &self,
        since: SystemTime,
    ) -> Result<bool, tokio::task::JoinError> {
        let files = self
            .apps_cache
            .lock()
            .ok()
            .and_then(|cache| cache.as_deref().map(apps::watched_files))
            .unwrap_or_default();
        crate::runtime::spawn_blocking(move || {
            apps::installs_changed_since(since) || apps::apps_updated_since(&files, since)
        })
        .await
    }

    pub async fn get_apps(&self) -> Result<Vec<apps::AppEntry>, String> {
        let timer = perf::start();
        if let Some(list) = self.apps_cache.lock().map_err(|e| e.to_string())?.clone() {
            perf::finish(timer, "get_apps_cache", || format!("count={}", list.len()));
            return Ok(list);
        }
        // Fast repeated opens can overlap the initial request. Only one scan
        // should touch the filesystem and icon cache; waiters recheck the
        // populated in-memory cache after the first scan completes.
        let _scan_guard = self.apps_scan_lock.lock().await;
        if let Some(list) = self.apps_cache.lock().map_err(|e| e.to_string())?.clone() {
            perf::finish(timer, "get_apps_cache_after_wait", || {
                format!("count={}", list.len())
            });
            return Ok(list);
        }
        // Off the UI thread: scanning walks the shell and extracts icons.
        let cache_path = apps_cache_path();
        let index = crate::runtime::spawn_blocking(move || apps::scan(&cache_path))
            .await
            .map_err(|e| join_error("app scan task failed", e))??;
        let list = self.publish_app_index(index);
        perf::finish(timer, "get_apps_scan", || format!("count={}", list.len()));
        Ok(list)
    }

    pub async fn refresh_apps(&self) -> Result<Vec<apps::AppEntry>, String> {
        let _scan_guard = self.apps_scan_lock.lock().await;
        let cache_path = apps_cache_path();
        let index = crate::runtime::spawn_blocking(move || {
            let _ = windows_tools::refresh();
            apps::scan_force(&cache_path)
        })
        .await
        .map_err(|e| join_error("app scan task failed", e))??;
        let list = self.publish_app_index(index);
        Ok(list)
    }

    fn cached_app(&self, id: &str) -> Result<apps::AppEntry, String> {
        let apps = self.apps_cache.lock().map_err(|e| e.to_string())?;
        apps.as_ref()
            .and_then(|list| list.iter().find(|a| a.app_id == id))
            .cloned()
            .ok_or_else(|| "unknown app id".to_string())
    }

    pub async fn launch_app(&self, id: String) -> Result<(), String> {
        let timer = perf::start();
        // Only launch apps that came from our own scan. The entry is cloned so
        // the cache lock is released before ShellExecuteW, which can block.
        let entry = self.cached_app(&id)?;
        let detail = format!("name={};source={}", entry.name, entry.source);
        let result = crate::runtime::spawn_blocking(move || crate::launch::launch(&entry))
            .await
            .map_err(|e| join_error("launch task failed", e))?;
        perf::finish(timer, "launch_app", move || detail.clone());
        if result.is_ok() {
            self.count_launch(&id);
        }
        result
    }

    pub async fn launch_app_as_admin(&self, id: String) -> Result<(), String> {
        let entry = self.cached_app(&id)?;
        crate::runtime::spawn_blocking(move || crate::launch::launch_elevated(&entry))
            .await
            .map_err(|e| join_error("elevated launch task failed", e))??;
        self.count_launch(&id);
        Ok(())
    }

    /// A launch that works counts for search ranking. A failed save does not
    /// fail the launch: the app is already open.
    fn count_launch(&self, id: &str) {
        let _ = self.launches.record(id);
    }

    /// Removes the launch counts of all apps (the Recent items "Clear" action).
    pub fn clear_launch_counts(&self) -> Result<(), String> {
        self.launches.clear()
    }

    /// Removes the launch count of one app (Recent items "Remove").
    pub fn forget_app_launches(&self, id: String) -> Result<(), String> {
        self.launches.forget(&id)
    }

    pub async fn open_path(&self, path: String) -> Result<(), String> {
        let timer = perf::start();
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.exists() {
            return Err("path must be an existing absolute file or folder".to_string());
        }
        let kind = if path.is_dir() { "directory" } else { "file" };
        let result = crate::runtime::spawn_blocking(move || crate::launch::open_path(&path))
            .await
            .map_err(|e| join_error("open path task failed", e))?;
        perf::finish(timer, "open_path", move || format!("kind={kind}"));
        result
    }

    pub async fn open_path_location(&self, path: String) -> Result<(), String> {
        let timer = perf::start();
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.exists() {
            return Err("path must be an existing absolute file or folder".to_string());
        }
        let result = crate::runtime::spawn_blocking(move || crate::launch::open_path_location(&path))
            .await
            .map_err(|e| join_error("open path location task failed", e))?;
        perf::finish(timer, "open_path_location", String::new);
        result
    }

    pub async fn run_path_as_admin(&self, path: String) -> Result<(), String> {
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.is_file() {
            return Err("path must be an existing absolute file".to_string());
        }
        crate::runtime::spawn_blocking(move || crate::launch::launch_path_elevated(&path, None, None))
            .await
            .map_err(|e| join_error("run as administrator task failed", e))?
    }

    pub async fn is_pinned_to_taskbar(&self, path: String) -> bool {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return false;
        }
        crate::runtime::spawn_blocking(move || crate::taskbar_pin::is_pinned_to_taskbar(&path))
            .await
            .unwrap_or(false)
    }

    pub async fn set_taskbar_pinned(&self, path: String, pinned: bool) -> Result<(), String> {
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.exists() {
            return Err("path must be an existing absolute file".to_string());
        }
        crate::runtime::spawn_blocking(move || crate::taskbar_pin::set_taskbar_pinned(&path, pinned))
            .await
            .map_err(|e| join_error("taskbar pin task failed", e))?
    }

    pub async fn show_path_properties(&self, path: String) -> Result<(), String> {
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.exists() {
            return Err("path must be an existing absolute file or folder".to_string());
        }
        crate::runtime::spawn_blocking(move || crate::launch::show_properties(&path))
            .await
            .map_err(|e| join_error("properties task failed", e))?
    }

    /// The validated paths for `start_file_drag`. The drag itself runs on the
    /// UI thread in the app.
    pub fn drag_paths(&self, paths: Vec<String>) -> Result<Vec<PathBuf>, String> {
        if paths.is_empty() {
            return Err("No file paths provided".to_string());
        }
        let path_bufs: Vec<PathBuf> = paths
            .into_iter()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.exists())
            .collect();
        if path_bufs.is_empty() {
            return Err("No existing absolute file paths provided for drag".to_string());
        }
        Ok(path_bufs)
    }

    pub fn query_phase1(&self, query: String) -> query::Phase1Response {
        let timer = perf::start();
        let query_length = query.chars().count();
        let windows = query::list_open_windows();
        let apps_guard = self.apps_cache.lock().ok();
        let apps = apps_guard
            .as_ref()
            .and_then(|guard| guard.as_ref())
            .map(|list| list.as_slice())
            .unwrap_or(&[]);
        let response = query::phase1(&query, apps, &windows, &self.intent, &self.launches);
        perf::finish(timer, "query_phase1", || {
            format!(
                "queryLength={query_length};actions={};apps={};windows={}",
                response.actions.len(),
                response.apps.len(),
                response.windows.len()
            )
        });
        response
    }

    pub async fn query_power_plans(
        &self,
        query: String,
    ) -> Result<Vec<power::PowerPlanHit>, String> {
        crate::runtime::spawn_blocking(move || power::search(&query))
            .await
            .map_err(|error| join_error("power plan query task failed", error))?
    }

    pub async fn execute_action(&self, id: String) -> Result<(), String> {
        crate::runtime::spawn_blocking(move || query::execute(&id))
            .await
            .map_err(|error| join_error("action task failed", error))?
    }

    pub fn accept_intent(&self, query: String, action_id: String) -> Result<(), String> {
        query::accept_intent(&query, &action_id, &self.intent)
    }

    pub fn focus_window(&self, hwnd: i64) -> Result<(), String> {
        query::focus_window(hwnd)
    }

    /// Phase 2: file search over the in-memory file index. The UI must not block Phase 1 on this.
    pub async fn search_files(
        &self,
        query: String,
        limit: Option<usize>,
    ) -> Result<catalog::types::FileSearchResponse, String> {
        let timer = perf::start();
        let query_length = query.chars().count();
        let index = self.file_index.clone();
        // Allocate the generation before scheduling. Worker start order is not
        // request order, so allocating inside spawn_blocking lets an older query
        // supersede a newer one.
        let generation = index.begin_search();
        let result = crate::runtime::spawn_blocking(move || {
            index.search_with_generation(&query, limit, generation)
        })
        .await
        .map_err(|error| join_error("file search task failed", error));
        perf::finish(timer, "search_files", || match &result {
            Ok(response) => format!(
                "queryLength={query_length};results={};pathBrowse={}",
                response.items.len(),
                response.path_browse
            ),
            Err(_) => format!("queryLength={query_length};error=true"),
        });
        result
    }

    pub fn rebuild_file_index(&self) {
        self.file_index.rebuild();
    }

    pub async fn get_file_thumbnails(&self, paths: Vec<String>) -> Vec<Option<String>> {
        crate::runtime::spawn_blocking(move || catalog::file_thumbnails(paths))
            .await
            .unwrap_or_default()
    }

    pub async fn get_quick_access(&self) -> Vec<catalog::types::QuickAccessEntry> {
        // Known-folder resolution and is_dir checks can block on redirected or
        // network locations; keep the startup path off the UI thread.
        crate::runtime::spawn_blocking(catalog::quick_access)
            .await
            .unwrap_or_default()
    }

    pub async fn existing_paths(&self, paths: Vec<String>) -> Vec<String> {
        crate::runtime::spawn_blocking(move || filter_existing_paths(paths))
            .await
            .unwrap_or_default()
    }

    pub async fn perform_power_action(&self, action: String) -> Result<(), String> {
        crate::runtime::spawn_blocking(move || power::perform(&action))
            .await
            .map_err(|error| join_error("power action task failed", error))?
    }

    pub async fn set_power_plan(&self, guid: String) -> Result<(), String> {
        crate::runtime::spawn_blocking(move || power::set_active(&guid))
            .await
            .map_err(|error| join_error("power plan task failed", error))?
    }

    pub async fn get_taskbar_settings(
        &self,
    ) -> Result<taskbar_customization::TaskbarSettings, String> {
        crate::runtime::spawn_blocking(taskbar_customization::settings)
            .await
            .map_err(|error| join_error("taskbar settings task failed", error))?
    }

    pub fn set_taskbar_thickness(&self, value: String) -> Result<(), String> {
        taskbar_customization::set_thickness(&value)
    }

    pub fn set_taskbar_auto_hide(&self, enabled: bool) -> Result<(), String> {
        taskbar_customization::set_auto_hide(enabled)
    }

    pub fn set_taskbar_combine_buttons(&self, value: String) -> Result<(), String> {
        taskbar_customization::set_combine_buttons(&value)
    }

    pub fn set_taskbar_scroll_volume(&self, enabled: bool) {
        audio_hook::set_enabled(enabled);
    }

    /// Pushes the accent preset to the native volume OSD so its level bar stays
    /// in family with the palette accent.
    pub fn set_osd_accent(&self, accent: String) {
        audio_osd::set_accent(&accent);
    }

    pub async fn set_custom_start_icon(&self, base64_png: String) -> Result<(), String> {
        crate::runtime::spawn_blocking(move || {
            taskbar_customization::set_custom_start_icon(&base64_png)
        })
        .await
        .map_err(|error| join_error("taskbar icon task failed", error))?
    }

    pub async fn select_custom_start_icon(&self, id: String) -> Result<(), String> {
        crate::runtime::spawn_blocking(move || taskbar_customization::select_custom_start_icon(&id))
            .await
            .map_err(|error| join_error("taskbar icon selection task failed", error))?
    }

    pub async fn remove_custom_start_icon(&self, id: String) -> Result<(), String> {
        crate::runtime::spawn_blocking(move || taskbar_customization::remove_custom_start_icon(&id))
            .await
            .map_err(|error| join_error("taskbar icon removal task failed", error))?
    }

    pub async fn set_taskbar_start_icon(&self, value: String) -> Result<(), String> {
        crate::runtime::spawn_blocking(move || taskbar_customization::set_start_icon(&value))
            .await
            .map_err(|error| join_error("taskbar icon mode task failed", error))?
    }

    pub fn get_system_theme(&self) -> String {
        match theme::apps_light() {
            Some(true) => "light".to_string(),
            _ => "dark".to_string(),
        }
    }

    fn apply_startup_shortcut(&'static self, combo: String) {
        let generation = self.shortcut_generation.load(Ordering::Acquire);
        crate::runtime::spawn(async move {
            if self.shortcut_generation.load(Ordering::Acquire) != generation {
                return;
            }
            let first_attempt = {
                let attempt_combo = combo.clone();
                crate::runtime::spawn_blocking(move || {
                    self.apply_shortcut_if_current(&attempt_combo, generation)
                })
                .await
            };
            if matches!(first_attempt, Ok(Ok(()))) {
                return;
            }

            let mut attempts = 0;
            while attempts < STARTUP_SHELL_RETRY_ATTEMPTS {
                tokio::time::sleep(STARTUP_SHELL_RETRY_DELAY).await;
                if self.shortcut_generation.load(Ordering::Acquire) != generation {
                    return;
                }
                // The Win mode needs Explorer's taskbar; until it exists an
                // attempt would only write and delete the bridge library.
                // Explorer can be missing for longer than the retries (a
                // stopped shell that the user starts again): wait for it,
                // and count only the attempts made while it runs.
                if combo == WIN_SHORTCUT && !crate::shell_bridge::shell_windows_ready() {
                    continue;
                }
                attempts += 1;
                let retry_combo = combo.clone();
                let result = crate::runtime::spawn_blocking(move || {
                    self.apply_shortcut_if_current(&retry_combo, generation)
                })
                .await;
                if matches!(result, Ok(Ok(()))) {
                    return;
                }
            }
            crate::host::emit(crate::host::CoreEvent::WinModeFailed(
                "Explorer taskbar integration was unavailable during startup".to_string(),
            ));
        });
    }

    /// Applies a global shortcut. The new binding is activated and proven
    /// BEFORE the old one is released; on any failure the previous shortcut
    /// stays active and an error is returned. State is only updated on success.
    fn apply_shortcut_if_current(&self, combo: &str, generation: u64) -> Result<(), String> {
        validate_shortcut(combo)?;
        let host = crate::host::get().ok_or("the app host is not installed")?;
        let mut prev = self.shortcut.lock().map_err(|e| e.to_string())?;
        if self.shortcut_generation.load(Ordering::Acquire) != generation {
            return Err("shortcut request was superseded".to_string());
        }
        if *prev == combo {
            return Ok(());
        }
        let release_previous = |prev: &str| {
            if !prev.is_empty() && prev != WIN_SHORTCUT {
                host.unregister_shortcut(prev);
            }
        };

        if combo == WIN_SHORTCUT {
            // Standalone Win mode uses the native hook rather than a
            // registration. If the hook later fails, it self-disables and the
            // UI is notified.
            let provider_suppression = start_menu::enable()?;
            win_key::set_provider_suppression(provider_suppression);
            if let Err(error) = win_key::set_enabled(true) {
                win_key::set_provider_suppression(false);
                let _ = start_menu::restore();
                return Err(error);
            }
            release_previous(&prev);
            *prev = combo.to_string();
            Ok(())
        } else {
            // Register the new combo first; only release the old one after
            // success, and never touch the hook unless everything worked.
            host.register_shortcut(combo)?;
            if let Err(error) = win_key::set_enabled(false) {
                host.unregister_shortcut(combo);
                return Err(error);
            }
            if let Err(error) = start_menu::restore() {
                let _ = win_key::set_enabled(true);
                host.unregister_shortcut(combo);
                return Err(error);
            }
            win_key::set_provider_suppression(false);
            release_previous(&prev);
            *prev = combo.to_string();
            Ok(())
        }
    }

    pub async fn set_shortcut(&'static self, combo: String) -> Result<(), String> {
        validate_shortcut(&combo)?;
        let generation = self.shortcut_generation.fetch_add(1, Ordering::AcqRel) + 1;
        crate::runtime::spawn_blocking(move || self.apply_shortcut_if_current(&combo, generation))
            .await
            .map_err(|error| join_error("shortcut task failed", error))?
    }

    pub fn load_state(&self) -> Result<serde_json::Value, String> {
        Ok(load_state_value()?.unwrap_or(serde_json::Value::Null))
    }

    /// Persists settings atomically after validating every value.
    pub fn save_state(&self, state: serde_json::Value) -> Result<(), String> {
        validate_state(&state)?;
        let mut value = state;
        value["version"] = serde_json::json!(STATE_VERSION);
        let dir = crate::host::data_dir();
        persist_state_value(&dir, &value)
    }

    /// Tears down interception before exit. Tearing down the Win-key bridge can
    /// take up to two seconds (pump stop handshake); call it off the UI thread.
    pub fn teardown_for_exit(&self) {
        taskbar::release_on_exit();
        let _ = win_key::set_enabled(false);
        let _ = start_menu::restore();
        win_key::set_provider_suppression(false);
        // Last: the snapshot is a write of about 100 MB, and the shell hooks
        // above must be released first.
        self.file_index.flush_for_exit();
    }
}

fn schedule_startup_taskbar_alignment() {
    crate::runtime::spawn(async move {
        // A launch at sign-in can come before Explorer's taskbar exists.
        for _ in 0..STARTUP_SHELL_RETRY_ATTEMPTS {
            if crate::shell_bridge::shell_windows_ready() {
                break;
            }
            tokio::time::sleep(STARTUP_SHELL_RETRY_DELAY).await;
        }
        tokio::time::sleep(STARTUP_ALIGNMENT_DELAY).await;
        let _ = crate::runtime::spawn_blocking(move || {
            let _ = taskbar_alignment::reapply_current();
        })
        .await;
    });
}

pub fn filter_existing_paths(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .take(64)
        .filter(|value| value.len() <= 4096)
        .filter(|value| {
            let path = Path::new(value);
            path.is_absolute() && path.exists()
        })
        .collect()
}


/// Reads persisted state and performs the one-time 0.3.3 shortcut migration.
/// Version 2 settings are preserved except for the shortcut, which the 0.3.3
/// installer contract intentionally changes to the new Win default.
pub fn load_state_value() -> Result<Option<serde_json::Value>, String> {
    let dir = crate::host::data_dir();
    let path = dir.join("prism.json");
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("failed to read state: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("corrupt state file: {e}"))?;
    let Some((value, migrated)) = migrate_state_value(value)? else {
        return Ok(None);
    };
    if migrated {
        persist_state_value(&dir, &value)?;
    }
    Ok(Some(value))
}


pub fn persist_state_value(dir: &Path, value: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    crate::win32::write_atomic(&dir.join("prism.json"), text.as_bytes()).map_err(|e| format!("failed to save state: {e}"))
}
