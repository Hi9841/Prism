//! Presents the Windows taskbar alongside Prism over fullscreen windows.

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_ACTIVATE, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WS_EX_TOPMOST,
};

/// Persistent marker proving Prism made the taskbar topmost. If Prism dies
/// while the palette is open, the next launch reads the marker and restores
/// the taskbar instead of leaving it stuck above every window.
const TOPMOST_MARKER: &str = "taskbar-topmost";

static MADE_TOPMOST: AtomicBool = AtomicBool::new(false);

/// Fullscreen windows must not be covered by the taskbar; a few pixels of
/// slack avoid classifying maximized windows as fullscreen.
const FULLSCREEN_TOLERANCE: i32 = 4;

pub fn present() {
    let Some(taskbar) = taskbar_window() else {
        return;
    };
    // Only assert the topmost band when the foreground app actually covers
    // the taskbar (fullscreen games and video). In normal desktop use the
    // taskbar is already visible; forcing topmost then leaves the taskbar
    // stuck above a fullscreen game later, when the palette closes and the
    // game cannot hide a topmost taskbar.
    if !foreground_is_fullscreen() {
        return;
    }
    unsafe {
        let was_topmost = GetWindowLongPtrW(taskbar, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0;
        // Preserve ownership across duplicate presentation requests. Otherwise
        // a second call observes our own topmost change, clears this flag, and
        // prevents `release` from restoring the taskbar afterward.
        if !was_topmost {
            MADE_TOPMOST.store(true, Ordering::Release);
        }
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar,
            lParam: LPARAM(1),
            ..Default::default()
        };
        let _ = SHAppBarMessage(ABM_ACTIVATE, &mut appbar);
        let _ = ShowWindow(taskbar, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            taskbar,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        if MADE_TOPMOST.load(Ordering::Acquire) {
            write_marker(true);
        }
    }
}

/// True when the foreground window covers its entire monitor - a fullscreen
/// game or video player. Called while the palette is still hidden, so the
/// foreground window is the app the user came from.
fn foreground_is_fullscreen() -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(foreground, &mut rect).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
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

pub fn release() {
    let Some(taskbar) = taskbar_window() else {
        MADE_TOPMOST.store(false, Ordering::Release);
        write_marker(false);
        return;
    };
    unsafe {
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar,
            lParam: LPARAM(0),
            ..Default::default()
        };
        let _ = SHAppBarMessage(ABM_ACTIVATE, &mut appbar);
        if MADE_TOPMOST.swap(false, Ordering::AcqRel) {
            let _ = SetWindowPos(
                taskbar,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        write_marker(false);
    }
}

/// Startup repair: if a previous Prism instance crashed while the palette was
/// open, its marker is still on disk - release the taskbar from the topmost
/// band it left behind.
pub fn recover() {
    if !marker_present() {
        return;
    }
    let Some(taskbar) = taskbar_window() else {
        write_marker(false);
        return;
    };
    unsafe {
        let _ = SetWindowPos(
            taskbar,
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    write_marker(false);
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

fn taskbar_window() -> Option<windows::Win32::Foundation::HWND> {
    let class = wide("Shell_TrayWnd");
    unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()).ok() }
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}
