//! Presents the Windows taskbar alongside Prism over fullscreen windows.

use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_ACTIVATE, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
    WS_EX_TOPMOST,
};

static MADE_TOPMOST: AtomicBool = AtomicBool::new(false);

pub fn present() {
    let Some(taskbar) = taskbar_window() else {
        return;
    };
    unsafe {
        let was_topmost = GetWindowLongPtrW(taskbar, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0;
        MADE_TOPMOST.store(!was_topmost, Ordering::Release);
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
}

pub fn release() {
    let Some(taskbar) = taskbar_window() else {
        MADE_TOPMOST.store(false, Ordering::Release);
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
