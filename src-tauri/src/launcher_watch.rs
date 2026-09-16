//! Backstop watcher that hides native Start and Search if they still appear.
//!
//! The Win key, Start button, Search button, and Win+S/Win+Q chords are
//! intercepted before Explorer launches those surfaces. A few paths still
//! leak: Explorer restart while the bridge is down, elevated UIPI, touch.
//! While Prism owns Win, this watcher hides the known host windows so the
//! user only sees Prism. Windows this module hid are shown again when Win
//! observation is turned off.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, ShowWindow,
    SW_HIDE, SW_SHOWNOACTIVATE,
};

const WATCH_INTERVAL: Duration = Duration::from_millis(50);

const WIN11_CORE_CLASS: &str = "Windows.UI.Core.CoreWindow";
const WIN10_LAUNCHER: &str = "ImmersiveLauncher";
const WIN10_MODE_INPUT: &str = "ModeInputWnd";

const HOST_PROCESSES: &[&str] = &[
    "StartMenuExperienceHost.exe",
    "SearchHost.exe",
    "SearchApp.exe",
    "SearchUI.exe",
];

static ENABLED: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static THREAD_STARTED: AtomicBool = AtomicBool::new(false);
static HIDDEN: Mutex<Vec<isize>> = Mutex::new(Vec::new());

pub fn init() {
    if THREAD_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(watch_loop);
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Release);
    if !on {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        restore_hidden();
    }
}

pub fn should_skip_scan(enabled: bool) -> bool {
    !enabled
}

/// True when this top-level window is the native Start menu or Search UI.
pub fn is_native_launcher(class: &str, process: &str) -> bool {
    if class.eq_ignore_ascii_case(WIN10_LAUNCHER) || class.eq_ignore_ascii_case(WIN10_MODE_INPUT) {
        return true;
    }
    class.eq_ignore_ascii_case(WIN11_CORE_CLASS) && is_host_process(process)
}

fn is_host_process(process: &str) -> bool {
    HOST_PROCESSES
        .iter()
        .any(|name| process.eq_ignore_ascii_case(name))
}

fn watch_loop() {
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        if should_skip_scan(ENABLED.load(Ordering::Acquire)) {
            continue;
        }
        hide_native_launchers();
    }
}

fn hide_native_launchers() {
    let _ = unsafe { EnumWindows(Some(enum_hide_launcher), LPARAM(0)) };
}

fn restore_hidden() {
    let hwnds = HIDDEN
        .lock()
        .map(|mut list| std::mem::take(&mut *list))
        .unwrap_or_default();
    for id in hwnds {
        let window = HWND(id as *mut _);
        if unsafe { IsWindow(Some(window)) }.as_bool() {
            let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
        }
    }
}

fn remember_hidden(window: HWND) {
    let id = window.0 as isize;
    if let Ok(mut list) = HIDDEN.lock() {
        if !list.contains(&id) {
            list.push(id);
        }
    }
}

unsafe extern "system" fn enum_hide_launcher(window: HWND, _detail: LPARAM) -> BOOL {
    if let Some((class, process)) = window_identity(window) {
        if is_native_launcher(&class, &process) && unsafe { IsWindowVisible(window) }.as_bool() {
            hide_or_rollback(window, &class, &process);
        }
    }
    BOOL(1)
}

fn hide_or_rollback(window: HWND, class: &str, process: &str) {
    let generation = GENERATION.load(Ordering::Acquire);
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let _ = unsafe { ShowWindow(window, SW_HIDE) };
    if ENABLED.load(Ordering::Acquire) && GENERATION.load(Ordering::Acquire) == generation {
        remember_hidden(window);
        crate::win_key::debug_trace(&format!("launcher-watch hid {class} ({process})"));
        return;
    }
    let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
}

fn window_identity(window: HWND) -> Option<(String, String)> {
    let class = class_name(window)?;
    let process = process_name(window).unwrap_or_default();
    Some((class, process))
}

fn class_name(window: HWND) -> Option<String> {
    let mut buffer = [0u16; 128];
    let len = unsafe { GetClassNameW(window, &mut buffer) };
    if len == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..len as usize]))
}

fn process_name(window: HWND) -> Option<String> {
    unsafe {
        let mut process_id = 0;
        if GetWindowThreadProcessId(window, Some(&mut process_id)) == 0 || process_id == 0 {
            return None;
        }
        crate::audio::get_process_name(process_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2e_search_host_core_window_is_hidden() {
        assert!(is_native_launcher(WIN11_CORE_CLASS, "SearchHost.exe"));
        assert!(is_native_launcher(
            WIN11_CORE_CLASS,
            "StartMenuExperienceHost.exe"
        ));
        assert!(is_native_launcher(WIN11_CORE_CLASS, "SearchUI.exe"));
    }

    #[test]
    fn e2e_unrelated_core_windows_are_left_alone() {
        assert!(!is_native_launcher(WIN11_CORE_CLASS, "Notepad.exe"));
        assert!(!is_native_launcher(
            WIN11_CORE_CLASS,
            "ApplicationFrameHost.exe"
        ));
        assert!(!is_native_launcher(
            "XamlExplorerHostIslandWindow",
            "explorer.exe"
        ));
    }

    #[test]
    fn e2e_win10_launcher_classes_are_hidden() {
        assert!(is_native_launcher(WIN10_LAUNCHER, "explorer.exe"));
        assert!(is_native_launcher(WIN10_MODE_INPUT, "explorer.exe"));
    }

    #[test]
    fn scan_skips_only_when_disabled() {
        assert!(should_skip_scan(false));
        assert!(!should_skip_scan(true));
    }
}
