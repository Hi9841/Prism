//! Presents the Windows taskbar alongside Prism over fullscreen windows.
//!
//! A bottom z-order lease is sticky: Alt-Tab does not undo `HWND_BOTTOM`.
//! The lease stays only while a real fullscreen app is the one on screen,
//! and ends as soon as the foreground app is windowed or the desktop.

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_ACTIVATE, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetClassNameW, GetForegroundWindow, GetTopWindow, GetWindow, GetWindowLongW,
    GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, GW_HWNDLAST, GW_HWNDNEXT, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WS_EX_APPWINDOW,
    WS_EX_TOOLWINDOW,
};

/// Persistent marker proving Prism still owns the taskbar's z-order. Present
/// while the palette holds it topmost, and while it is parked behind a
/// fullscreen app after the palette closes. Cleared once the taskbar is back
/// in the normal band, so a crash cannot leave it buried.
const TOPMOST_MARKER: &str = "taskbar-topmost";

/// Palette is showing and intentionally holds the taskbar in the topmost band.
static PRESENTED: AtomicBool = AtomicBool::new(false);

/// Taskbar was sent to `HWND_BOTTOM` for a fullscreen app. Stays set until a
/// windowed app or the desktop is actually showing.
static BURIED: AtomicBool = AtomicBool::new(false);

static WATCH_RUNNING: AtomicBool = AtomicBool::new(false);

/// Fullscreen windows must not be covered by the taskbar; a few pixels of
/// slack avoid classifying maximized windows as fullscreen.
const FULLSCREEN_TOLERANCE: i32 = 4;

/// Shell, desktop, and Alt-Tab surfaces. They can cover the monitor during
/// the gesture, but they are not the app the user is switching to.
const SKIP_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "MultitaskingViewFrame",
    "XamlExplorerHostIslandWindow",
    "ForegroundStaging",
    "TaskSwitcherWnd",
    "IME",
    "MSCTFIME UI",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StackEntry {
    /// Prism, the Alt-Tab switcher, the desktop, or a minimized window.
    Skip,
    Fullscreen,
    Normal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskbarBand {
    /// Behind a fullscreen app, so the game is not letterboxed by the bar.
    Bottom,
    /// Normal band: visible over windowed apps and the desktop.
    Normal,
}

/// `foreground` is the current foreground classification when it is known.
/// Skip/None means Prism, nothing, or the Alt-Tab switcher still owns the
/// sample, which is not evidence of a fullscreen app. `underneath` is the
/// first real window below that sample. A windowed foreground wins even if
/// a fullscreen game is still open below it.
fn band_for(foreground: Option<StackEntry>, underneath: Option<StackEntry>) -> TaskbarBand {
    match foreground {
        Some(StackEntry::Fullscreen) => TaskbarBand::Bottom,
        Some(StackEntry::Normal) => TaskbarBand::Normal,
        Some(StackEntry::Skip) | None => match underneath {
            Some(StackEntry::Fullscreen) => TaskbarBand::Bottom,
            Some(StackEntry::Normal) | Some(StackEntry::Skip) | None => TaskbarBand::Normal,
        },
    }
}

pub fn present() {
    let Some(taskbar) = taskbar_window() else {
        return;
    };
    if !foreground_is_fullscreen() {
        // Opened over a windowed app, often by Alt-Tab away from a game.
        // Drop a bottom lease now; leaving it hides the taskbar behind that app.
        if BURIED.load(Ordering::Acquire) && !PRESENTED.load(Ordering::Acquire) {
            restore_normal(taskbar);
        }
        return;
    }
    // Presentation owns the topmost band. The bury watcher must not fight it.
    BURIED.store(false, Ordering::Release);
    PRESENTED.store(true, Ordering::Release);
    unsafe {
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar,
            lParam: LPARAM(1),
            ..Default::default()
        };
        let _ = SHAppBarMessage(ABM_ACTIVATE, &mut appbar);
        let _ = ShowWindow(taskbar, SW_SHOWNOACTIVATE);
        place(taskbar, HWND_TOPMOST, true);
        write_marker(true);
    }
}

pub fn release() {
    let Some(taskbar) = taskbar_window() else {
        PRESENTED.store(false, Ordering::Release);
        clear_bury();
        return;
    };
    deactivate_appbar(taskbar);
    let owned = PRESENTED.swap(false, Ordering::AcqRel)
        || marker_present()
        || BURIED.load(Ordering::Acquire);
    if !owned {
        return;
    }
    apply_showing_band(taskbar);
}

/// Quit path. A buried lease dies with this process, so put the taskbar back
/// before exit. A quit that never took the lease must not touch z-order:
/// `HWND_NOTOPMOST` clears the shell tray's `WS_EX_TOPMOST`.
pub fn release_on_exit() {
    let presented = PRESENTED.swap(false, Ordering::AcqRel);
    let buried = BURIED.swap(false, Ordering::AcqRel);
    let marked = marker_present();
    if !presented && !buried && !marked {
        return;
    }
    if let Some(taskbar) = taskbar_window() {
        deactivate_appbar(taskbar);
        place(taskbar, HWND_TOPMOST, false);
    }
    write_marker(false);
}

/// Startup repair. Also heals a taskbar a previous build parked at
/// `HWND_BOTTOM` and then forgot (the marker was cleared on release).
pub fn recover() {
    let Some(taskbar) = taskbar_window() else {
        clear_bury();
        return;
    };
    if !marker_present() && !taskbar_is_bottom(taskbar) {
        return;
    }
    PRESENTED.store(false, Ordering::Release);
    apply_showing_band(taskbar);
}

fn apply_showing_band(taskbar: HWND) {
    match showing_band() {
        TaskbarBand::Bottom => bury(taskbar),
        TaskbarBand::Normal => restore_normal(taskbar),
    }
}

fn bury(taskbar: HWND) {
    place(taskbar, HWND_BOTTOM, false);
    BURIED.store(true, Ordering::Release);
    write_marker(true);
    start_bury_watch();
}

fn restore_normal(taskbar: HWND) {
    place(taskbar, HWND_NOTOPMOST, false);
    // Lost the race with a new presentation: that path owns topmost.
    if PRESENTED.load(Ordering::Acquire) {
        place(taskbar, HWND_TOPMOST, true);
        return;
    }
    BURIED.store(false, Ordering::Release);
    write_marker(false);
}

fn clear_bury() {
    BURIED.store(false, Ordering::Release);
    write_marker(false);
}

fn start_bury_watch() {
    if WATCH_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("prism-taskbar".into())
        .spawn(|| {
            watch_buried_taskbar();
            WATCH_RUNNING.store(false, Ordering::Release);
            if BURIED.load(Ordering::Acquire) {
                start_bury_watch();
            }
        });
    if spawned.is_err() {
        WATCH_RUNNING.store(false, Ordering::Release);
    }
}

fn watch_buried_taskbar() {
    while BURIED.load(Ordering::Acquire) {
        reconcile_bury();
        if !BURIED.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

fn reconcile_bury() {
    if PRESENTED.load(Ordering::Acquire) || !BURIED.load(Ordering::Acquire) {
        return;
    }
    if showing_band() != TaskbarBand::Normal {
        return;
    }
    let Some(taskbar) = taskbar_window() else {
        clear_bury();
        return;
    };
    if PRESENTED.load(Ordering::Acquire) || !BURIED.load(Ordering::Acquire) {
        return;
    }
    restore_normal(taskbar);
}

fn showing_band() -> TaskbarBand {
    let foreground = unsafe { GetForegroundWindow() };
    let self_pid = unsafe { GetCurrentProcessId() };
    let foreground_entry = if foreground.0.is_null() {
        None
    } else {
        Some(classify_window(foreground, self_pid))
    };
    if matches!(
        foreground_entry,
        Some(StackEntry::Fullscreen | StackEntry::Normal)
    ) {
        return band_for(foreground_entry, None);
    }
    band_for(foreground_entry, first_real_window(self_pid))
}

fn first_real_window(self_pid: u32) -> Option<StackEntry> {
    let Ok(mut hwnd) = (unsafe { GetTopWindow(None) }) else {
        return None;
    };
    for _ in 0..128 {
        if hwnd.0.is_null() {
            break;
        }
        let entry = classify_window(hwnd, self_pid);
        if entry != StackEntry::Skip {
            return Some(entry);
        }
        match unsafe { GetWindow(hwnd, GW_HWNDNEXT) } {
            Ok(next) if !next.0.is_null() && next != hwnd => hwnd = next,
            _ => break,
        }
    }
    None
}

fn classify_window(hwnd: HWND, self_pid: u32) -> StackEntry {
    unsafe {
        if hwnd.0.is_null()
            || !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
            || cloaked(hwnd)
        {
            return StackEntry::Skip;
        }
        if skipped_class(hwnd) || tool_window(hwnd) {
            return StackEntry::Skip;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == self_pid {
            return StackEntry::Skip;
        }
        if window_covers_monitor(hwnd) {
            return StackEntry::Fullscreen;
        }
        if !has_area(hwnd) {
            return StackEntry::Skip;
        }
        StackEntry::Normal
    }
}

fn foreground_is_fullscreen() -> bool {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return false;
    }
    classify_window(foreground, unsafe { GetCurrentProcessId() }) == StackEntry::Fullscreen
}

fn window_covers_monitor(hwnd: HWND) -> bool {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        rect.left <= info.rcMonitor.left + FULLSCREEN_TOLERANCE
            && rect.top <= info.rcMonitor.top + FULLSCREEN_TOLERANCE
            && rect.right >= info.rcMonitor.right - FULLSCREEN_TOLERANCE
            && rect.bottom >= info.rcMonitor.bottom - FULLSCREEN_TOLERANCE
    }
}

fn has_area(hwnd: HWND) -> bool {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        rect.right > rect.left && rect.bottom > rect.top
    }
}

fn cloaked(hwnd: HWND) -> bool {
    let mut value = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut value as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && value != 0
    }
}

fn skipped_class(hwnd: HWND) -> bool {
    let class = window_class(hwnd);
    SKIP_CLASSES
        .iter()
        .any(|skip| class.eq_ignore_ascii_case(skip))
}

fn tool_window(hwnd: HWND) -> bool {
    unsafe {
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let tool = ex & WS_EX_TOOLWINDOW.0 != 0;
        let app = ex & WS_EX_APPWINDOW.0 != 0;
        tool && !app
    }
}

fn window_class(hwnd: HWND) -> String {
    unsafe {
        let mut buffer = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut buffer);
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..len as usize])
    }
}

fn taskbar_is_bottom(taskbar: HWND) -> bool {
    unsafe { matches!(GetWindow(taskbar, GW_HWNDLAST), Ok(last) if last == taskbar) }
}

fn deactivate_appbar(taskbar: HWND) {
    unsafe {
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar,
            lParam: LPARAM(0),
            ..Default::default()
        };
        let _ = SHAppBarMessage(ABM_ACTIVATE, &mut appbar);
    }
}

fn place(taskbar: HWND, insert_after: HWND, show: bool) {
    let flags = if show {
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW
    } else {
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE
    };
    unsafe {
        let _ = SetWindowPos(taskbar, Some(insert_after), 0, 0, 0, 0, flags);
    }
}

fn marker_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|dir| dir.join("app.prism.launcher").join(TOPMOST_MARKER))
}

fn marker_present() -> bool {
    marker_path().is_some_and(|path| path.is_file())
}

fn write_marker(present: bool) {
    let Some(path) = marker_path() else {
        return;
    };
    if present {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, []);
    } else {
        let _ = std::fs::remove_file(&path);
    }
}

fn taskbar_window() -> Option<HWND> {
    let class = wide("Shell_TrayWnd");
    unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()).ok() }
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{band_for, StackEntry, TaskbarBand};

    #[test]
    fn alt_tab_to_windowed_app_shows_the_taskbar() {
        // The destination is windowed. A fullscreen game still open underneath
        // must not keep the taskbar at the bottom of the z-order.
        assert_eq!(
            band_for(Some(StackEntry::Normal), Some(StackEntry::Fullscreen)),
            TaskbarBand::Normal
        );
        assert_eq!(
            band_for(Some(StackEntry::Skip), Some(StackEntry::Normal)),
            TaskbarBand::Normal
        );
    }

    #[test]
    fn prism_or_missing_foreground_is_not_a_fullscreen_app() {
        // hide() often leaves Prism, or no window, as the foreground sample.
        // That transient is not the game. A windowed window underneath shows
        // the taskbar; only a real fullscreen window keeps it behind.
        assert_eq!(
            band_for(None, Some(StackEntry::Normal)),
            TaskbarBand::Normal
        );
        assert_eq!(
            band_for(Some(StackEntry::Skip), Some(StackEntry::Normal)),
            TaskbarBand::Normal
        );
        assert_eq!(
            band_for(None, Some(StackEntry::Fullscreen)),
            TaskbarBand::Bottom
        );
    }

    #[test]
    fn fullscreen_app_keeps_the_taskbar_behind() {
        assert_eq!(
            band_for(Some(StackEntry::Fullscreen), Some(StackEntry::Normal)),
            TaskbarBand::Bottom
        );
        assert_eq!(
            band_for(Some(StackEntry::Skip), Some(StackEntry::Fullscreen)),
            TaskbarBand::Bottom
        );
    }

    #[test]
    fn desktop_shows_the_taskbar() {
        assert_eq!(band_for(None, None), TaskbarBand::Normal);
        assert_eq!(band_for(Some(StackEntry::Skip), None), TaskbarBand::Normal);
    }
}
