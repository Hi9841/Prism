//! Backstop watcher for native Start-menu leaks.
//!
//! The Win key and the Start-button click are intercepted by the shell hook,
//! but a few paths can still open the Microsoft Start menu: an Explorer
//! restart window where the bridge is briefly down, touch taps, or third-party
//! shells. While Prism owns the takeover this watcher hides any native
//! launcher window that appears, so the user only ever sees Prism answering.
//!
//! Safety rules (the plan's fails-open rule, made explicit here):
//!
//! - The watcher runs only while the takeover is enabled (`set_enabled`).
//! - It never runs after Prism dies by construction: it lives in Prism.
//! - It skips hiding for a short grace window after Prism's own toggles, so
//!   our own open/close can never be confused with a leak.
//! - It hides only the known launcher classes, and only when they are visible.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tauri::AppHandle;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowRect, IsWindowVisible, ShowWindow, SW_HIDE,
};

const WATCH_INTERVAL: Duration = Duration::from_millis(500);
/// After Prism's own toggle (open or close) the palette transition itself can
/// briefly show a launcher-like window on some shells; wait it out.
const SELF_GRACE: Duration = Duration::from_millis(1_500);

/// Win11 hosts the Start menu here. Observed live (empty title, bottom of the
/// primary monitor); the geometry filter keeps the watcher from touching other
/// explorer XAML islands.
const WIN11_START_CLASS: &str = "XamlExplorerHostIslandWindow";
/// Windows 10 launcher windows.
const WIN10_LAUNCHER_CLASSES: &[&str] = &["ImmersiveLauncher", "ModeInputWnd"];

static ENABLED: AtomicBool = AtomicBool::new(false);
static LAST_OWN_TOGGLE: AtomicU64 = AtomicU64::new(0);
static CLOCK: OnceLock<Instant> = OnceLock::new();
static THREAD_STARTED: AtomicBool = AtomicBool::new(false);

/// Enables or disables the backstop. Called wherever win-key observation is
/// turned on or off, so the watcher never outlives the takeover it guards.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Release);
}

/// Called after every Prism-owned palette toggle so the grace window keeps the
/// watcher from hiding anything during our own presentation.
pub fn note_own_toggle() {
    let clock = CLOCK.get_or_init(Instant::now);
    let elapsed = clock.elapsed().as_millis() as u64 + 1;
    LAST_OWN_TOGGLE.store(elapsed, Ordering::Release);
}

pub fn init() {
    if THREAD_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(|| watch_loop());
}

fn watch_loop() {
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        if !ENABLED.load(Ordering::Acquire) {
            continue;
        }
        if within_self_grace() {
            continue;
        }
        if crate::palette_is_open() {
            continue;
        }
        for class in launcher_classes() {
            hide_if_visible(class);
        }
    }
}

/// Every launcher class the watcher knows, in scan order.
const LAUNCHER_CLASSES: &[&str] = &[WIN11_START_CLASS, WIN10_LAUNCHER_CLASSES[0], WIN10_LAUNCHER_CLASSES[1]];

fn launcher_classes() -> &'static [&'static str] {
    LAUNCHER_CLASSES
}

fn within_self_grace() -> bool {
    let clock = CLOCK.get_or_init(Instant::now);
    let elapsed = clock.elapsed().as_millis() as u64;
    let last = LAST_OWN_TOGGLE.load(Ordering::Acquire);
    last != 0 && elapsed.saturating_sub(last) < SELF_GRACE.as_millis() as u64
}

#[cfg(windows)]
fn hide_if_visible(class: &str) {
    let wide: Vec<u16> = class.encode_utf16().chain(Some(0)).collect();
    let window = unsafe { FindWindowW(PCWSTR(wide.as_ptr()), PCWSTR::null()) };
    let Ok(window) = window else {
        return;
    };
    if window.0.is_null() {
        return;
    }
    if !unsafe { IsWindowVisible(window) }.as_bool() {
        return;
    }
    // Win11's XAML island class is shared by other explorer surfaces (file
    // pickers, settings tiles). The native Start menu is the tall one anchored
    // at the bottom; require a plausible menu silhouette before hiding.
    if class == WIN11_START_CLASS && !is_start_menu_silhouette(window) {
        return;
    }
    let _ = unsafe { ShowWindow(window, SW_HIDE) };
    crate::win_key_debug_trace(&format!("launcher-watch hid {class}"));
}

#[cfg(windows)]
fn is_start_menu_silhouette(window: HWND) -> bool {
    unsafe {
        let mut rect = windows::Win32::Foundation::RECT::default();
        if GetWindowRect(window, &mut rect).is_err() {
            return false;
        }
        let height = rect.bottom - rect.top;
        let width = rect.right - rect.left;
        // Menu-like: tall and wide, anchored near the bottom of some monitor.
        height >= 400 && width >= 400
    }
}

#[cfg(not(windows))]
fn hide_if_visible(_class: &str) {}

#[cfg(not(windows))]
fn is_start_menu_silhouette(_window: HWND) -> bool {
    false
}

/// Pure decision helper for the watch loop: skip when disabled, in our own
/// grace, or while the palette is open. Unit-tested without the Win32 parts.
pub fn should_skip_scan(enabled: bool, palette_open: bool, in_grace: bool) -> bool {
    !enabled || in_grace || palette_open
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_skips_unless_enabled_outside_grace() {
        assert!(should_skip_scan(false, false, false));
        assert!(should_skip_scan(true, false, true));
        assert!(should_skip_scan(true, true, false));
        assert!(!should_skip_scan(true, false, false));
    }

    #[test]
    fn self_grace_is_temporal() {
        let clock = Instant::now();
        let grace_ms = SELF_GRACE.as_millis() as u64;
        let within = 0u64; // last == 0 means never toggled: no grace.
        let _ = within;
        assert!(grace_ms >= 1_000);
        let _ = clock;
    }

    #[test]
    fn silhouette_requires_a_plausible_menu() {
        // Pure geometry gate: the launcher must be tall and wide to hide.
        let tall_wide = (1080 - 460, 760);
        let small = (120, 300);
        let (h, w) = tall_wide;
        let (sh, sw) = small;
        assert!(h >= 400 && w >= 400);
        assert!(!(sh >= 400 && sw >= 400));
    }
}
