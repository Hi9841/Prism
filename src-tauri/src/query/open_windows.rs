//! Visible top-level windows for Phase 1 (EnumWindows). Kept in-process.

use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, BringWindowToTop, EnumWindows, GetAncestor, GetClassNameW,
    GetForegroundWindow, GetLastActivePopup, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, LockSetForegroundWindow,
    SetForegroundWindow, SetWindowPos, ShowWindow, GA_ROOTOWNER, GWL_EXSTYLE, HWND_NOTOPMOST,
    HWND_TOPMOST, LSFW_UNLOCK, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_RESTORE,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

const SKIP_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.Internal.Shell.TabProxyWindow",
    "IME",
    "MSCTFIME UI",
];

#[derive(Clone, Debug)]
pub struct OpenWindow {
    pub hwnd: i64,
    pub title: String,
    pub process_name: String,
}

pub fn list() -> Vec<OpenWindow> {
    let mut windows = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect_window),
            LPARAM((&mut windows as *mut Vec<OpenWindow>) as isize),
        );
    }
    windows
}

pub fn search(query: &str, windows: &[OpenWindow], limit: usize) -> Vec<(OpenWindow, i32)> {
    let mut ranked: Vec<(OpenWindow, i32)> = windows
        .iter()
        .filter_map(|window| {
            let title_score = super::score::score_text(query, &window.title);
            let process_score = super::score::score_text(query, &window.process_name)
                .map(|score| (score - 40).max(0));
            match (title_score, process_score) {
                (Some(title), Some(process)) => Some(title.max(process)),
                (Some(title), None) => Some(title),
                (None, Some(process)) if process > 0 => Some(process),
                _ => None,
            }
            .map(|score| (window.clone(), score))
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.title.to_lowercase().cmp(&b.0.title.to_lowercase()))
    });
    ranked.truncate(limit);
    ranked
}

pub fn focus(hwnd_value: i64) -> Result<(), String> {
    if hwnd_value == 0 {
        return Err("invalid window handle".to_string());
    }
    let hwnd = HWND(hwnd_value as *mut std::ffi::c_void);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err("that window is no longer open".to_string());
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let mut target_pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut target_pid));
        // A single SetForegroundWindow is denied whenever the caller is not
        // the foreground owner yet (palette just opened behind an elevated
        // window, hide/show racing, Explorer holding activation). Unlock,
        // grant the target, and retry: success is the foreground actually
        // moving, not the call returning nonzero once.
        for _ in 0..5 {
            let _ = LockSetForegroundWindow(LSFW_UNLOCK);
            if target_pid != 0 {
                let _ = AllowSetForegroundWindow(target_pid);
            }
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetActiveWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            if foreground_is_target(hwnd, target_pid) {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        // Last resort: force the z-order even without activation so the
        // window is at least visible on top instead of silently staying
        // behind, then try activation once more without forcing topmost.
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = SetForegroundWindow(hwnd);
        if foreground_is_target(hwnd, target_pid) {
            return Ok(());
        }
        Err("Windows did not switch to that window".to_string())
    }
}

/// True when the target took the foreground. Accept a same-process
/// redirect: SetForegroundWindow may activate a different top-level HWND
/// (popup/owner) in the target app instead of the exact handle we stored.
fn foreground_is_target(hwnd: HWND, target_pid: u32) -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground == hwnd {
            return true;
        }
        if target_pid == 0 || foreground.0.is_null() {
            return false;
        }
        let mut foreground_pid = 0u32;
        GetWindowThreadProcessId(foreground, Some(&mut foreground_pid));
        foreground_pid != 0 && foreground_pid == target_pid
    }
}

unsafe extern "system" fn collect_window(hwnd: HWND, detail: LPARAM) -> BOOL {
    if let Some(window) = describe(hwnd) {
        let windows = unsafe { &mut *(detail.0 as *mut Vec<OpenWindow>) };
        if windows.len() < 64 {
            windows.push(window);
        }
    }
    BOOL(1)
}

fn describe(hwnd: HWND) -> Option<OpenWindow> {
    unsafe {
        if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        if !is_switch_target(hwnd) {
            return None;
        }
        let class_name = window_class(hwnd);
        if SKIP_CLASSES
            .iter()
            .any(|skip| class_name.eq_ignore_ascii_case(skip))
        {
            return None;
        }
        let title = window_title(hwnd);
        if title.is_empty() {
            return None;
        }
        let mut process_id = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        if process_id == 0 || process_id == GetCurrentProcessId() {
            return None;
        }
        Some(OpenWindow {
            hwnd: hwnd.0 as i64,
            title,
            process_name: process_stem(process_id).unwrap_or_default(),
        })
    }
}

fn is_switch_target(hwnd: HWND) -> bool {
    unsafe {
        let owner = GetAncestor(hwnd, GA_ROOTOWNER);
        let mut walk = if owner.0.is_null() { hwnd } else { owner };
        for _ in 0..8 {
            let popup = GetLastActivePopup(walk);
            if popup.0.is_null() || popup == walk || IsWindowVisible(popup).as_bool() {
                if !popup.0.is_null() {
                    walk = popup;
                }
                break;
            }
            walk = popup;
        }
        if walk != hwnd {
            return false;
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let tool = ex & WS_EX_TOOLWINDOW.0 != 0;
        let app = ex & WS_EX_APPWINDOW.0 != 0;
        !tool || app
    }
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let written = GetWindowTextW(hwnd, &mut buffer);
        if written <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..written as usize])
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

fn process_stem(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let success = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if success.is_err() || size == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        std::path::Path::new(&path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_ranks_title_matches() {
        let windows = vec![
            OpenWindow {
                hwnd: 1,
                title: "Discord".into(),
                process_name: "Discord".into(),
            },
            OpenWindow {
                hwnd: 2,
                title: "Display settings".into(),
                process_name: "SystemSettings".into(),
            },
        ];
        let hits = search("disc", &windows, 4);
        assert_eq!(hits[0].0.title, "Discord");
    }

    #[test]
    fn empty_query_matches_nothing() {
        let windows = vec![OpenWindow {
            hwnd: 1,
            title: "Discord".into(),
            process_name: "Discord".into(),
        }];
        assert!(search("", &windows, 4).is_empty());
    }
}
