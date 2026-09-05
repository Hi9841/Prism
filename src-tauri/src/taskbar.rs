//! Presents the Windows taskbar alongside Prism over fullscreen windows.

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABM_ACTIVATE, ABM_GETSTATE, ABS_AUTOHIDE, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowRect, IsWindowVisible,
    SetWindowPos, ShowWindow, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
};

/// Persistent marker proving Prism presented the taskbar over a fullscreen
/// window. If Prism dies while the palette is open, the next launch reads the
/// marker and restores the taskbar instead of leaving it stuck above every
/// window.
const TOPMOST_MARKER: &str = "taskbar-topmost";

/// Tracks a temporary taskbar z-order lease for the current process. This is
/// deliberately presentation-based rather than based on the taskbar's
/// initial `WS_EX_TOPMOST` bit: Explorer commonly starts with that bit set,
/// but Prism still needs to demote the taskbar when its fullscreen presentation
/// ends.
static PRESENTED: AtomicBool = AtomicBool::new(false);

/// Fullscreen windows must not be covered by the taskbar; a few pixels of
/// slack avoid classifying maximized windows as fullscreen.
const FULLSCREEN_TOLERANCE: i32 = 4;

/// True when the taskbar is in auto-hide. `rcWork` already covers the full
/// monitor in that mode, so live tray HWNDs must not be subtracted.
pub fn auto_hide() -> bool {
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        ..Default::default()
    };
    unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) as u32 & ABS_AUTOHIDE != 0 }
}

/// Primary taskbar HWND, if Explorer has created it.
pub fn tray_present() -> bool {
    taskbar_window().is_some()
}

/// Visible primary and secondary taskbar rectangles. Windows 11's XAML
/// taskbar often leaves `rcWork` equal to the full monitor, so callers that
/// dock a window to the work area have to subtract these themselves.
pub fn bar_rects() -> Vec<RECT> {
    let mut rects = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect_taskbar_rect),
            LPARAM((&mut rects as *mut Vec<RECT>) as isize),
        );
    }
    rects
}

unsafe extern "system" fn collect_taskbar_rect(window: HWND, detail: LPARAM) -> BOOL {
    let mut class_name = [0u16; 64];
    let length = GetClassNameW(window, &mut class_name).max(0) as usize;
    let is_taskbar = class_name_is(&class_name[..length], "Shell_TrayWnd")
        || class_name_is(&class_name[..length], "Shell_SecondaryTrayWnd");
    if is_taskbar && IsWindowVisible(window).as_bool() {
        let mut rect = RECT::default();
        if GetWindowRect(window, &mut rect).is_ok()
            && rect.right > rect.left
            && rect.bottom > rect.top
        {
            (*(detail.0 as *mut Vec<RECT>)).push(rect);
        }
    }
    BOOL(1)
}

fn class_name_is(actual: &[u16], expected: &str) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected.bytes())
            .all(|(actual, expected)| (*actual as u8).eq_ignore_ascii_case(&expected))
}

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
        // Preserve ownership across duplicate presentation requests. The
        // taskbar may already be topmost before Prism opens, but this call
        // still creates a temporary lease that must be released afterward.
        PRESENTED.store(true, Ordering::Release);
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
        write_marker(true);
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
        PRESENTED.store(false, Ordering::Release);
        write_marker(false);
        return;
    };
    let fullscreen_foreground = foreground_is_fullscreen();
    unsafe {
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar,
            lParam: LPARAM(0),
            ..Default::default()
        };
        let _ = SHAppBarMessage(ABM_ACTIVATE, &mut appbar);
        // Release even when Explorer had the taskbar in the topmost band
        // before Prism opened. `present()` owns the temporary presentation,
        // not only the transition from a non-topmost style. HWND_NOTOPMOST
        // alone would leave the taskbar at the top of the normal z-order,
        // still above a borderless fullscreen game, so put it at the bottom
        // while that game is foreground.
        if PRESENTED.swap(false, Ordering::AcqRel) || marker_present() {
            let insert_after = if fullscreen_foreground {
                HWND_BOTTOM
            } else {
                HWND_NOTOPMOST
            };
            let _ = SetWindowPos(
                taskbar,
                Some(insert_after),
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
