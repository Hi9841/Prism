//! Backstop watcher that hides native Start and Search if they still appear.
//!
//! The Win key, Start button, Search button, and Win+S/Win+Q chords are
//! intercepted before Explorer launches those surfaces. A few paths still
//! leak: Explorer restart while the bridge is down, elevated UIPI, touch.
//! While Prism owns Win, this watcher hides the known host windows so the
//! user only sees Prism.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
    ShowWindow, SW_HIDE,
};

const WATCH_INTERVAL: Duration = Duration::from_millis(50);
const SELF_GRACE: Duration = Duration::from_millis(1_500);

const WIN11_START_CLASS: &str = "XamlExplorerHostIslandWindow";
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
static LAST_OWN_TOGGLE: AtomicU64 = AtomicU64::new(0);
static CLOCK: OnceLock<Instant> = OnceLock::new();
static THREAD_STARTED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    if THREAD_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(watch_loop);
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Release);
}

pub fn note_own_toggle() {
    let clock = CLOCK.get_or_init(Instant::now);
    LAST_OWN_TOGGLE.store(clock.elapsed().as_millis() as u64 + 1, Ordering::Release);
}

pub fn should_skip_scan(enabled: bool, in_grace: bool) -> bool {
    !enabled || in_grace
}

/// True when this top-level window is the native Start menu or Search UI.
pub fn is_native_launcher(class: &str, process: &str, width: i32, height: i32) -> bool {
    if class.eq_ignore_ascii_case(WIN10_LAUNCHER) || class.eq_ignore_ascii_case(WIN10_MODE_INPUT) {
        return true;
    }
    if class.eq_ignore_ascii_case(WIN11_START_CLASS) {
        return is_menu_silhouette(width, height) && is_explorer_or_host(process);
    }
    if class.eq_ignore_ascii_case(WIN11_CORE_CLASS) {
        return is_host_process(process);
    }
    false
}

fn is_menu_silhouette(width: i32, height: i32) -> bool {
    width >= 400 && height >= 400
}

fn is_host_process(process: &str) -> bool {
    HOST_PROCESSES
        .iter()
        .any(|name| process.eq_ignore_ascii_case(name))
}

fn is_explorer_or_host(process: &str) -> bool {
    process.eq_ignore_ascii_case("explorer.exe") || is_host_process(process)
}

fn watch_loop() {
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        if should_skip_scan(ENABLED.load(Ordering::Acquire), within_self_grace()) {
            continue;
        }
        hide_native_launchers();
    }
}

fn within_self_grace() -> bool {
    let last = LAST_OWN_TOGGLE.load(Ordering::Acquire);
    if last == 0 {
        return false;
    }
    let elapsed = CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64;
    elapsed.saturating_sub(last) < SELF_GRACE.as_millis() as u64
}

fn hide_native_launchers() {
    let _ = unsafe { EnumWindows(Some(enum_hide_launcher), LPARAM(0)) };
}

unsafe extern "system" fn enum_hide_launcher(window: HWND, _detail: LPARAM) -> BOOL {
    if let Some((class, process, width, height)) = window_identity(window) {
        if is_native_launcher(&class, &process, width, height)
            && unsafe { IsWindowVisible(window) }.as_bool()
        {
            let _ = unsafe { ShowWindow(window, SW_HIDE) };
            crate::win_key::debug_trace(&format!("launcher-watch hid {class} ({process})"));
        }
    }
    BOOL(1)
}

fn window_identity(window: HWND) -> Option<(String, String, i32, i32)> {
    let class = class_name(window)?;
    let process = process_name(window).unwrap_or_default();
    let (width, height) = window_size(window);
    Some((class, process, width, height))
}

fn class_name(window: HWND) -> Option<String> {
    let mut buffer = [0u16; 128];
    let len = unsafe { GetClassNameW(window, &mut buffer) };
    if len == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..len as usize]))
}

fn window_size(window: HWND) -> (i32, i32) {
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(window, &mut rect) }.is_err() {
        return (0, 0);
    }
    (rect.right - rect.left, rect.bottom - rect.top)
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
        assert!(is_native_launcher(
            WIN11_CORE_CLASS,
            "SearchHost.exe",
            640,
            720
        ));
        assert!(is_native_launcher(
            WIN11_CORE_CLASS,
            "StartMenuExperienceHost.exe",
            860,
            900
        ));
        assert!(is_native_launcher(
            WIN11_CORE_CLASS,
            "SearchUI.exe",
            500,
            500
        ));
    }

    #[test]
    fn e2e_unrelated_core_windows_are_left_alone() {
        assert!(!is_native_launcher(
            WIN11_CORE_CLASS,
            "Notepad.exe",
            800,
            600
        ));
        assert!(!is_native_launcher(
            WIN11_CORE_CLASS,
            "ApplicationFrameHost.exe",
            1200,
            800
        ));
    }

    #[test]
    fn e2e_win11_start_island_requires_menu_size() {
        assert!(is_native_launcher(
            WIN11_START_CLASS,
            "explorer.exe",
            760,
            640
        ));
        assert!(!is_native_launcher(
            WIN11_START_CLASS,
            "explorer.exe",
            120,
            300
        ));
        assert!(!is_native_launcher(
            WIN11_START_CLASS,
            "chrome.exe",
            760,
            640
        ));
    }

    #[test]
    fn e2e_win10_launcher_classes_are_hidden() {
        assert!(is_native_launcher(WIN10_LAUNCHER, "explorer.exe", 1, 1));
        assert!(is_native_launcher(WIN10_MODE_INPUT, "explorer.exe", 1, 1));
    }

    #[test]
    fn scan_skips_when_disabled_or_in_grace() {
        assert!(should_skip_scan(false, false));
        assert!(should_skip_scan(true, true));
        assert!(!should_skip_scan(true, false));
    }
}
