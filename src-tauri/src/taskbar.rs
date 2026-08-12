//! Presents the Windows taskbar alongside Prism over fullscreen windows and
//! reveals an auto-hidden taskbar for the duration of the palette.

use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABM_ACTIVATE, ABM_GETSTATE, ABM_SETSTATE, ABS_AUTOHIDE, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
    WS_EX_TOPMOST,
};

static MADE_TOPMOST: AtomicBool = AtomicBool::new(false);
/// True while Prism has force-revealed an auto-hidden taskbar.
static REVEALED_AUTOHIDE: AtomicBool = AtomicBool::new(false);
/// The appbar state observed before the reveal, re-applied by `release`.
static REVEALED_FROM_STATE: AtomicU32 = AtomicU32::new(0);

/// Presents the taskbar alongside Prism. Returns true when an auto-hidden
/// taskbar was revealed; the shell updates its geometry asynchronously, so
/// callers should re-read the work area before positioning their window.
pub fn present() -> bool {
    let Some(taskbar) = taskbar_window() else {
        return false;
    };
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
    }
    reveal_auto_hidden(taskbar)
}

/// While the palette is open, an auto-hidden taskbar is revealed so a bare
/// Win press keeps its native "show the taskbar" behavior. Prism consumes the
/// shell's Start command (which normally performs the reveal), so the appbar
/// state must be changed here. The exact previous state is restored on
/// `release`, and a user changing the setting meanwhile is never undone.
fn reveal_auto_hidden(taskbar: windows::Win32::Foundation::HWND) -> bool {
    unsafe {
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar,
            ..Default::default()
        };
        let current = SHAppBarMessage(ABM_GETSTATE, &mut appbar) as u32;
        if current & ABS_AUTOHIDE == 0 {
            return false;
        }
        appbar.lParam = LPARAM((current & !ABS_AUTOHIDE) as isize);
        let applied = SHAppBarMessage(ABM_SETSTATE, &mut appbar) != 0;
        if applied {
            REVEALED_AUTOHIDE.store(true, Ordering::Release);
            REVEALED_FROM_STATE.store(current, Ordering::Release);
        }
        applied
    }
}

pub fn release() {
    let Some(taskbar) = taskbar_window() else {
        MADE_TOPMOST.store(false, Ordering::Release);
        restore_auto_hidden();
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
    }
    restore_auto_hidden();
}

/// Re-applies ABS_AUTOHIDE only when the state is still exactly what Prism
/// set, so a setting change made while the palette is open is preserved.
fn restore_auto_hidden() {
    if !REVEALED_AUTOHIDE.swap(false, Ordering::AcqRel) {
        return;
    }
    let original = REVEALED_FROM_STATE.swap(0, Ordering::AcqRel);
    unsafe {
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            ..Default::default()
        };
        let current = SHAppBarMessage(ABM_GETSTATE, &mut appbar) as u32;
        if current == original & !ABS_AUTOHIDE {
            appbar.lParam = LPARAM(original as isize);
            let _ = SHAppBarMessage(ABM_SETSTATE, &mut appbar);
        }
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
