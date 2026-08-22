mod apps;
mod catalog;
mod files;
mod perf;
mod power;
mod start_menu;
mod taskbar;
mod taskbar_alignment;
mod taskbar_customization;
mod taskbar_icon_overlay;
mod theme;
mod win_key;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{Emitter, Manager, Theme, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use windows::Win32::Foundation::{HWND, POINT, RECT};
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
const PALETTE_STARTUP_DELAY: Duration = Duration::from_millis(100);
const STARTUP_SHELL_RETRY_DELAY: Duration = Duration::from_millis(250);
const STARTUP_SHELL_RETRY_ATTEMPTS: usize = 120;
const STARTUP_ALIGNMENT_DELAY: Duration = Duration::from_secs(1);

static PALETTE_OPEN: AtomicBool = AtomicBool::new(false);
static PALETTE_TRANSITION: AtomicU64 = AtomicU64::new(0);
static ACTIVATION_FOCUS_PENDING: AtomicBool = AtomicBool::new(false);
static PRESENTATION_ANCHOR: Mutex<Option<PresentationAnchor>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum PresentationSource {
    WinKey,
    TaskbarStartClick,
    ConfiguredShortcut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalPoint {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum TaskbarEdge {
    Bottom,
    Top,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PresentationAnchor {
    start_button: Option<PhysicalRect>,
    click_point: Option<PhysicalPoint>,
    taskbar_edge: Option<TaskbarEdge>,
    monitor: Option<PhysicalRect>,
    work_area: Option<PhysicalRect>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PresentationEvent {
    open: bool,
    source: PresentationSource,
    anchor: Option<PresentationAnchor>,
    generation: u64,
}

pub struct AppState {
    apps_cache: Mutex<Option<Vec<apps::AppEntry>>>,
    apps_scan_lock: tokio::sync::Mutex<()>,
    file_index: files::FileIndex,
    shortcut: Mutex<String>,
    shortcut_generation: AtomicU64,
}

/// Handles the internal crash-recovery mode before Tauri or the single-instance
/// plugin starts. Returns true when this process was launched as the watchdog.
pub fn run_start_restore_watchdog_if_requested() -> bool {
    start_menu::run_watchdog_from_args()
}

pub fn run() {
    let startup_timer = perf::start();
    let show_on_start = !launched_for_autostart();
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
        .manage(AppState {
            apps_cache: Mutex::new(None),
            apps_scan_lock: tokio::sync::Mutex::new(()),
            file_index: files::FileIndex::default(),
            shortcut: Mutex::new(String::new()),
            shortcut_generation: AtomicU64::new(0),
        })
        .setup(move |app| {
            let _ = start_menu::recover_stale(app.handle());
            // Repair the taskbar band if a previous instance crashed while
            // the palette was open.
            tauri::async_runtime::spawn_blocking(taskbar::recover);
            let persisted = load_state_value(app.handle()).ok().flatten();
            let alignment = startup_taskbar_alignment(persisted.as_ref());
            let _ = taskbar_alignment::initialize(&alignment);
            schedule_startup_taskbar_alignment();
            win_key::init(app.handle().clone());
            let customization_app = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                taskbar_customization::init(customization_app);
            });
            theme::watch(app.handle().clone());
            let app_data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            files::warm(
                app.state::<AppState>().file_index.clone(),
                app_data_dir,
                app.handle().clone(),
            );
            if let Some(window) = app.get_webview_window("main") {
                // Start from the persisted look instead of hardcoded defaults so
                // light-theme or blur-material users never see a wrong flash
                // before the frontend restyles the window.
                let theme =
                    startup_window_look(persisted.as_ref(), theme::apps_light() == Some(true));
                let _ = apply_window_look_with(|| clear_window_material(&window));
                schedule_hidden_memory_trim(app.handle().clone());
            }
            warm_apps(app.handle().clone());
            // Start installing the persisted shortcut during native setup. A
            // fresh, corrupt, or incomplete state defaults natively to Win,
            // so first-run activation never waits on frontend startup. The
            // Explorer bridge is initialized off the Tauri main thread.
            let combo = startup_shortcut(persisted.as_ref());
            apply_startup_shortcut(app.handle(), combo);
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
                ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
                return;
            }
            // Clicking away dismisses the launcher, like Raycast.
            if matches!(event, WindowEvent::Focused(false)) && window.is_visible().unwrap_or(false)
            {
                // Windows may report the palette as unfocused once before
                // granting foreground activation to the existing process.
                if ACTIVATION_FOCUS_PENDING.swap(false, Ordering::AcqRel) {
                    return;
                }
                PALETTE_OPEN.store(false, Ordering::Release);
                PALETTE_TRANSITION.fetch_add(1, Ordering::AcqRel);
                PRESENTATION_ANCHOR
                    .lock()
                    .map(|mut value| *value = None)
                    .ok();
                taskbar::release();
                let _ = window.hide();
                if let Some(webview) = window.app_handle().get_webview_window("main") {
                    set_webview_memory_target(&webview, true);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_apps,
            get_app_icons,
            refresh_apps,
            search_files,
            rebuild_file_index,
            get_file_thumbnail,
            get_file_thumbnails,
            get_quick_access,
            existing_paths,
            launch_app,
            launch_app_as_admin,
            open_path,
            run_path_as_admin,
            present_palette,
            hide_palette,
            set_window_style,
            set_window_width,
            set_taskbar_alignment,
            get_taskbar_settings,
            set_taskbar_thickness,
            set_taskbar_auto_hide,
            set_taskbar_combine_buttons,
            set_taskbar_task_view,
            set_taskbar_widgets,
            set_taskbar_searchbox_mode,
            set_taskbar_start_icon,
            set_custom_start_icon,
            select_custom_start_icon,
            remove_custom_start_icon,
            get_system_theme,
            set_shortcut,
            load_state,
            save_state,
            perform_power_action,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Prism");
}

fn launched_for_autostart() -> bool {
    std::env::args_os()
        .skip(1)
        .any(|argument| argument == std::ffi::OsStr::new("--autostart"))
}

fn schedule_startup_taskbar_alignment() {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_ALIGNMENT_DELAY).await;
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let _ = taskbar_alignment::reapply_current();
        })
        .await;
    });
}

fn warm_apps(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _scan_guard = state.apps_scan_lock.lock().await;
        if state
            .apps_cache
            .lock()
            .ok()
            .and_then(|cache| cache.as_ref().map(|_| ()))
            .is_some()
        {
            return;
        }
        let cache_path = apps_cache_path(&app);
        let result = tauri::async_runtime::spawn_blocking(move || apps::scan(&cache_path)).await;
        let Ok(Ok(list)) = result else {
            return;
        };
        let cache_result = state.apps_cache.lock();
        if let Ok(mut cache) = cache_result {
            *cache = Some(list);
        }
    });
}

fn schedule_initial_palette(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(PALETTE_STARTUP_DELAY).await;
        let main_thread_app = app.clone();
        let _ = app.run_on_main_thread(move || activate_palette(&main_thread_app));
    });
}

fn activate_palette(app: &tauri::AppHandle) {
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
    let Some((x, y)) = palette_target(window, anchor, taskbar_alignment::current()) else {
        return;
    };
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

fn reconcile_palette_position(
    window: &tauri::WebviewWindow,
    anchor: Option<PresentationAnchor>,
) -> Result<(), String> {
    let alignment = taskbar_alignment::current();
    let (x, y) = palette_target(window, anchor, alignment)
        .ok_or_else(|| "cannot resolve Prism alignment target".to_string())?;
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
) -> Option<(i32, i32)> {
    let Ok(hwnd) = window.hwnd() else {
        return None;
    };
    let Ok(size) = window.outer_size() else {
        return None;
    };
    let info = anchor
        .and_then(|value| value.monitor.zip(value.work_area))
        .or_else(|| monitor_geometry_for_window(HWND(hwnd.0)));
    let (monitor, work) = info?;
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
    let edge = anchor
        .and_then(|value| value.taskbar_edge)
        .or_else(|| taskbar_edge(monitor, work))
        .unwrap_or(TaskbarEdge::Bottom);
    Some(palette_position(work, edge, alignment, width, height))
}

fn presentation_anchor(
    start_button: Option<RECT>,
    click_point: Option<POINT>,
) -> PresentationAnchor {
    let start_button = start_button.map(PhysicalRect::from);
    let click_point = click_point.map(PhysicalPoint::from);
    let monitor_point = click_point.or_else(|| start_button.map(PhysicalRect::center));
    let geometry = monitor_point.and_then(monitor_geometry_for_point);
    PresentationAnchor {
        start_button,
        click_point,
        taskbar_edge: geometry.and_then(|(monitor, work)| taskbar_edge(monitor, work)),
        monitor: geometry.map(|(monitor, _)| monitor),
        work_area: geometry.map(|(_, work)| work),
    }
}

fn monitor_geometry_for_point(point: PhysicalPoint) -> Option<(PhysicalRect, PhysicalRect)> {
    let monitor = unsafe {
        MonitorFromPoint(
            POINT {
                x: point.x,
                y: point.y,
            },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    monitor_geometry(monitor)
}

fn monitor_geometry_for_window(window: HWND) -> Option<(PhysicalRect, PhysicalRect)> {
    let mut point = POINT::default();
    let monitor = unsafe {
        if GetCursorPos(&mut point).is_ok() {
            MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST)
        }
    };
    monitor_geometry(monitor)
}

fn monitor_geometry(
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
) -> Option<(PhysicalRect, PhysicalRect)> {
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    unsafe { GetMonitorInfoW(monitor, &mut info).as_bool() }.then(|| {
        (
            PhysicalRect::from(info.rcMonitor),
            PhysicalRect::from(info.rcWork),
        )
    })
}

fn taskbar_edge(monitor: PhysicalRect, work: PhysicalRect) -> Option<TaskbarEdge> {
    let candidates = [
        (work.top - monitor.top, TaskbarEdge::Top),
        (monitor.bottom - work.bottom, TaskbarEdge::Bottom),
        (work.left - monitor.left, TaskbarEdge::Left),
        (monitor.right - work.right, TaskbarEdge::Right),
    ];
    candidates
        .into_iter()
        .filter(|(inset, _)| *inset > 0)
        .max_by_key(|(inset, _)| *inset)
        .map(|(_, edge)| edge)
}

fn palette_position(
    work: PhysicalRect,
    edge: TaskbarEdge,
    alignment: taskbar_alignment::Alignment,
    width: i32,
    height: i32,
) -> (i32, i32) {
    let aligned_x = match alignment {
        taskbar_alignment::Alignment::Left => work.left,
        taskbar_alignment::Alignment::Center => work.left + (work.width() - width) / 2,
        taskbar_alignment::Alignment::Right => work.right - width,
    };
    let aligned_y = match alignment {
        taskbar_alignment::Alignment::Left => work.top,
        taskbar_alignment::Alignment::Center => work.top + (work.height() - height) / 2,
        taskbar_alignment::Alignment::Right => work.bottom - height,
    };
    let (x, y) = match edge {
        TaskbarEdge::Bottom => (aligned_x, work.bottom - height),
        TaskbarEdge::Top => (aligned_x, work.top),
        TaskbarEdge::Left => (work.left, aligned_y),
        TaskbarEdge::Right => (work.right - width, aligned_y),
    };
    (
        x.clamp(work.left, work.right - width),
        y.clamp(work.top, work.bottom - height),
    )
}

impl PhysicalRect {
    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }

    fn center(self) -> PhysicalPoint {
        PhysicalPoint {
            x: self.left + self.width() / 2,
            y: self.top + self.height() / 2,
        }
    }
}

impl From<RECT> for PhysicalRect {
    fn from(rect: RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

impl From<POINT> for PhysicalPoint {
    fn from(point: POINT) -> Self {
        Self {
            x: point.x,
            y: point.y,
        }
    }
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
    if !opening {
        ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
    }
    if opening {
        set_webview_memory_target(&window, false);
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
                        set_webview_memory_target(&window, true);
                        taskbar::release();
                    }
                }
            });
        });
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
    raise_palette(&window)?;
    schedule_palette_raise_retry(&app, PALETTE_TRANSITION.load(Ordering::Acquire));
    perf::finish(timer, "palette_present", || "window=main".to_string());
    Ok(true)
}

#[tauri::command]
fn hide_palette(app: tauri::AppHandle) -> Result<(), String> {
    PALETTE_OPEN.store(false, Ordering::Release);
    ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
    PALETTE_TRANSITION.fetch_add(1, Ordering::AcqRel);
    PRESENTATION_ANCHOR
        .lock()
        .map(|mut value| *value = None)
        .ok();
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

/// Prism renders one way: a solid CSS-painted surface over a transparent
/// window. Native backdrop materials (acrylic/mica/blur) paint the full
/// square HWND and fight that design at every corner, so they are always
/// cleared - including whatever an older build or a hand-edited state file
/// left active on the window.
fn apply_window_look_with(mut clear: impl FnMut() -> Result<(), String>) -> Result<(), String> {
    clear()
}

fn clear_window_material(window: &tauri::WebviewWindow) -> Result<(), String> {
    // `EffectsBuilder::clear_effects()` only empties a new config. Tauri
    // interprets `None` as the instruction to remove the active native
    // material from the HWND.
    window.set_effects(None).map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_apps(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<apps::AppEntry>, String> {
    let timer = perf::start();
    // Fast path: already scanned this session.
    if let Some(list) = state.apps_cache.lock().map_err(|e| e.to_string())?.clone() {
        perf::finish(timer, "get_apps_cache", || format!("count={}", list.len()));
        return Ok(strip_icons(list));
    }
    // React StrictMode and fast repeated opens can overlap the initial IPC.
    // Only one scan should touch the filesystem and icon cache; waiters recheck
    // the populated in-memory cache after the first scan completes.
    let _scan_guard = state.apps_scan_lock.lock().await;
    if let Some(list) = state.apps_cache.lock().map_err(|e| e.to_string())?.clone() {
        perf::finish(timer, "get_apps_cache_after_wait", || {
            format!("count={}", list.len())
        });
        return Ok(strip_icons(list));
    }
    // Off the main thread: scanning walks the shell and extracts icons.
    let cache_path = apps_cache_path(&app);
    let list = tauri::async_runtime::spawn_blocking(move || apps::scan(&cache_path))
        .await
        .map_err(|e| format!("app scan task failed: {e}"))??;
    *state.apps_cache.lock().map_err(|e| e.to_string())? = Some(list.clone());
    perf::finish(timer, "get_apps_scan", || format!("count={}", list.len()));
    Ok(strip_icons(list))
}

/// The full icon payload is several MB of base64. Results only need icons for
/// the rows that are actually rendered, so the metadata list stays lean and
/// `get_app_icons` delivers the pixels lazily in one batched IPC call.
fn strip_icons(mut list: Vec<apps::AppEntry>) -> Vec<apps::AppEntry> {
    for entry in &mut list {
        entry.icon = None;
    }
    list
}

#[tauri::command]
fn get_app_icons(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let mut icons = std::collections::HashMap::new();
    let apps = state.apps_cache.lock().map_err(|error| error.to_string())?;
    let apps = apps
        .as_ref()
        .ok_or_else(|| "application index is not ready".to_string())?;
    for id in ids.into_iter().take(512) {
        let Some(entry) = apps.iter().find(|entry| entry.app_id == id) else {
            continue;
        };
        if let Some(icon) = entry.icon.as_deref() {
            icons.insert(id, icon.to_string());
        }
    }
    Ok(icons)
}

#[tauri::command]
async fn refresh_apps(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<apps::AppEntry>, String> {
    let _scan_guard = state.apps_scan_lock.lock().await;
    let cache_path = apps_cache_path(&app);
    let list = tauri::async_runtime::spawn_blocking(move || apps::scan_force(&cache_path))
        .await
        .map_err(|e| format!("app scan task failed: {e}"))??;
    *state.apps_cache.lock().map_err(|e| e.to_string())? = Some(list.clone());
    Ok(strip_icons(list))
}

fn apps_cache_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("apps.json")
}

#[tauri::command]
async fn launch_app(id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let timer = perf::start();
    // Only launch apps that came from our own scan. The entry is cloned so
    // the cache lock is released before ShellExecuteW, which can block.
    let entry = {
        let apps = state.apps_cache.lock().map_err(|e| e.to_string())?;
        apps.as_ref()
            .and_then(|list| list.iter().find(|a| a.app_id == id))
            .cloned()
            .ok_or_else(|| "unknown app id".to_string())?
    };
    let detail = format!("name={};source={}", entry.name, entry.source);
    let result = tauri::async_runtime::spawn_blocking(move || apps::launch(&entry))
        .await
        .map_err(|e| format!("launch task failed: {e}"))?;
    perf::finish(timer, "launch_app", move || detail.clone());
    result
}

#[tauri::command]
async fn launch_app_as_admin(id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let entry = {
        let apps = state.apps_cache.lock().map_err(|e| e.to_string())?;
        apps.as_ref()
            .and_then(|list| list.iter().find(|app| app.app_id == id))
            .cloned()
            .ok_or_else(|| "unknown app id".to_string())?
    };
    tauri::async_runtime::spawn_blocking(move || apps::launch_elevated(&entry))
        .await
        .map_err(|e| format!("elevated launch task failed: {e}"))?
}

#[tauri::command]
async fn open_path(path: String) -> Result<(), String> {
    let timer = perf::start();
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.exists() {
        return Err("path must be an existing absolute file or folder".to_string());
    }
    let kind = if path.is_dir() { "directory" } else { "file" };
    let result = tauri::async_runtime::spawn_blocking(move || apps::open_path(&path))
        .await
        .map_err(|e| format!("open path task failed: {e}"))?;
    perf::finish(timer, "open_path", move || format!("kind={kind}"));
    result
}

#[tauri::command]
async fn run_path_as_admin(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.is_file() {
        return Err("path must be an existing absolute file".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || apps::launch_path_elevated(&path, None, None))
        .await
        .map_err(|e| format!("run as administrator task failed: {e}"))?
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
async fn rebuild_file_index(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.file_index.rebuild();
    Ok(())
}

#[tauri::command]
async fn get_file_thumbnail(path: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || files::file_thumbnail(&path))
        .await
        .ok()
        .flatten()
}

#[tauri::command]
async fn get_file_thumbnails(paths: Vec<String>) -> Vec<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || files::file_thumbnails(paths))
        .await
        .unwrap_or_default()
}

#[tauri::command]
async fn get_quick_access() -> Vec<files::QuickAccessEntry> {
    // Known-folder resolution and is_dir checks can block on redirected or
    // network locations; keep the startup path off the main thread.
    tauri::async_runtime::spawn_blocking(files::quick_access)
        .await
        .unwrap_or_default()
}

fn filter_existing_paths(paths: Vec<String>) -> Vec<String> {
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
async fn existing_paths(paths: Vec<String>) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || filter_existing_paths(paths))
        .await
        .unwrap_or_default()
}

#[tauri::command]
async fn perform_power_action(action: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || power::perform(&action))
        .await
        .map_err(|error| format!("power action task failed: {error}"))?
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
    let anchor = PRESENTATION_ANCHOR.lock().ok().and_then(|value| *value);
    position_palette(&window, anchor);
    Ok(())
}

#[tauri::command]
fn set_taskbar_alignment(app: tauri::AppHandle, alignment: String) -> Result<(), String> {
    let alignment = taskbar_alignment::Alignment::parse(&alignment)?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    let anchor = PRESENTATION_ANCHOR.lock().ok().and_then(|value| *value);
    let (x, y) = palette_target(&window, anchor, alignment)
        .ok_or_else(|| "cannot resolve Prism alignment target".to_string())?;
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
async fn get_taskbar_settings(
    app: tauri::AppHandle,
) -> Result<taskbar_customization::TaskbarSettings, String> {
    tauri::async_runtime::spawn_blocking(move || taskbar_customization::settings(&app))
        .await
        .map_err(|error| format!("taskbar settings task failed: {error}"))?
}

#[tauri::command]
fn set_taskbar_thickness(value: String) -> Result<(), String> {
    taskbar_customization::set_thickness(&value)
}

#[tauri::command]
fn set_taskbar_auto_hide(enabled: bool) -> Result<(), String> {
    taskbar_customization::set_auto_hide(enabled)
}

#[tauri::command]
fn set_taskbar_combine_buttons(value: String) -> Result<(), String> {
    taskbar_customization::set_combine_buttons(&value)
}

#[tauri::command]
fn set_taskbar_task_view(visible: bool) -> Result<(), String> {
    taskbar_customization::set_task_view(visible)
}

#[tauri::command]
fn set_taskbar_widgets(visible: bool) -> Result<(), String> {
    taskbar_customization::set_widgets(visible)
}

#[tauri::command]
fn set_taskbar_searchbox_mode(value: String) -> Result<(), String> {
    taskbar_customization::set_searchbox_mode(&value)
}

#[tauri::command]
async fn set_custom_start_icon(app: tauri::AppHandle, base64_png: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        taskbar_customization::set_custom_start_icon(&app, &base64_png)
    })
    .await
    .map_err(|error| format!("taskbar icon task failed: {error}"))?
}

#[tauri::command]
async fn select_custom_start_icon(app: tauri::AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        taskbar_customization::select_custom_start_icon(&app, &id)
    })
    .await
    .map_err(|error| format!("taskbar icon selection task failed: {error}"))?
}

#[tauri::command]
async fn remove_custom_start_icon(app: tauri::AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        taskbar_customization::remove_custom_start_icon(&app, &id)
    })
    .await
    .map_err(|error| format!("taskbar icon removal task failed: {error}"))?
}

#[tauri::command]
async fn set_taskbar_start_icon(app: tauri::AppHandle, value: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        taskbar_customization::set_start_icon(&app, &value)
    })
    .await
    .map_err(|error| format!("taskbar icon mode task failed: {error}"))?
}

fn is_animatable_window_width(width: u32) -> bool {
    (560..=720).contains(&width)
}

/// Applies the window style in one IPC round-trip: native theme plus the
/// solid-only surface rule.
#[tauri::command]
fn set_window_style(app: tauri::AppHandle, theme: String) -> Result<(), String> {
    if !matches!(theme.as_str(), "light" | "dark") {
        return Err(format!("unknown theme '{theme}'"));
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    let _ = window.set_theme(Some(if theme == "light" {
        Theme::Light
    } else {
        Theme::Dark
    }));
    apply_window_look_with(|| clear_window_material(&window))
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

/// Resolves the (theme, effect) pair the window starts with, before the
/// frontend loads. Persisted values win when valid; "system" resolves
/// against the OS apps-light preference; anything else falls back to dark
/// solid so startup never depends on frontend timing.
fn startup_window_look(
    state: Option<&serde_json::Value>,
    system_prefers_light: bool,
) -> &'static str {
    let raw = state
        .and_then(|value| value.get("settings"))
        .and_then(|value| value.as_object())
        .and_then(|settings| settings.get("theme"))
        .and_then(|value| value.as_str());
    match raw {
        Some("light") => "light",
        Some("dark") => "dark",
        Some("system") if system_prefers_light => "light",
        _ => "dark",
    }
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

fn startup_taskbar_alignment(state: Option<&serde_json::Value>) -> String {
    state
        .and_then(|value| value.get("settings"))
        .and_then(|settings| settings.get("taskbarAlignment"))
        .and_then(|alignment| alignment.as_str())
        .filter(|alignment| matches!(*alignment, "left" | "center" | "right"))
        .unwrap_or("center")
        .to_string()
}

fn apply_startup_shortcut(app: &tauri::AppHandle, combo: String) {
    let generation = app
        .state::<AppState>()
        .shortcut_generation
        .load(Ordering::Acquire);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if app
            .state::<AppState>()
            .shortcut_generation
            .load(Ordering::Acquire)
            != generation
        {
            return;
        }
        let first_attempt = {
            let attempt_app = app.clone();
            let attempt_combo = combo.clone();
            tauri::async_runtime::spawn_blocking(move || {
                apply_shortcut_if_current(&attempt_app, &attempt_combo, generation)
            })
            .await
        };
        if matches!(first_attempt, Ok(Ok(()))) {
            return;
        }

        for _ in 0..STARTUP_SHELL_RETRY_ATTEMPTS {
            tokio::time::sleep(STARTUP_SHELL_RETRY_DELAY).await;
            if app
                .state::<AppState>()
                .shortcut_generation
                .load(Ordering::Acquire)
                != generation
            {
                return;
            }
            let retry_app = app.clone();
            let retry_combo = combo.clone();
            let result = tauri::async_runtime::spawn_blocking(move || {
                apply_shortcut_if_current(&retry_app, &retry_combo, generation)
            })
            .await;
            if matches!(result, Ok(Ok(()))) {
                return;
            }
        }
        let _ = app.emit(
            win_key::FAILED_EVENT,
            "Explorer taskbar integration was unavailable during startup",
        );
    });
}

/// Applies a global shortcut. The new binding is activated and proven
/// BEFORE the old one is released; on any failure the previous shortcut
/// stays active and an error is returned. State is only updated on success.
fn apply_shortcut_if_current(
    app: &tauri::AppHandle,
    combo: &str,
    generation: u64,
) -> Result<(), String> {
    apply_shortcut_with_generation(app, combo, Some(generation))
}

fn apply_shortcut_with_generation(
    app: &tauri::AppHandle,
    combo: &str,
    expected_generation: Option<u64>,
) -> Result<(), String> {
    validate_shortcut(combo)?;
    let state = app.state::<AppState>();
    let mut prev = state.shortcut.lock().map_err(|e| e.to_string())?;
    if expected_generation
        .is_some_and(|generation| state.shortcut_generation.load(Ordering::Acquire) != generation)
    {
        return Err("shortcut request was superseded".to_string());
    }
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
async fn set_shortcut(app: tauri::AppHandle, combo: String) -> Result<(), String> {
    validate_shortcut(&combo)?;
    let generation = app
        .state::<AppState>()
        .shortcut_generation
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    tauri::async_runtime::spawn_blocking(move || {
        apply_shortcut_if_current(&app, &combo, generation)
    })
    .await
    .map_err(|error| format!("shortcut task failed: {error}"))?
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
    if let Some(always_on_top) = settings.get("alwaysOnTop") {
        if !always_on_top.is_boolean() {
            return Err("state.settings.alwaysOnTop must be a boolean".to_string());
        }
    }
    if let Some(alignment) = settings.get("taskbarAlignment") {
        let alignment = alignment
            .as_str()
            .ok_or("state.settings.taskbarAlignment must be a string")?;
        if !matches!(alignment, "left" | "center" | "right") {
            return Err(format!("unknown taskbar alignment '{alignment}'"));
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
    if let Some(quick_access) = settings.get("quickAccess") {
        let entries = quick_access
            .as_array()
            .ok_or("state.settings.quickAccess must be an array")?;
        if entries.len() > 6 {
            return Err("state.settings.quickAccess has too many entries".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        for entry in entries {
            let kind = entry
                .as_str()
                .ok_or("quick access entries must be strings")?;
            if !matches!(
                kind,
                "home" | "desktop" | "downloads" | "documents" | "pictures" | "music" | "videos"
            ) {
                return Err(format!("unknown quick access entry '{kind}'"));
            }
            if !seen.insert(kind) {
                return Err(format!("duplicate quick access entry '{kind}'"));
            }
        }
    }
    if let Some(collapsed) = settings.get("quickAccessCollapsed") {
        if !collapsed.is_boolean() {
            return Err("state.settings.quickAccessCollapsed must be a boolean".to_string());
        }
    }
    if let Some(section_order) = settings.get("sectionOrder") {
        let entries = section_order
            .as_array()
            .ok_or("state.settings.sectionOrder must be an array")?;
        if entries.len() > 4 {
            return Err("state.settings.sectionOrder has too many entries".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        for entry in entries {
            let id = entry
                .as_str()
                .ok_or("section order entries must be strings")?;
            if !matches!(id, "pinned" | "recent" | "quick" | "apps") {
                return Err(format!("unknown section order entry '{id}'"));
            }
            if !seen.insert(id) {
                return Err(format!("duplicate section order entry '{id}'"));
            }
        }
    }
    if let Some(pinned_apps) = settings.get("pinnedApps") {
        let entries = pinned_apps
            .as_array()
            .ok_or("state.settings.pinnedApps must be an array")?;
        if entries.len() > 64 {
            return Err("state.settings.pinnedApps has too many entries".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        for entry in entries {
            let app_id = entry
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= 4096)
                .ok_or("pinned app ids must be non-empty strings")?;
            if !seen.insert(app_id) {
                return Err("state.settings.pinnedApps contains duplicates".to_string());
            }
        }
    }
    if let Some(app_groups) = settings.get("appGroups") {
        let groups = app_groups
            .as_array()
            .ok_or("state.settings.appGroups must be an array")?;
        if groups.len() > 16 {
            return Err("state.settings.appGroups has too many entries".to_string());
        }
        let mut group_ids = std::collections::HashSet::new();
        for group in groups {
            let group = group
                .as_object()
                .ok_or("app group entries must be objects")?;
            let id = group
                .get("id")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty() && value.len() <= 96)
                .ok_or("app group ids must be non-empty strings")?;
            let name = group
                .get("name")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty() && value.len() <= 64)
                .ok_or("app group names must be non-empty strings")?;
            if !group_ids.insert(id) {
                return Err("state.settings.appGroups contains duplicate ids".to_string());
            }
            if !group
                .get("collapsed")
                .is_some_and(serde_json::Value::is_boolean)
            {
                return Err("app group collapsed must be a boolean".to_string());
            }
            let app_ids = group
                .get("appIds")
                .and_then(|value| value.as_array())
                .ok_or("app group appIds must be an array")?;
            if app_ids.len() > 64 {
                return Err(format!("app group '{name}' has too many apps"));
            }
            let mut seen = std::collections::HashSet::new();
            for app_id in app_ids {
                let app_id = app_id
                    .as_str()
                    .filter(|value| !value.is_empty() && value.len() <= 4096)
                    .ok_or("app group app ids must be non-empty strings")?;
                if !seen.insert(app_id) {
                    return Err(format!("app group '{name}' contains duplicate apps"));
                }
            }
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
async fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    // Ensure interception is fully torn down before exiting. Tearing down the
    // Win-key bridge can take up to two seconds (pump stop handshake); it runs
    // off the main thread so quitting never freezes the UI.
    let teardown_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        taskbar::release();
        let _ = win_key::set_enabled(false);
        let _ = start_menu::restore(&teardown_app);
        win_key::set_provider_suppression(false);
    })
    .await
    .map_err(|error| format!("quit teardown task failed: {error}"))?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_materials_are_always_cleared_for_solid_rendering() {
        // Prism renders solid only: whatever a legacy setting or an older
        // build left on the HWND gets cleared before every style pass.
        let cleared = std::cell::Cell::new(false);
        apply_window_look_with(|| {
            cleared.set(true);
            Ok(())
        })
        .unwrap();
        assert!(cleared.get());

        // A failed clear fails the call so the frontend knows styling broke.
        assert!(apply_window_look_with(|| Err("clear failed".to_string())).is_err());
    }

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
    fn startup_window_look_follows_persisted_theme() {
        // No persisted state: dark default.
        assert_eq!(startup_window_look(None, false), "dark");
        // Persisted value wins when valid.
        assert_eq!(
            startup_window_look(
                Some(&serde_json::json!({ "settings": { "theme": "light" } })),
                false
            ),
            "light"
        );
        // "system" resolves against the OS apps-light preference.
        assert_eq!(
            startup_window_look(
                Some(&serde_json::json!({ "settings": { "theme": "system" } })),
                true
            ),
            "light"
        );
        assert_eq!(
            startup_window_look(
                Some(&serde_json::json!({ "settings": { "theme": "system" } })),
                false
            ),
            "dark"
        );
        // Invalid or missing values fall back instead of crashing startup.
        assert_eq!(
            startup_window_look(
                Some(&serde_json::json!({ "settings": { "theme": "hologram" } })),
                false
            ),
            "dark"
        );
        assert_eq!(
            startup_window_look(Some(&serde_json::json!({ "version": 3 })), false),
            "dark"
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
            serde_json::json!({"settings": {"effect": "hologram"}}),
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
}
