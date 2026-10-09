//! The Tauri host for the Prism palette. The backend lives in `prism-core`;
//! this crate keeps the WebView window lifecycle and thin command wrappers.

pub use prism_core::{audio, audio_hook, audio_osd, drag};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prism_core::backend::{backend, startup_window_theme as startup_theme_name};
#[cfg(test)]
use prism_core::backend::{
    filter_existing_paths, migrate_state_value, startup_shortcut, startup_taskbar_alignment,
    validate_shortcut, validate_state, ALLOWED_SHORTCUTS, DEFAULT_SHORTCUT, LEGACY_STATE_VERSION,
    STATE_VERSION,
};
#[cfg(test)]
use prism_core::palette_window::{
    calculate_clamped_window_size, palette_position, PhysicalRect, TaskbarEdge, DEFAULT_LOGICAL_HEIGHT,
};
use prism_core::palette_window::{
    self, is_prism_foreground, log_foreground, presentation_anchor, PresentationAnchor,
    PresentationEvent, PresentationSource, DEFAULT_LOGICAL_WIDTH,
};
use prism_core::{apps, files, perf, power, query, taskbar, taskbar_alignment, taskbar_customization, theme, win_key};
use tauri::{Emitter, Manager, Theme, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use windows::Win32::Foundation::{HWND, POINT, RECT};

/// Keep this >= frontend LAUNCHER_MOTION_MS (110ms) so the webview can fade out.
const PALETTE_HIDE_DELAY: Duration = Duration::from_millis(125);
/// Reassert over ~500ms, not once: the Explorer bridge grants foreground
/// asynchronously and an elevated/fullscreen owner can deny the first
/// SetForegroundWindow. Each shot is skipped once the palette is already
/// foreground or a newer transition/close took over.
const PALETTE_RAISE_RETRY_DELAYS: &[u64] = &[40, 120, 250, 500];
/// Window during which a focus-lost event after opening is treated as part of
/// the activation burst instead of a click-away dismissal.
const ACTIVATION_FOCUS_GRACE: Duration = Duration::from_millis(600);
const PALETTE_STARTUP_DELAY: Duration = Duration::from_millis(100);

static PALETTE_OPEN: AtomicBool = AtomicBool::new(false);
static PALETTE_TRANSITION: AtomicU64 = AtomicU64::new(0);
static ACTIVATION_FOCUS_PENDING: AtomicBool = AtomicBool::new(false);
/// This open has already put Prism in the foreground. Raise retries exist to
/// win the initial denial from an elevated owner. Once that succeeded, a later
/// retry would steal Alt-Tab and launches back.
static PALETTE_TOOK_FOREGROUND: AtomicBool = AtomicBool::new(false);
static PRESENTATION_ANCHOR: Mutex<Option<PresentationAnchor>> = Mutex::new(None);
static LOGICAL_WIDTH: AtomicU32 = AtomicU32::new(DEFAULT_LOGICAL_WIDTH);

/// The `prism-core` host backed by the Tauri app handle.
struct TauriHost {
    app: tauri::AppHandle,
}

impl prism_core::host::Host for TauriHost {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.app.emit(event, payload);
    }

    fn app_data_dir(&self) -> Result<PathBuf, String> {
        self.app.path().app_data_dir().map_err(|error| error.to_string())
    }

    fn palette_visible(&self) -> bool {
        self.app
            .get_webview_window("main")
            .map(|window| window.is_visible().unwrap_or(true))
            .unwrap_or(true)
    }

    fn toggle_from_win(&self, start_button: Option<RECT>) {
        let app = self.app.clone();
        let _ = self
            .app
            .run_on_main_thread(move || toggle_palette_from_win(&app, start_button));
    }

    fn toggle_from_taskbar(&self, click: POINT, start_button: Option<RECT>) {
        let app = self.app.clone();
        let _ = self
            .app
            .run_on_main_thread(move || toggle_palette_from_taskbar(&app, click, start_button));
    }

    fn activate(&self) {
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || activate_palette(&app));
    }

    fn register_shortcut(&self, combo: &str) -> Result<(), String> {
        register_shortcut(&self.app, combo)
    }

    fn unregister_shortcut(&self, combo: &str) {
        if let Ok(shortcut) = combo.parse::<Shortcut>() {
            let _ = self.app.global_shortcut().unregister(shortcut);
        }
    }
}

/// Handles the internal crash-recovery mode before Tauri or the single-instance
/// plugin starts. Returns true when this process was launched as the watchdog.
pub fn run_start_restore_watchdog_if_requested() -> bool {
    prism_core::backend::run_start_restore_watchdog_if_requested()
}

pub fn run() {
    let startup_timer = perf::start();
    let show_on_start = !prism_core::backend::launched_for_autostart();
    tauri::Builder::default()
        // Register this before every other plugin so a relaunch is redirected
        // before another plugin can initialize a second app instance.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let activation_app = app.clone();
            let _ = app.run_on_main_thread(move || {
                activate_palette(&activation_app);
            });
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            prism_core::host::install(Arc::new(TauriHost {
                app: app.handle().clone(),
            }));
            let persisted = prism_core::backend::load_state_value().ok().flatten();
            if let Some(persisted_width) = persisted
                .as_ref()
                .and_then(|value| value.pointer("/settings/width"))
                .and_then(|value| value.as_u64())
            {
                if is_animatable_window_width(persisted_width as u32) {
                    LOGICAL_WIDTH.store(persisted_width as u32, Ordering::Release);
                }
            }
            if let Some(window) = app.get_webview_window("main") {
                let startup_theme = startup_window_theme(persisted.as_ref(), theme::apps_light());
                let _ = apply_window_style(&window, startup_theme);
                schedule_hidden_memory_trim(app.handle().clone());
            }
            backend().start(persisted.as_ref());
            if show_on_start {
                schedule_initial_palette(app.handle().clone());
            }
            perf::finish(startup_timer, "startup_setup", String::new);
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            // The activation guard exists because Windows may report the
            // palette as unfocused once before granting foreground
            // activation to an existing process. Once the palette actually
            // receives focus the pending state is stale: clear it so a real
            // later unfocus (alt-tab back into a game) is never swallowed,
            // which would leave the palette open and the taskbar topmost.
            if matches!(event, WindowEvent::Focused(true)) {
                PALETTE_TOOK_FOREGROUND.store(true, Ordering::Release);
                ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
                return;
            }
            // Clicking away dismisses the launcher. Keep the window up for
            // PALETTE_HIDE_DELAY so the webview can play the same exit as
            // Win-key close. Do not raise or steal focus; the click already
            // belongs to the other window.
            if matches!(event, WindowEvent::Focused(false)) && window.is_visible().unwrap_or(false)
            {
                if drag::is_dragging() {
                    return;
                }
                // WebView2 hosts child windows of its own; a focus event whose
                // foreground still belongs to Prism is an internal transition,
                // not a click-away.
                if is_prism_foreground() {
                    return;
                }
                // Windows may report the palette as unfocused once before
                // granting foreground activation to the existing process.
                if ACTIVATION_FOCUS_PENDING.swap(false, Ordering::AcqRel) {
                    return;
                }
                // A close already in flight owns the hide timer. Resetting it
                // here would stretch Win-key / Escape exits when blur follows.
                if !PALETTE_OPEN.load(Ordering::Acquire) {
                    return;
                }
                let transition = begin_palette_close();
                win_key::debug_trace("palette-hide-blur");
                let _ = window.emit(
                    "prism-toggle",
                    PresentationEvent {
                        open: false,
                        source: PresentationSource::FocusLoss,
                        anchor: None,
                        generation: transition,
                    },
                );
                schedule_palette_hide(window.app_handle().clone(), transition);
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_apps,
            get_app_icons,
            refresh_apps,
            query_phase1,
            query_power_plans,
            execute_action,
            accept_intent,
            focus_window,
            search_files,
            rebuild_file_index,
            get_file_thumbnails,
            get_quick_access,
            existing_paths,
            launch_app,
            launch_app_as_admin,
            open_path,
            open_path_location,
            run_path_as_admin,
            is_pinned_to_taskbar,
            set_taskbar_pinned,
            show_path_properties,
            start_file_drag,
            present_palette,
            hide_palette,
            set_window_style,
            set_window_width,
            set_taskbar_alignment,
            get_taskbar_settings,
            set_taskbar_thickness,
            set_taskbar_auto_hide,
            set_taskbar_combine_buttons,
            set_taskbar_start_icon,
            set_custom_start_icon,
            select_custom_start_icon,
            remove_custom_start_icon,
            get_system_theme,
            set_shortcut,
            load_state,
            save_state,
            perform_power_action,
            set_power_plan,
            set_taskbar_scroll_volume,
            set_osd_accent,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Prism");
}

fn schedule_initial_palette(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(PALETTE_STARTUP_DELAY).await;
        let main_thread_app = app.clone();
        let _ = app.run_on_main_thread(move || activate_palette(&main_thread_app));
    });
}

pub(crate) fn activate_palette(app: &tauri::AppHandle) {
    // A user-initiated launch should open the reusable palette without
    // creating another WebView window or relying on frontend timing.
    ACTIVATION_FOCUS_PENDING.store(true, Ordering::Release);
    if !PALETTE_OPEN.load(Ordering::Acquire) {
        toggle_palette(app);
    }
    let _ = present_palette(app.clone());
}

/* ---------------- window lifecycle ---------------- */

/// Positions the reusable window against the Start button's taskbar edge and
/// clamps it to the physical-pixel work area. Keyboard and shortcut fallback
/// placement remains bottom-centered on the active monitor.
fn position_palette(window: &tauri::WebviewWindow, anchor: Option<PresentationAnchor>) {
    let Ok((x, y)) = palette_target(window, anchor, taskbar_alignment::current()) else {
        return;
    };
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

fn reconcile_palette_position(
    window: &tauri::WebviewWindow,
    anchor: Option<PresentationAnchor>,
) -> Result<(), String> {
    let alignment = taskbar_alignment::current();
    let (x, y) = palette_target(window, anchor, alignment)?;
    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    taskbar_alignment::reapply_with_companion(taskbar_alignment::CompanionMove {
        window: HWND(hwnd.0),
        x,
        y,
    })
}

fn palette_target(
    window: &tauri::WebviewWindow,
    anchor: Option<PresentationAnchor>,
    alignment: taskbar_alignment::Alignment,
) -> Result<(i32, i32), String> {
    let hwnd = window
        .hwnd()
        .map_err(|error| format!("cannot resolve window handle: {error}"))?;
    let frame = palette_window::palette_frame(
        HWND(hwnd.0),
        anchor,
        alignment,
        LOGICAL_WIDTH.load(Ordering::Acquire),
    )?;
    // A failed query or resize is reported instead of positioning for a size
    // the window does not have.
    let size = window.outer_size().map_err(|error| error.to_string())?;
    if size.width as i32 != frame.width || size.height as i32 != frame.height {
        window
            .set_size(tauri::PhysicalSize::new(frame.width as u32, frame.height as u32))
            .map_err(|error| error.to_string())?;
    }
    Ok((frame.x, frame.y))
}

/// Reasserts Prism at the front of the topmost band. `alwaysOnTop` keeps the
/// window in that band, but an already-active topmost window can still sit
/// above it until Prism is explicitly repositioned.
fn raise_palette(window: &tauri::WebviewWindow) -> Result<(), String> {
    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    palette_window::raise(HWND(hwnd.0))?;
    window.set_focus().map_err(|error| error.to_string())
}

fn palette_raise_retry_wanted(open: bool, same_transition: bool, took_foreground: bool) -> bool {
    open && same_transition && !took_foreground
}

fn schedule_palette_raise_retry(app: &tauri::AppHandle, transition: u64) {
    for &delay_ms in PALETTE_RAISE_RETRY_DELAYS {
        let retry_app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            if !palette_raise_still_wanted(transition) {
                return;
            }
            let main_thread_app = retry_app.clone();
            let _ = retry_app.run_on_main_thread(move || {
                if !palette_raise_still_wanted(transition) {
                    return;
                }
                if is_prism_foreground() {
                    PALETTE_TOOK_FOREGROUND.store(true, Ordering::Release);
                    return;
                }
                if let Some(window) = main_thread_app.get_webview_window("main") {
                    let _ = raise_palette(&window);
                    if is_prism_foreground() {
                        PALETTE_TOOK_FOREGROUND.store(true, Ordering::Release);
                    }
                }
            });
        });
    }
}

fn palette_raise_still_wanted(transition: u64) -> bool {
    palette_raise_retry_wanted(
        PALETTE_OPEN.load(Ordering::Acquire),
        PALETTE_TRANSITION.load(Ordering::Acquire) == transition,
        PALETTE_TOOK_FOREGROUND.load(Ordering::Acquire),
    )
}

#[cfg(windows)]
fn set_webview_memory_target(window: &tauri::WebviewWindow, low: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    };
    use windows::core::Interface;

    let _ = window.with_webview(move |webview| unsafe {
        let Ok(core) = webview.controller().CoreWebView2() else {
            return;
        };
        let Ok(core) = core.cast::<ICoreWebView2_19>() else {
            return;
        };
        let level = if low {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
        } else {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
        };
        let _ = core.SetMemoryUsageTargetLevel(level);
    });
}

#[cfg(not(windows))]
fn set_webview_memory_target(_window: &tauri::WebviewWindow, _low: bool) {}

fn schedule_hidden_memory_trim(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if PALETTE_OPEN.load(Ordering::Acquire) {
            return;
        }
        let trim_app = app.clone();
        let _ = app.run_on_main_thread(move || {
            if !PALETTE_OPEN.load(Ordering::Acquire) {
                if let Some(window) = trim_app.get_webview_window("main") {
                    set_webview_memory_target(&window, true);
                }
            }
        });
    });
}

pub(crate) fn toggle_palette(app: &tauri::AppHandle) {
    toggle_palette_with_presentation(app, PresentationSource::ConfiguredShortcut, None);
}

pub(crate) fn toggle_palette_from_win(app: &tauri::AppHandle, start_button: Option<RECT>) {
    let anchor = start_button.map(|rect| presentation_anchor(Some(rect), None));
    toggle_palette_with_presentation(app, PresentationSource::WinKey, anchor);
}

pub(crate) fn toggle_palette_from_taskbar(
    app: &tauri::AppHandle,
    click_point: POINT,
    start_button: Option<RECT>,
) {
    let anchor = Some(presentation_anchor(start_button, Some(click_point)));
    toggle_palette_with_presentation(app, PresentationSource::TaskbarStartClick, anchor);
}

fn toggle_palette_with_presentation(
    app: &tauri::AppHandle,
    source: PresentationSource,
    anchor: Option<PresentationAnchor>,
) {
    let timer = perf::start();
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let opening = toggle_open_state(&PALETTE_OPEN);
    let transition = PALETTE_TRANSITION.fetch_add(1, Ordering::AcqRel) + 1;
    log_foreground(if opening {
        "toggle-open-before"
    } else {
        "toggle-close-before"
    });
    win_key::debug_trace(&format!("palette-toggle open={opening}"));
    if !opening {
        ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
    }
    if opening {
        PALETTE_TOOK_FOREGROUND.store(false, Ordering::Release);
        backend().schedule_installed_apps_refresh();
        set_webview_memory_target(&window, false);
        // Windows can report the palette as unfocused once while it is still
        // acquiring foreground from an elevated window; do not let that
        // spurious blur dismiss the palette before it appears.
        ACTIVATION_FOCUS_PENDING.store(true, Ordering::Release);
        let grace_transition = transition;
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(ACTIVATION_FOCUS_GRACE).await;
            if PALETTE_TRANSITION.load(Ordering::Acquire) == grace_transition {
                ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
            }
        });
        PRESENTATION_ANCHOR
            .lock()
            .map(|mut value| *value = anchor)
            .ok();
    }
    if !opening {
        PRESENTATION_ANCHOR
            .lock()
            .map(|mut value| *value = None)
            .ok();
        schedule_palette_hide(app.clone(), transition);
    }
    // Send the desired state, not an ambiguous toggle, so native and webview
    // state cannot diverge if an event is delayed.
    let _ = window.emit(
        "prism-toggle",
        PresentationEvent {
            open: opening,
            source,
            anchor,
            generation: transition,
        },
    );
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
    set_webview_memory_target(&window, false);
    taskbar::present();
    let anchor = PRESENTATION_ANCHOR.lock().ok().and_then(|value| *value);
    // Reconcile the taskbar and Prism together on every presentation. This
    // also covers webview refreshes and single-instance reopens, where the
    // frontend is repositioned but Explorer may have relaid out its children.
    if reconcile_palette_position(&window, anchor).is_err() {
        position_palette(&window, anchor);
    }
    window.show().map_err(|error| error.to_string())?;
    // A transient focus denial (elevated owner, Explorer still holding
    // activation) must not fail the whole presentation after the window is
    // already shown - that desyncs native-visible from the frontend phase.
    // The Explorer bridge plus the scheduled retries finish the job.
    let _ = raise_palette(&window);
    // If Prism could not take foreground itself (for example an elevated Task
    // Manager holds it), ask the injected Explorer bridge to hand it over.
    // Explorer handled the Win key, so it may still hold activation rights.
    if !is_prism_foreground() {
        if let Ok(hwnd) = window.hwnd() {
            let granted = win_key::request_foreground(hwnd.0 as isize);
            win_key::debug_trace(&format!(
                "explorer-fg granted={granted} self={}",
                is_prism_foreground()
            ));
        }
    }
    if is_prism_foreground() {
        PALETTE_TOOK_FOREGROUND.store(true, Ordering::Release);
    }
    schedule_palette_raise_retry(&app, PALETTE_TRANSITION.load(Ordering::Acquire));
    log_foreground("present-after");
    #[cfg(debug_assertions)]
    if let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) {
        win_key::debug_trace(&format!(
            "present-geom pos={},{} size={}x{} visible={} focused={}",
            pos.x,
            pos.y,
            size.width,
            size.height,
            window.is_visible().unwrap_or(false),
            window.is_focused().unwrap_or(false)
        ));
    }
    perf::finish(timer, "palette_present", || "window=main".to_string());
    Ok(true)
}

fn begin_palette_close() -> u64 {
    PALETTE_OPEN.store(false, Ordering::Release);
    ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
    let transition = PALETTE_TRANSITION.fetch_add(1, Ordering::AcqRel) + 1;
    PRESENTATION_ANCHOR
        .lock()
        .map(|mut value| *value = None)
        .ok();
    transition
}

fn schedule_palette_hide(app: tauri::AppHandle, transition: u64) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(PALETTE_HIDE_DELAY).await;
        if PALETTE_TRANSITION.load(Ordering::Acquire) != transition
            || PALETTE_OPEN.load(Ordering::Acquire)
        {
            return;
        }
        let hide_app = app.clone();
        let _ = app.run_on_main_thread(move || {
            if PALETTE_TRANSITION.load(Ordering::Acquire) == transition
                && !PALETTE_OPEN.load(Ordering::Acquire)
            {
                if let Some(window) = hide_app.get_webview_window("main") {
                    let _ = window.hide();
                    set_webview_memory_target(&window, true);
                    taskbar::release();
                }
            }
        });
    });
}

#[tauri::command]
fn hide_palette(app: tauri::AppHandle, defer: bool) -> Result<(), String> {
    win_key::debug_trace(&format!("palette-hide-command defer={defer}"));
    let transition = begin_palette_close();
    if defer {
        schedule_palette_hide(app, transition);
        return Ok(());
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window.hide().map_err(|error| error.to_string())?;
    set_webview_memory_target(&window, true);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowTheme {
    Light,
    Dark,
}

impl WindowTheme {
    fn from_request(value: &str) -> Result<Self, String> {
        match value {
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err(format!("unknown theme '{value}'")),
        }
    }

    fn as_tauri(self) -> Theme {
        match self {
            Self::Light => Theme::Light,
            Self::Dark => Theme::Dark,
        }
    }
}

fn startup_window_theme(
    state: Option<&serde_json::Value>,
    system_light: Option<bool>,
) -> WindowTheme {
    WindowTheme::from_request(startup_theme_name(state, system_light)).unwrap_or(WindowTheme::Dark)
}

fn enforce_solid_window_surface(window: &tauri::WebviewWindow) -> Result<(), String> {
    // `EffectsBuilder::clear_effects()` only empties a new config. Tauri
    // interprets `None` as the instruction to remove the active native
    // material from the HWND.
    window.set_effects(None).map_err(|error| error.to_string())
}

fn apply_window_style(window: &tauri::WebviewWindow, theme: WindowTheme) -> Result<(), String> {
    window
        .set_theme(Some(theme.as_tauri()))
        .map_err(|error| format!("apply native window theme: {error}"))?;
    enforce_solid_window_surface(window)
}

/* ---------------- commands (bodies in prism-core::backend) ---------------- */

#[tauri::command]
async fn get_apps() -> Result<Vec<apps::AppEntry>, String> {
    backend().get_apps().await
}

#[tauri::command]
fn get_app_icons(ids: Vec<String>) -> Result<std::collections::HashMap<String, String>, String> {
    backend().get_app_icons(ids)
}

#[tauri::command]
async fn refresh_apps() -> Result<Vec<apps::AppEntry>, String> {
    backend().refresh_apps().await
}

#[tauri::command]
async fn launch_app(id: String) -> Result<(), String> {
    backend().launch_app(id).await
}

#[tauri::command]
async fn launch_app_as_admin(id: String) -> Result<(), String> {
    backend().launch_app_as_admin(id).await
}

#[tauri::command]
async fn open_path(path: String) -> Result<(), String> {
    backend().open_path(path).await
}

#[tauri::command]
async fn open_path_location(path: String) -> Result<(), String> {
    backend().open_path_location(path).await
}

#[tauri::command]
async fn run_path_as_admin(path: String) -> Result<(), String> {
    backend().run_path_as_admin(path).await
}

#[tauri::command]
async fn is_pinned_to_taskbar(path: String) -> bool {
    backend().is_pinned_to_taskbar(path).await
}

#[tauri::command]
async fn set_taskbar_pinned(path: String, pinned: bool) -> Result<(), String> {
    backend().set_taskbar_pinned(path, pinned).await
}

#[tauri::command]
async fn show_path_properties(path: String) -> Result<(), String> {
    backend().show_path_properties(path).await
}

#[tauri::command]
async fn start_file_drag(app: tauri::AppHandle, paths: Vec<String>) -> Result<bool, String> {
    let path_bufs = backend().drag_paths(paths)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = drag::drag_files(&path_bufs);
        let _ = tx.send(result);
    })
    .map_err(|e| format!("failed to schedule drag on main thread: {e}"))?;

    let drag_result = rx.await.map_err(|_| "drag task cancelled".to_string())??;
    let dropped = drag_result == drag::DragResult::Dropped;
    if dropped {
        let hide_app = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = hide_palette(hide_app, false);
        });
    }
    Ok(dropped)
}

#[tauri::command]
fn query_phase1(query: String, recent: Vec<query::RecentCommand>) -> query::Phase1Response {
    backend().query_phase1(query, recent)
}

#[tauri::command]
async fn query_power_plans(query: String) -> Result<Vec<power::PowerPlanHit>, String> {
    backend().query_power_plans(query).await
}

#[tauri::command]
async fn execute_action(id: String) -> Result<(), String> {
    backend().execute_action(id).await
}

#[tauri::command]
fn accept_intent(query: String, action_id: String) -> Result<(), String> {
    backend().accept_intent(query, action_id)
}

#[tauri::command]
fn focus_window(hwnd: i64) -> Result<(), String> {
    backend().focus_window(hwnd)
}

/// Phase 2: SQLite FTS5 file search. The UI must not block Phase 1 on this.
#[tauri::command]
async fn search_files(
    query: String,
    limit: Option<usize>,
) -> Result<files::FileSearchResponse, String> {
    backend().search_files(query, limit).await
}

#[tauri::command]
async fn rebuild_file_index() -> Result<(), String> {
    backend().rebuild_file_index();
    Ok(())
}

#[tauri::command]
async fn get_file_thumbnails(paths: Vec<String>) -> Vec<Option<String>> {
    backend().get_file_thumbnails(paths).await
}

#[tauri::command]
async fn get_quick_access() -> Vec<files::QuickAccessEntry> {
    backend().get_quick_access().await
}

#[tauri::command]
async fn existing_paths(paths: Vec<String>) -> Vec<String> {
    backend().existing_paths(paths).await
}

#[tauri::command]
async fn perform_power_action(action: String) -> Result<(), String> {
    backend().perform_power_action(action).await
}

#[tauri::command]
async fn set_power_plan(guid: String) -> Result<(), String> {
    backend().set_power_plan(guid).await
}

#[tauri::command]
fn set_window_width(app: tauri::AppHandle, width: u32) -> Result<(), String> {
    // Persisted choices remain the three discrete presets. Intermediate
    // values are accepted only inside their bounds so the frontend can
    // animate the native window without broadening the settings schema.
    if !is_animatable_window_width(width) {
        return Err(format!("unsupported width '{width}'"));
    }
    LOGICAL_WIDTH.store(width, Ordering::Release);
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    // Sizing flows through the same reconcile path as presentation so the
    // new width is converted with the target monitor's scale factor and
    // clamped against its work area.
    let anchor = PRESENTATION_ANCHOR.lock().ok().and_then(|value| *value);
    if reconcile_palette_position(&window, anchor).is_err() {
        position_palette(&window, anchor);
    }
    Ok(())
}

#[tauri::command]
fn set_taskbar_alignment(app: tauri::AppHandle, alignment: String) -> Result<(), String> {
    let alignment = taskbar_alignment::Alignment::parse(&alignment)?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    let anchor = PRESENTATION_ANCHOR.lock().ok().and_then(|value| *value);
    let (x, y) = palette_target(&window, anchor, alignment)?;
    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    taskbar_alignment::set_with_companion(
        alignment,
        taskbar_alignment::CompanionMove {
            window: HWND(hwnd.0),
            x,
            y,
        },
    )
}

#[tauri::command]
async fn get_taskbar_settings() -> Result<taskbar_customization::TaskbarSettings, String> {
    backend().get_taskbar_settings().await
}

#[tauri::command]
fn set_taskbar_thickness(value: String) -> Result<(), String> {
    backend().set_taskbar_thickness(value)
}

#[tauri::command]
fn set_taskbar_auto_hide(enabled: bool) -> Result<(), String> {
    backend().set_taskbar_auto_hide(enabled)
}

#[tauri::command]
fn set_taskbar_combine_buttons(value: String) -> Result<(), String> {
    backend().set_taskbar_combine_buttons(value)
}

#[tauri::command]
fn set_taskbar_scroll_volume(enabled: bool) {
    backend().set_taskbar_scroll_volume(enabled);
}

#[tauri::command]
fn set_osd_accent(accent: String) {
    backend().set_osd_accent(accent);
}

#[tauri::command]
async fn set_custom_start_icon(base64_png: String) -> Result<(), String> {
    backend().set_custom_start_icon(base64_png).await
}

#[tauri::command]
async fn select_custom_start_icon(id: String) -> Result<(), String> {
    backend().select_custom_start_icon(id).await
}

#[tauri::command]
async fn remove_custom_start_icon(id: String) -> Result<(), String> {
    backend().remove_custom_start_icon(id).await
}

#[tauri::command]
async fn set_taskbar_start_icon(value: String) -> Result<(), String> {
    backend().set_taskbar_start_icon(value).await
}

fn is_animatable_window_width(width: u32) -> bool {
    (560..=720).contains(&width)
}

/// Applies the window style in one IPC round-trip: native theme plus the
/// solid-only surface rule.
#[tauri::command]
fn set_window_style(app: tauri::AppHandle, theme: String) -> Result<(), String> {
    let theme = WindowTheme::from_request(&theme)?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    apply_window_style(&window, theme)
}

#[tauri::command]
fn get_system_theme() -> String {
    backend().get_system_theme()
}

#[tauri::command]
async fn set_shortcut(combo: String) -> Result<(), String> {
    backend().set_shortcut(combo).await
}

#[tauri::command]
fn load_state() -> Result<serde_json::Value, String> {
    backend().load_state()
}

/// Persists settings atomically after validating every value.
#[tauri::command]
fn save_state(state: serde_json::Value) -> Result<(), String> {
    backend().save_state(state)
}

#[tauri::command]
async fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    // Ensure interception is fully torn down before exiting. Tearing down the
    // Win-key bridge can take up to two seconds (pump stop handshake); it runs
    // off the main thread so quitting never freezes the UI.
    tauri::async_runtime::spawn_blocking(|| backend().teardown_for_exit())
        .await
        .map_err(|error| format!("quit teardown task failed: {error}"))?;
    app.exit(0);
    Ok(())
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
    fn startup_taskbar_alignment_is_available_before_frontend_state_loads() {
        assert_eq!(startup_taskbar_alignment(None), "center");
        for alignment in ["left", "center", "right"] {
            assert_eq!(
                startup_taskbar_alignment(Some(&serde_json::json!({
                    "settings": { "taskbarAlignment": alignment }
                }))),
                alignment
            );
        }
        assert_eq!(
            startup_taskbar_alignment(Some(&serde_json::json!({
                "settings": { "taskbarAlignment": "edge" }
            }))),
            "center"
        );
    }

    #[test]
    fn startup_window_theme_uses_persisted_or_system_preference() {
        assert_eq!(startup_window_theme(None, Some(true)), WindowTheme::Light);
        assert_eq!(startup_window_theme(None, Some(false)), WindowTheme::Dark);
        assert_eq!(startup_window_theme(None, None), WindowTheme::Dark);
        assert_eq!(
            startup_window_theme(
                Some(&serde_json::json!({ "settings": { "theme": "light" } })),
                Some(false),
            ),
            WindowTheme::Light
        );
        assert_eq!(
            startup_window_theme(
                Some(&serde_json::json!({ "settings": { "theme": "dark" } })),
                Some(true),
            ),
            WindowTheme::Dark
        );
        assert_eq!(
            startup_window_theme(
                Some(&serde_json::json!({ "settings": { "theme": "system" } })),
                Some(true),
            ),
            WindowTheme::Light
        );
    }

    #[test]
    fn runtime_window_theme_rejects_unknown_values() {
        assert_eq!(WindowTheme::from_request("light"), Ok(WindowTheme::Light));
        assert_eq!(WindowTheme::from_request("dark"), Ok(WindowTheme::Dark));
        assert!(WindowTheme::from_request("system").is_err());
        assert!(WindowTheme::from_request("sepia").is_err());
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
    fn anchor_placement_tracks_every_taskbar_edge() {
        let bottom_work = PhysicalRect {
            left: 0,
            top: 0,
            right: 1_920,
            bottom: 1_040,
        };
        assert_eq!(
            palette_position(
                bottom_work,
                TaskbarEdge::Bottom,
                taskbar_alignment::Alignment::Center,
                720,
                620,
            ),
            (600, 420)
        );

        let top_work = PhysicalRect {
            top: 40,
            ..bottom_work
        };
        assert_eq!(
            palette_position(
                top_work,
                TaskbarEdge::Top,
                taskbar_alignment::Alignment::Center,
                720,
                620,
            ),
            (600, 40)
        );

        assert_eq!(
            palette_position(
                PhysicalRect {
                    left: 48,
                    bottom: 1_080,
                    ..bottom_work
                },
                TaskbarEdge::Left,
                taskbar_alignment::Alignment::Center,
                720,
                620
            ),
            (48, 230)
        );
        assert_eq!(
            palette_position(
                PhysicalRect {
                    right: 1_872,
                    bottom: 1_080,
                    ..bottom_work
                },
                TaskbarEdge::Right,
                taskbar_alignment::Alignment::Center,
                720,
                620
            ),
            (1_152, 230)
        );
    }

    #[test]
    fn anchor_placement_supports_negative_coordinates_and_clamps() {
        let work = PhysicalRect {
            left: -1_920,
            top: -1_040,
            right: 0,
            bottom: 0,
        };
        assert_eq!(
            palette_position(
                work,
                TaskbarEdge::Bottom,
                taskbar_alignment::Alignment::Center,
                720,
                620,
            ),
            (-1_320, -620)
        );

        let small_work = PhysicalRect {
            left: 0,
            top: 0,
            right: 500,
            bottom: 420,
        };
        assert_eq!(
            palette_position(
                small_work,
                TaskbarEdge::Bottom,
                taskbar_alignment::Alignment::Center,
                480,
                400,
            ),
            (10, 20)
        );
    }

    #[test]
    fn taskbar_alignment_moves_horizontal_menu_with_the_icon_group() {
        let work = PhysicalRect {
            left: 0,
            top: 0,
            right: 1_920,
            bottom: 1_032,
        };
        assert_eq!(
            palette_position(
                work,
                TaskbarEdge::Bottom,
                taskbar_alignment::Alignment::Left,
                560,
                620,
            ),
            (0, 412)
        );
        assert_eq!(
            palette_position(
                work,
                TaskbarEdge::Bottom,
                taskbar_alignment::Alignment::Center,
                560,
                620,
            ),
            (680, 412)
        );
        assert_eq!(
            palette_position(
                work,
                TaskbarEdge::Bottom,
                taskbar_alignment::Alignment::Right,
                560,
                620,
            ),
            (1_360, 412)
        );
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
    fn clamped_window_size_scales_both_axes_with_monitor_dpi() {
        let work = PhysicalRect {
            left: 0,
            top: 0,
            right: 1_920,
            bottom: 1_040,
        };
        for (width, scale, expected_w, expected_h) in [
            (560, 1.0, 560, 620),
            (640, 1.0, 640, 620),
            (720, 1.0, 720, 620),
            (640, 1.25, 800, 775),
            (720, 1.5, 1_080, 930),
        ] {
            assert_eq!(
                calculate_clamped_window_size(width, DEFAULT_LOGICAL_HEIGHT, work, scale),
                (expected_w, expected_h),
                "width={width} scale={scale}"
            );
        }
        // 200% DPI on a 4K panel: both axes scale past the 1080p work area.
        let four_k = PhysicalRect {
            left: 0,
            top: 0,
            right: 3_840,
            bottom: 2_100,
        };
        assert_eq!(
            calculate_clamped_window_size(560, DEFAULT_LOGICAL_HEIGHT, four_k, 2.0),
            (1_120, 1_240)
        );
    }

    #[test]
    fn clamped_window_size_falls_back_for_invalid_scale() {
        let work = PhysicalRect {
            left: 0,
            top: 0,
            right: 1_920,
            bottom: 1_040,
        };
        for scale in [0.0, -1.5, f64::NAN, f64::INFINITY] {
            assert_eq!(
                calculate_clamped_window_size(640, DEFAULT_LOGICAL_HEIGHT, work, scale),
                (640, 620),
                "scale={scale}"
            );
        }
    }

    #[test]
    fn clamped_window_size_fits_small_and_portrait_work_areas() {
        // 150% DPI on a 1366x768 laptop screen: the palette clamps to the
        // work area instead of overflowing or shrinking forever.
        let short = PhysicalRect {
            left: 0,
            top: 0,
            right: 1_366,
            bottom: 720,
        };
        assert_eq!(
            calculate_clamped_window_size(640, DEFAULT_LOGICAL_HEIGHT, short, 1.5),
            (960, 720)
        );

        let portrait = PhysicalRect {
            left: 0,
            top: 0,
            right: 700,
            bottom: 1_100,
        };
        assert_eq!(
            calculate_clamped_window_size(720, DEFAULT_LOGICAL_HEIGHT, portrait, 1.0),
            (700, 620)
        );

        let tiny = PhysicalRect {
            left: 0,
            top: 0,
            right: 400,
            bottom: 350,
        };
        assert_eq!(
            calculate_clamped_window_size(640, DEFAULT_LOGICAL_HEIGHT, tiny, 1.0),
            (400, 350)
        );
    }

    #[test]
    fn preferred_size_is_restored_after_a_temporary_clamp() {
        // The function derives size only from the logical preference, so a
        // monitor that was too small does not leave the window shrunk once
        // it is presented on a larger work area again.
        let cramped = PhysicalRect {
            left: 0,
            top: 0,
            right: 500,
            bottom: 420,
        };
        let roomy = PhysicalRect {
            left: 0,
            top: 0,
            right: 2_560,
            bottom: 1_400,
        };
        assert_eq!(
            calculate_clamped_window_size(720, DEFAULT_LOGICAL_HEIGHT, cramped, 1.0),
            (500, 420)
        );
        assert_eq!(
            calculate_clamped_window_size(720, DEFAULT_LOGICAL_HEIGHT, roomy, 1.0),
            (720, 620)
        );
    }

    #[test]
    fn palette_position_clamps_when_window_exceeds_work_area() {
        let tiny_work = PhysicalRect {
            left: 100,
            top: 50,
            right: 400,
            bottom: 300,
        };
        assert_eq!(
            palette_position(
                tiny_work,
                TaskbarEdge::Bottom,
                taskbar_alignment::Alignment::Center,
                720,
                620,
            ),
            (100, 50)
        );
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
                "taskbarAlignment": "left",
                "theme": "system",
                "quickAccess": ["home", "desktop", "downloads", "documents", "pictures", "music"],
                "quickAccessCollapsed": false,
                "pinnedApps": ["app-one", "app-two"],
                "sectionOrder": ["apps", "recent", "quick", "pinned"]
            },
            "history": []
        });
        assert!(validate_state(&good).is_ok());

        for patch in [
            serde_json::json!({"settings": {"accent": "neon"}}),
            serde_json::json!({"settings": {"theme": "sepia"}}),
            serde_json::json!({"settings": {"shortcut": "X"}}),
            serde_json::json!({"settings": {"alwaysOnTop": "yes"}}),
            serde_json::json!({"settings": {"width": 999}}),
            serde_json::json!({"settings": {"viewZoom": 135}}),
            serde_json::json!({"settings": {"viewZoom": "large"}}),
            serde_json::json!({"settings": {"taskbarAlignment": "edge"}}),
            serde_json::json!({"settings": {"taskbarAlignment": 1}}),
            serde_json::json!({"settings": {"quickAccess": "home"}}),
            serde_json::json!({"settings": {"quickAccess": ["home", "home"]}}),
            serde_json::json!({"settings": {"quickAccess": ["network"]}}),
            serde_json::json!({"settings": {"quickAccess": ["home", "desktop", "downloads", "documents", "pictures", "music", "videos"]}}),
            serde_json::json!({"settings": {"quickAccessCollapsed": "yes"}}),
            serde_json::json!({"settings": {"sectionOrder": "apps"}}),
            serde_json::json!({"settings": {"sectionOrder": ["apps", "apps"]}}),
            serde_json::json!({"settings": {"sectionOrder": ["unknown"]}}),
            serde_json::json!({"settings": {"pinnedApps": "app-one"}}),
            serde_json::json!({"settings": {"pinnedApps": [""]}}),
            serde_json::json!({"settings": {"pinnedApps": ["app-one", "app-one"]}}),
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
            filter_existing_paths(vec![path_text.clone(), "relative.txt".to_string()]),
            vec![path_text.clone()]
        );
        std::fs::remove_file(&path).expect("remove test file");
        assert!(filter_existing_paths(vec![path_text]).is_empty());
    }

    #[test]
    fn raise_retries_stop_after_the_palette_took_foreground() {
        assert!(palette_raise_retry_wanted(true, true, false));
        assert!(!palette_raise_retry_wanted(true, true, true));
        assert!(!palette_raise_retry_wanted(false, true, false));
        assert!(!palette_raise_retry_wanted(true, false, false));
    }
}
