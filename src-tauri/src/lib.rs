mod apps;
mod files;
mod perf;
mod start_menu;
mod taskbar;
mod theme;
mod win_key;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{
    window::{Color, Effect, EffectsBuilder},
    Emitter, Manager, Theme, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, SetForegroundWindow, SetWindowPos, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW,
};

/// The only accepted global shortcuts. Bare typing keys, reserved keys and
/// known system/security combos are never accepted.
const ALLOWED_SHORTCUTS: &[&str] = &[
    "Win", // standalone Win-key interception and first-run default
    "Ctrl+Alt+Space",
    "Ctrl+Alt+S",
    "Ctrl+Alt+Shift+S",
    "Ctrl+Alt+P",
    "Ctrl+Alt+Shift+P",
    "Ctrl+Alt+Enter",
    "Ctrl+Alt+Shift+Enter",
];

const DEFAULT_SHORTCUT: &str = "Win";
const LEGACY_STATE_VERSION: u32 = 2;
const STATE_VERSION: u32 = 3;
const PALETTE_HIDE_DELAY: Duration = Duration::from_millis(125);
const PALETTE_RAISE_RETRY_DELAY: Duration = Duration::from_millis(40);

static PALETTE_OPEN: AtomicBool = AtomicBool::new(false);
static PALETTE_TRANSITION: AtomicU64 = AtomicU64::new(0);

pub struct AppState {
    apps_cache: Mutex<Option<Vec<apps::AppEntry>>>,
    file_index: files::FileIndex,
    shortcut: Mutex<String>,
    effect: Mutex<String>,
    theme: Mutex<String>,
}

/// Handles the internal crash-recovery mode before Tauri or the single-instance
/// plugin starts. Returns true when this process was launched as the watchdog.
pub fn run_start_restore_watchdog_if_requested() -> bool {
    start_menu::run_watchdog_from_args()
}

pub fn run() {
    let startup_timer = perf::start();
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // A second launch just brings the running instance forward.
            if let Some(window) = app.get_webview_window("main") {
                if window.is_visible().unwrap_or(false) {
                    let _ = window.set_focus();
                }
            }
        }))
        .manage(AppState {
            apps_cache: Mutex::new(None),
            file_index: files::FileIndex::default(),
            shortcut: Mutex::new(String::new()),
            effect: Mutex::new("solid".to_string()),
            theme: Mutex::new("dark".to_string()),
        })
        .setup(move |app| {
            let _ = start_menu::recover_stale(app.handle());
            win_key::init(app.handle().clone());
            theme::watch(app.handle().clone());
            let file_cache = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("files.json");
            files::warm(
                app.state::<AppState>().file_index.clone(),
                file_cache,
                app.handle().clone(),
            );
            if let Some(window) = app.get_webview_window("main") {
                let _ = apply_window_look(&window, "solid", "dark");
            }
            // Install the persisted shortcut before the webview loads. A
            // fresh, corrupt, or incomplete state defaults natively to Win,
            // so first-run activation never waits on frontend startup.
            let persisted = load_state_value(app.handle()).ok().flatten();
            let combo = startup_shortcut(persisted.as_ref());
            let _ = apply_shortcut(app.handle(), &combo);
            perf::finish(startup_timer, "startup_setup", String::new);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Clicking away dismisses the launcher, like Raycast.
            if window.label() == "main"
                && matches!(event, WindowEvent::Focused(false))
                && window.is_visible().unwrap_or(false)
                && !win_key::is_start_guard_active()
            {
                PALETTE_OPEN.store(false, Ordering::Release);
                PALETTE_TRANSITION.fetch_add(1, Ordering::AcqRel);
                taskbar::release();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_apps,
            refresh_apps,
            search_files,
            get_quick_access,
            existing_paths,
            launch_app,
            open_path,
            present_palette,
            hide_palette,
            set_window_effect,
            set_window_theme,
            set_window_width,
            get_system_theme,
            set_shortcut,
            load_state,
            save_state,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Prism");
}

/* ---------------- window lifecycle ---------------- */

/// Bottom-anchored placement on the monitor the cursor is on (falling back
/// to the window's monitor), clamped to the work area so small screens and
/// large taskbars never cut the palette off.
fn position_palette(window: &tauri::WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    unsafe {
        // Active-monitor placement: prefer the monitor under the cursor,
        // like the real Start menu; fall back to the window's monitor.
        let mut pt = POINT::default();
        let monitor = if GetCursorPos(&mut pt).is_ok() {
            MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(HWND(hwnd.0), MONITOR_DEFAULTTONEAREST)
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
    }
    let work = info.rcWork;
    let mut width = size.width as i32;
    let mut height = size.height as i32;
    // Clamp to the work area (small screens, huge taskbars, 200% DPI).
    let work_w = work.right - work.left;
    let work_h = work.bottom - work.top;
    if width > work_w {
        width = work_w;
        let _ = window.set_size(tauri::PhysicalSize::new(width as u32, height as u32));
    }
    if height > work_h {
        height = work_h;
        let _ = window.set_size(tauri::PhysicalSize::new(width as u32, height as u32));
    }
    let x = work.left + (work_w - width) / 2;
    let y = work.bottom - height;
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

/// Reasserts Prism at the front of the topmost band. `alwaysOnTop` keeps the
/// window in that band, but an already-active topmost window can still sit
/// above it until Prism is explicitly repositioned.
fn raise_palette(window: &tauri::WebviewWindow) -> Result<(), String> {
    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    unsafe {
        SetWindowPos(
            HWND(hwnd.0),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        )
        .map_err(|error| error.to_string())?;
        // The Win press is the user's current input, so Windows normally grants
        // this foreground request. SetWindowPos still fixes visibility if focus
        // is restricted by another process.
        let _ = SetForegroundWindow(HWND(hwnd.0));
    }
    window.set_focus().map_err(|error| error.to_string())
}

fn schedule_palette_raise_retry(app: &tauri::AppHandle, transition: u64) {
    let retry_app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(PALETTE_RAISE_RETRY_DELAY).await;
        if PALETTE_TRANSITION.load(Ordering::Acquire) != transition
            || !PALETTE_OPEN.load(Ordering::Acquire)
        {
            return;
        }
        let main_thread_app = retry_app.clone();
        let _ = retry_app.run_on_main_thread(move || {
            if PALETTE_TRANSITION.load(Ordering::Acquire) == transition
                && PALETTE_OPEN.load(Ordering::Acquire)
            {
                if let Some(window) = main_thread_app.get_webview_window("main") {
                    let _ = raise_palette(&window);
                }
            }
        });
    });
}

pub(crate) fn toggle_palette(app: &tauri::AppHandle) {
    let timer = perf::start();
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let opening = toggle_open_state(&PALETTE_OPEN);
    let transition = PALETTE_TRANSITION.fetch_add(1, Ordering::AcqRel) + 1;
    if !opening {
        let close_app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(PALETTE_HIDE_DELAY).await;
            if PALETTE_TRANSITION.load(Ordering::Acquire) != transition
                || PALETTE_OPEN.load(Ordering::Acquire)
            {
                return;
            }
            let hide_app = close_app.clone();
            let _ = close_app.run_on_main_thread(move || {
                if PALETTE_TRANSITION.load(Ordering::Acquire) == transition
                    && !PALETTE_OPEN.load(Ordering::Acquire)
                {
                    if let Some(window) = hide_app.get_webview_window("main") {
                        let _ = window.hide();
                        taskbar::release();
                    }
                }
            });
        });
    }
    // Send the desired state, not an ambiguous toggle, so native and webview
    // state cannot diverge if an event is delayed.
    let _ = window.emit("prism-toggle", opening);
    perf::finish(timer, "palette_toggle", || format!("open={opening}"));
}

fn toggle_open_state(open: &AtomicBool) -> bool {
    !open.fetch_xor(true, Ordering::AcqRel)
}

#[tauri::command]
fn present_palette(app: tauri::AppHandle) -> Result<bool, String> {
    if !PALETTE_OPEN.load(Ordering::Acquire) {
        return Ok(false);
    }
    let timer = perf::start();
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    taskbar::present();
    position_palette(&window);
    window.show().map_err(|error| error.to_string())?;
    raise_palette(&window)?;
    schedule_palette_raise_retry(&app, PALETTE_TRANSITION.load(Ordering::Acquire));
    perf::finish(timer, "palette_present", || "window=main".to_string());
    Ok(true)
}

#[tauri::command]
fn hide_palette(app: tauri::AppHandle) -> Result<(), String> {
    PALETTE_OPEN.store(false, Ordering::Release);
    PALETTE_TRANSITION.fetch_add(1, Ordering::AcqRel);
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window.hide().map_err(|error| error.to_string())?;
    taskbar::release();
    Ok(())
}

fn register_shortcut(app: &tauri::AppHandle, combo: &str) -> Result<(), String> {
    let shortcut: Shortcut = combo
        .parse()
        .map_err(|e| format!("invalid shortcut '{combo}': {e}"))?;
    let app = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                toggle_palette(app);
            }
        })
        .map_err(|e| format!("failed to register shortcut: {e}"))?;
    Ok(())
}

fn apply_window_look(
    window: &tauri::WebviewWindow,
    effect: &str,
    theme: &str,
) -> Result<(), String> {
    let (tint, mica) = if theme == "light" {
        (Color(238, 240, 244, 190), Effect::MicaLight)
    } else {
        (Color(20, 20, 26, 165), Effect::MicaDark)
    };
    let build = |effect: Effect| {
        EffectsBuilder::new()
            .effect(effect)
            .radius(12.0)
            .color(tint)
            .build()
    };
    let attempt = |effect: Effect| window.set_effects(build(effect)).is_ok();

    // Fall back down the material ladder when a blur type is unsupported.
    match effect {
        "acrylic" => {
            if attempt(Effect::Acrylic) || attempt(mica) || attempt(Effect::Blur) {
                Ok(())
            } else {
                window
                    .set_effects(EffectsBuilder::new().clear_effects().build())
                    .map_err(|e| e.to_string())
            }
        }
        "mica" => {
            if attempt(mica) || attempt(Effect::Acrylic) || attempt(Effect::Blur) {
                Ok(())
            } else {
                window
                    .set_effects(EffectsBuilder::new().clear_effects().build())
                    .map_err(|e| e.to_string())
            }
        }
        "solid" => window
            .set_effects(EffectsBuilder::new().clear_effects().build())
            .map_err(|e| e.to_string()),
        _ => Err(format!("unknown effect '{effect}'")),
    }
}

/* ---------------- commands ---------------- */

#[tauri::command]
async fn get_apps(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<apps::AppEntry>, String> {
    let timer = perf::start();
    eprintln!("[get_apps] called");
    // Fast path: already scanned this session.
    if let Some(list) = state.apps_cache.lock().map_err(|e| e.to_string())?.clone() {
        eprintln!("[get_apps] cache hit: {}", list.len());
        perf::finish(timer, "get_apps_cache", || format!("count={}", list.len()));
        return Ok(list);
    }
    // Off the main thread: scanning spawns PowerShell and does icon work.
    let cache_path = apps_cache_path(&app);
    eprintln!("[get_apps] scanning, cache={cache_path:?}");
    let list = tauri::async_runtime::spawn_blocking(move || apps::scan(&cache_path))
        .await
        .map_err(|e| format!("app scan task failed: {e}"))??;
    eprintln!("[get_apps] scan done: {} apps", list.len());
    *state.apps_cache.lock().map_err(|e| e.to_string())? = Some(list.clone());
    perf::finish(timer, "get_apps_scan", || format!("count={}", list.len()));
    Ok(list)
}

#[tauri::command]
async fn refresh_apps(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<apps::AppEntry>, String> {
    eprintln!("[refresh_apps] called");
    let cache_path = apps_cache_path(&app);
    let list = tauri::async_runtime::spawn_blocking(move || apps::scan_force(&cache_path))
        .await
        .map_err(|e| format!("app scan task failed: {e}"))??;
    eprintln!("[refresh_apps] done: {} apps", list.len());
    *state.apps_cache.lock().map_err(|e| e.to_string())? = Some(list.clone());
    Ok(list)
}

fn apps_cache_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("apps.json")
}

#[tauri::command]
fn launch_app(id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let timer = perf::start();
    // Only launch apps that came from our own scan.
    let apps = state.apps_cache.lock().map_err(|e| e.to_string())?;
    let entry = apps
        .as_ref()
        .and_then(|list| list.iter().find(|a| a.app_id == id))
        .ok_or_else(|| "unknown app id".to_string())?;
    let result = apps::launch(entry);
    perf::finish(timer, "launch_app", || {
        format!("name={};source={}", entry.name, entry.source)
    });
    result
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let timer = perf::start();
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.exists() {
        return Err("path must be an existing absolute file or folder".to_string());
    }
    let result = apps::open_path(&path);
    perf::finish(timer, "open_path", || {
        format!("kind={}", if path.is_dir() { "directory" } else { "file" })
    });
    result
}

#[tauri::command]
async fn search_files(
    query: String,
    limit: Option<usize>,
    state: tauri::State<'_, AppState>,
) -> Result<files::FileSearchResponse, String> {
    let timer = perf::start();
    let query_length = query.chars().count();
    let index = state.file_index.clone();
    let result = tauri::async_runtime::spawn_blocking(move || index.search(&query, limit))
        .await
        .map_err(|error| format!("file search task failed: {error}"));
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

#[tauri::command]
fn get_quick_access() -> Vec<files::QuickAccessEntry> {
    files::quick_access()
}

#[tauri::command]
fn existing_paths(paths: Vec<String>) -> Vec<String> {
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

#[tauri::command]
fn set_window_effect(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    effect: String,
) -> Result<(), String> {
    if !matches!(effect.as_str(), "acrylic" | "mica" | "solid") {
        return Err(format!("unknown effect '{effect}'"));
    }
    *state.effect.lock().map_err(|e| e.to_string())? = effect.clone();
    let theme = state.theme.lock().map_err(|e| e.to_string())?.clone();
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    apply_window_look(&window, &effect, &theme)
}

#[tauri::command]
fn set_window_width(app: tauri::AppHandle, width: u32) -> Result<(), String> {
    // Persisted choices remain the three discrete presets. Intermediate
    // values are accepted only inside their bounds so the frontend can
    // animate the native window without broadening the settings schema.
    if !is_animatable_window_width(width) {
        return Err(format!("unsupported width '{width}'"));
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    let height = window.outer_size().map_err(|e| e.to_string())?.height;
    window
        .set_size(tauri::PhysicalSize::new(width, height))
        .map_err(|e| e.to_string())?;
    position_palette(&window);
    Ok(())
}

fn is_animatable_window_width(width: u32) -> bool {
    (560..=720).contains(&width)
}

#[tauri::command]
fn set_window_theme(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    theme: String,
) -> Result<(), String> {
    if !matches!(theme.as_str(), "light" | "dark") {
        return Err(format!("unknown theme '{theme}'"));
    }
    *state.theme.lock().map_err(|e| e.to_string())? = theme.clone();
    let effect = state.effect.lock().map_err(|e| e.to_string())?.clone();
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    let _ = window.set_theme(Some(if theme == "light" {
        Theme::Light
    } else {
        Theme::Dark
    }));
    apply_window_look(&window, &effect, &theme)
}

#[tauri::command]
fn get_system_theme() -> String {
    match theme::apps_light() {
        Some(true) => "light".to_string(),
        _ => "dark".to_string(),
    }
}

/// Validates a shortcut string against the whitelist and (for non-Win
/// combos) the platform parser.
fn validate_shortcut(combo: &str) -> Result<(), String> {
    if !ALLOWED_SHORTCUTS.contains(&combo) {
        return Err(format!(
            "shortcut '{combo}' is not supported; pick one of: {}",
            ALLOWED_SHORTCUTS.join(", ")
        ));
    }
    if combo != "Win" {
        combo
            .parse::<Shortcut>()
            .map_err(|_| format!("invalid shortcut '{combo}'"))?;
    }
    Ok(())
}

fn startup_shortcut(state: Option<&serde_json::Value>) -> String {
    state
        .and_then(|value| value.get("settings"))
        .and_then(|settings| settings.get("shortcut"))
        .and_then(|shortcut| shortcut.as_str())
        .filter(|shortcut| validate_shortcut(shortcut).is_ok())
        .unwrap_or(DEFAULT_SHORTCUT)
        .to_string()
}

/// Applies a global shortcut. The new binding is activated and proven
/// BEFORE the old one is released; on any failure the previous shortcut
/// stays active and an error is returned. State is only updated on success.
fn apply_shortcut(app: &tauri::AppHandle, combo: &str) -> Result<(), String> {
    validate_shortcut(combo)?;
    let state = app.state::<AppState>();
    let mut prev = state.shortcut.lock().map_err(|e| e.to_string())?;
    if *prev == combo {
        // Idempotent: already active.
        return Ok(());
    }

    let gs = app.global_shortcut();
    if combo == "Win" {
        // Standalone Win mode uses the native hook rather than a plugin
        // registration. If the hook later fails, it self-disables and the
        // frontend is notified.
        let provider_suppression = start_menu::enable(app)?;
        win_key::set_provider_suppression(provider_suppression);
        if let Err(error) = win_key::set_enabled(true) {
            win_key::set_provider_suppression(false);
            let _ = start_menu::restore(app);
            return Err(error);
        }
        if let Ok(old) = prev.parse::<Shortcut>() {
            let _ = gs.unregister(old);
        }
        *prev = combo.to_string();
        Ok(())
    } else {
        // Register the new combo first; only release the old one after
        // success, and never touch the hook unless everything worked.
        register_shortcut(app, combo)?;
        if let Err(error) = win_key::set_enabled(false) {
            if let Ok(shortcut) = combo.parse::<Shortcut>() {
                let _ = gs.unregister(shortcut);
            }
            return Err(error);
        }
        if let Err(error) = start_menu::restore(app) {
            let _ = win_key::set_enabled(true);
            if let Ok(shortcut) = combo.parse::<Shortcut>() {
                let _ = gs.unregister(shortcut);
            }
            return Err(error);
        }
        win_key::set_provider_suppression(false);
        if let Ok(old) = prev.parse::<Shortcut>() {
            let _ = gs.unregister(old);
        }
        *prev = combo.to_string();
        Ok(())
    }
}

#[tauri::command]
fn set_shortcut(app: tauri::AppHandle, combo: String) -> Result<(), String> {
    apply_shortcut(&app, &combo)
}

/// Reads persisted state and performs the one-time 0.3.3 shortcut migration.
/// Version 2 settings are preserved except for the shortcut, which the 0.3.3
/// installer contract intentionally changes to the new Win default.
fn load_state_value(app: &tauri::AppHandle) -> Result<Option<serde_json::Value>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
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

fn migrate_state_value(
    mut value: serde_json::Value,
) -> Result<Option<(serde_json::Value, bool)>, String> {
    let version = value.get("version").and_then(|v| v.as_u64());
    if version == Some(STATE_VERSION as u64) {
        return Ok(Some((value, false)));
    }
    if version != Some(LEGACY_STATE_VERSION as u64) {
        return Ok(None);
    }

    let settings = value
        .get_mut("settings")
        .and_then(|settings| settings.as_object_mut())
        .ok_or_else(|| "legacy state.settings must be an object".to_string())?;
    settings.insert("shortcut".to_string(), serde_json::json!(DEFAULT_SHORTCUT));
    value["version"] = serde_json::json!(STATE_VERSION);
    validate_state(&value)?;
    Ok(Some((value, true)))
}

#[tauri::command]
fn load_state(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    Ok(load_state_value(&app)?.unwrap_or(serde_json::Value::Null))
}

/// Persists settings atomically after validating every value.
#[tauri::command]
fn save_state(app: tauri::AppHandle, state: serde_json::Value) -> Result<(), String> {
    validate_state(&state)?;
    let mut value = state;
    value["version"] = serde_json::json!(STATE_VERSION);
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    persist_state_value(&dir, &value)
}

fn persist_state_value(dir: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("failed to create data dir: {e}"))?;
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    let final_path = dir.join("prism.json");
    let tmp_path = dir.join("prism.json.tmp");
    std::fs::write(&tmp_path, text).map_err(|e| format!("failed to save state: {e}"))?;
    files::replace_file(&tmp_path, &final_path).map_err(|e| format!("failed to commit state: {e}"))
}

/// Validates the persisted state shape; returns Err on anything unexpected.
fn validate_state(state: &serde_json::Value) -> Result<(), String> {
    let obj = state.as_object().ok_or("state must be an object")?;
    let settings = obj
        .get("settings")
        .and_then(|v| v.as_object())
        .ok_or("state.settings must be an object")?;
    if let Some(accent) = settings.get("accent").and_then(|v| v.as_str()) {
        if !matches!(accent, "iris" | "azure" | "mint" | "amber" | "rose") {
            return Err(format!("unknown accent '{accent}'"));
        }
    }
    if let Some(effect) = settings.get("effect").and_then(|v| v.as_str()) {
        if !matches!(effect, "acrylic" | "mica" | "solid") {
            return Err(format!("unknown effect '{effect}'"));
        }
    }
    if let Some(theme) = settings.get("theme").and_then(|v| v.as_str()) {
        if !matches!(theme, "system" | "dark" | "light") {
            return Err(format!("unknown theme '{theme}'"));
        }
    }
    if let Some(shortcut) = settings.get("shortcut").and_then(|v| v.as_str()) {
        validate_shortcut(shortcut)?;
    }
    if let Some(width) = settings.get("width").and_then(|v| v.as_u64()) {
        if !matches!(width, 560 | 640 | 720) {
            return Err(format!("unsupported width '{width}'"));
        }
    }
    if let Some(zoom) = settings.get("viewZoom") {
        let zoom = zoom
            .as_u64()
            .ok_or("state.settings.viewZoom must be an integer")?;
        if !(70..=150).contains(&zoom) || zoom % 10 != 0 {
            return Err(format!("unsupported view zoom '{zoom}'"));
        }
    }
    if let Some(history) = obj.get("history") {
        let Some(entries) = history.as_array() else {
            return Err("state.history must be an array".to_string());
        };
        if entries.len() > 20 {
            return Err("state.history has too many entries".to_string());
        }
        for entry in entries {
            let Some(entry) = entry.as_object() else {
                return Err("history entries must be objects".to_string());
            };
            let valid_id = entry
                .get("id")
                .and_then(|value| value.as_str())
                .is_some_and(|value| !value.is_empty() && value.len() <= 4096);
            let valid_title = entry
                .get("title")
                .and_then(|value| value.as_str())
                .is_some_and(|value| !value.is_empty() && value.len() <= 512);
            let valid_timestamp = entry.get("ts").is_some_and(|value| value.is_u64());
            if !valid_id || !valid_title || !valid_timestamp {
                return Err("invalid history entry".to_string());
            }
        }
    }
    Ok(())
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    // Ensure interception is fully torn down before exiting.
    taskbar::release();
    let _ = win_key::set_enabled(false);
    let _ = start_menu::restore(&app);
    win_key::set_provider_suppression(false);
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_shortcuts_parse_and_disallowed_are_rejected() {
        for combo in ALLOWED_SHORTCUTS {
            assert!(validate_shortcut(combo).is_ok(), "should allow {combo}");
        }
        for bad in [
            "E",
            "Space",
            "Win+L",
            "Ctrl+Alt+Delete",
            "Ctrl+Shift+Esc",
            "F4",
            "Super+Space",
            "Alt+Space",
            "",
            "Ctrl+Win+E",
        ] {
            assert!(validate_shortcut(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn startup_shortcut_defaults_to_win_and_preserves_valid_choices() {
        assert_eq!(startup_shortcut(None), DEFAULT_SHORTCUT);
        assert_eq!(
            startup_shortcut(Some(&serde_json::json!({
                "settings": { "shortcut": "Ctrl+Alt+Space" }
            }))),
            "Ctrl+Alt+Space"
        );
        assert_eq!(
            startup_shortcut(Some(&serde_json::json!({
                "settings": { "shortcut": "Win+L" }
            }))),
            DEFAULT_SHORTCUT
        );
    }

    #[test]
    fn version_two_state_migrates_only_the_shortcut_once() {
        let legacy = serde_json::json!({
            "version": LEGACY_STATE_VERSION,
            "settings": {
                "accent": "mint",
                "width": 720,
                "viewZoom": 110,
                "effect": "mica",
                "shortcut": "Ctrl+Alt+Space",
                "alwaysOnTop": false,
                "theme": "dark"
            },
            "history": [{ "id": "app::test", "title": "Test", "ts": 7 }]
        });
        let (migrated, changed) = migrate_state_value(legacy).unwrap().unwrap();
        assert!(changed);
        assert_eq!(migrated["version"], STATE_VERSION);
        assert_eq!(migrated["settings"]["shortcut"], DEFAULT_SHORTCUT);
        assert_eq!(migrated["settings"]["accent"], "mint");
        assert_eq!(migrated["history"][0]["id"], "app::test");

        let (unchanged, changed_again) = migrate_state_value(migrated).unwrap().unwrap();
        assert!(!changed_again);
        assert_eq!(unchanged["settings"]["shortcut"], DEFAULT_SHORTCUT);
    }

    #[test]
    fn repeated_toggle_state_alternates_open_and_closed() {
        let open = AtomicBool::new(false);
        assert!(toggle_open_state(&open));
        assert!(!toggle_open_state(&open));
        assert!(toggle_open_state(&open));
        assert!(!toggle_open_state(&open));
    }

    #[test]
    fn window_width_animation_stays_inside_preset_bounds() {
        for width in [560, 561, 639, 640, 719, 720] {
            assert!(is_animatable_window_width(width));
        }
        for width in [0, 559, 721, u32::MAX] {
            assert!(!is_animatable_window_width(width));
        }
    }

    #[test]
    fn state_validation_accepts_good_and_rejects_bad() {
        let good = serde_json::json!({
            "settings": {
                "accent": "iris",
                "width": 640,
                "viewZoom": 100,
                "effect": "solid",
                "shortcut": "Ctrl+Alt+Space",
                "alwaysOnTop": true,
                "theme": "system"
            },
            "history": []
        });
        assert!(validate_state(&good).is_ok());

        for patch in [
            serde_json::json!({"settings": {"accent": "neon"}}),
            serde_json::json!({"settings": {"effect": "hologram"}}),
            serde_json::json!({"settings": {"theme": "sepia"}}),
            serde_json::json!({"settings": {"shortcut": "X"}}),
            serde_json::json!({"settings": {"width": 999}}),
            serde_json::json!({"settings": {"viewZoom": 135}}),
            serde_json::json!({"settings": {"viewZoom": "large"}}),
            serde_json::json!({"settings": []}),
            serde_json::json!({"history": "nope"}),
            serde_json::json!({
                "settings": good["settings"].clone(),
                "history": [{"id": 4, "title": "bad", "ts": 1}]
            }),
        ] {
            assert!(validate_state(&patch).is_err(), "should reject {patch}");
        }
    }

    #[test]
    fn existing_paths_rejects_deleted_and_relative_entries() {
        let path = std::env::temp_dir().join(format!(
            "prism-existing-path-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&path, "test").expect("create test file");
        let path_text = path.to_string_lossy().into_owned();
        assert_eq!(
            existing_paths(vec![path_text.clone(), "relative.txt".to_string()]),
            vec![path_text.clone()]
        );
        std::fs::remove_file(&path).expect("remove test file");
        assert!(existing_paths(vec![path_text]).is_empty());
    }
}
