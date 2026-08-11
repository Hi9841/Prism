//! System theme detection (light/dark) straight from the Windows registry
//! (`AppsUseLightTheme`), with a background watcher that emits an event
//! whenever the OS theme flips so the UI can follow live.

use tauri::{AppHandle, Emitter};
use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET,
};

const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const APPS_LIGHT: &str = "AppsUseLightTheme";

/// Returns Some(true) when apps should use the light theme.
pub fn apps_light() -> Option<bool> {
    unsafe {
        let mut wide_path: Vec<u16> = PERSONALIZE_KEY.encode_utf16().collect();
        wide_path.push(0);
        let mut wide_name: Vec<u16> = APPS_LIGHT.encode_utf16().collect();
        wide_name.push(0);
        let mut key = HKEY::default();
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide_path.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        );
        if open.is_ok() {
            let mut value: u32 = 0;
            let mut size = std::mem::size_of::<u32>() as u32;
            let result = RegQueryValueExW(
                key,
                PCWSTR(wide_name.as_ptr()),
                None,
                None,
                Some(&mut value as *mut u32 as *mut u8),
                Some(&mut size),
            );
            let _ = RegCloseKey(key);
            if result.is_ok() {
                return Some(value != 0);
            }
            eprintln!("[theme] query failed: {result:?} size={size}");
        } else {
            eprintln!("[theme] open failed: {open:?}");
        }
        None
    }
}

#[derive(Clone, serde::Serialize)]
struct ThemeEvent {
    theme: &'static str,
}

/// Waits for registry changes and emits `system-theme-changed` when the mode flips.
pub fn watch(app: AppHandle) {
    std::thread::spawn(move || {
        let mut wide_path: Vec<u16> = PERSONALIZE_KEY.encode_utf16().chain(Some(0)).collect();
        let mut key = HKEY::default();
        let open = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(wide_path.as_mut_ptr()),
                None,
                KEY_READ | KEY_NOTIFY,
                &mut key,
            )
        };
        if open.is_err() {
            return;
        }

        let mut last = apps_light().unwrap_or(false);
        loop {
            // Synchronous registry notification blocks this one thread without
            // periodic wakeups, CPU work, or repeated key open/close calls.
            let changed = unsafe {
                RegNotifyChangeKeyValue(key, false, REG_NOTIFY_CHANGE_LAST_SET, None, false)
            };
            if changed.is_err() {
                break;
            }
            if let Some(current) = apps_light() {
                if current != last {
                    last = current;
                    let _ = app.emit(
                        "system-theme-changed",
                        ThemeEvent {
                            theme: if current { "light" } else { "dark" },
                        },
                    );
                }
            }
        }
        unsafe {
            let _ = RegCloseKey(key);
        }
    });
}
