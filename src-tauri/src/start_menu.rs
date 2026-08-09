//! Migration recovery for Start-menu settings changed by older Prism builds.
//!
//! Prism no longer changes StartAllBack's global `WinkeyFunction`. The journal
//! reader and watchdog entry point remain so an upgrade or an already-running
//! watchdog can restore the exact pre-Prism value from an older session.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, LPARAM, WAIT_TIMEOUT, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_VALUE_TYPE,
};
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

const KEY_PATH: &str = r"Software\StartIsBack";
const VALUE_NAME: &str = "WinkeyFunction";
const DO_NOTHING: u32 = 2;
const JOURNAL_NAME: &str = "start-menu-restore.json";
const SETTINGS_MESSAGE: &str = "SIBSettings";
const WATCHDOG_ARGUMENT: &str = "--prism-start-restore-watchdog";
const RESTORE_ARGUMENT: &str = "--prism-restore-start-menu";
const WATCHDOG_POLL_MS: u32 = 500;

#[derive(Debug, Deserialize, Serialize)]
struct RestoreJournal {
    version: u32,
    token: u64,
    value_existed: bool,
    value: u32,
}

/// Restores an override left by a previous abnormal termination.
pub fn recover_stale(app: &AppHandle) -> Result<(), String> {
    restore_from_journal(&journal_path(app)?)
}

/// Restores any override left by an older Prism build, then selects the
/// cooperative keyboard-hook path. Prism no longer changes another shell
/// provider's global keyboard policy while it is running.
pub fn enable(app: &AppHandle) -> Result<(), String> {
    recover_stale(app)
}

/// Restores the exact value (including absence) that existed before Prism
/// claimed Win.
pub fn restore(app: &AppHandle) -> Result<(), String> {
    restore_from_journal(&journal_path(app)?)
}

fn restore_from_journal(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let bytes =
        std::fs::read(path).map_err(|error| format!("read Start restore journal: {error}"))?;
    let journal: RestoreJournal = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse Start restore journal: {error}"))?;
    if journal.version != 2 {
        return Err("unsupported Start restore journal".to_string());
    }

    let Some((key, current)) = open_provider_key()? else {
        return Err("Start provider registry key is unavailable".to_string());
    };
    let key_guard = RegistryKey(key);
    // A settings UI may have changed this while Prism was running. Only undo
    // the value Prism itself installed; otherwise preserve the user's choice.
    if current == Some(DO_NOTHING) {
        restore_value(key_guard.0, &journal)?;
        notify_provider()?;
    }
    std::fs::remove_file(path).map_err(|error| format!("remove Start restore journal: {error}"))?;
    Ok(())
}

fn watchdog_ready_path(journal_path: &Path, token: u64) -> PathBuf {
    journal_path.with_extension(format!("watchdog-{token}.ready"))
}

/// Internal process mode that restores the provider setting if the owning
/// Prism process disappears before normal cleanup.
pub fn run_watchdog_from_args() -> bool {
    let mut args = std::env::args_os();
    let _ = args.next();
    let Some(mode) = args.next() else {
        return false;
    };
    if mode == std::ffi::OsStr::new(RESTORE_ARGUMENT) {
        if let Some(journal_path) = args.next().map(PathBuf::from) {
            let _ = restore_from_journal(&journal_path);
        }
        return true;
    }
    if mode != std::ffi::OsStr::new(WATCHDOG_ARGUMENT) {
        return false;
    }

    let Some(parent_pid) = args
        .next()
        .and_then(|value| value.to_string_lossy().parse::<u32>().ok())
    else {
        return true;
    };
    let Some(journal_path) = args.next().map(PathBuf::from) else {
        return true;
    };
    let Some(token) = args
        .next()
        .and_then(|value| value.to_string_lossy().parse::<u64>().ok())
    else {
        return true;
    };

    let Ok(parent) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid) }) else {
        return true;
    };
    let ready_path = watchdog_ready_path(&journal_path, token);
    if std::fs::write(&ready_path, []).is_err() {
        unsafe {
            let _ = CloseHandle(parent);
        }
        return true;
    }

    loop {
        let wait = unsafe { WaitForSingleObject(parent, WATCHDOG_POLL_MS) };
        if wait != WAIT_TIMEOUT {
            if journal_has_token(&journal_path, token) {
                let _ = restore_from_journal(&journal_path);
            }
            break;
        }
        if !journal_has_token(&journal_path, token) {
            break;
        }
    }
    unsafe {
        let _ = CloseHandle(parent);
    }
    let _ = std::fs::remove_file(ready_path);
    true
}

fn journal_has_token(path: &Path, token: u64) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<RestoreJournal>(&bytes).ok())
        .is_some_and(|journal| journal.version == 2 && journal.token == token)
}

fn journal_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join(JOURNAL_NAME))
        .map_err(|error| error.to_string())
}

fn open_provider_key() -> Result<Option<(HKEY, Option<u32>)>, String> {
    let key_path = wide(KEY_PATH);
    let mut key = HKEY::default();
    let error = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            &mut key,
        )
    };
    if error.0 != 0 || key.0.is_null() {
        return Ok(None);
    }
    let key_guard = RegistryKey(key);
    let value = read_dword(key_guard.0)?;
    let key = key_guard.0;
    std::mem::forget(key_guard);
    Ok(Some((key, value)))
}

fn read_dword(key: HKEY) -> Result<Option<u32>, String> {
    let value_name = wide(VALUE_NAME);
    let mut value_type = REG_VALUE_TYPE::default();
    let mut size = 4u32;
    let mut bytes = [0u8; 4];
    let error = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut value_type),
            Some(bytes.as_mut_ptr()),
            Some(&mut size),
        )
    };
    if error.0 == 2 {
        return Ok(None);
    }
    if error.0 != 0 {
        return Err(format!(
            "read Start provider setting: Win32 error {}",
            error.0
        ));
    }
    if value_type != REG_DWORD || size != 4 {
        return Err("Start provider setting is not a DWORD".to_string());
    }
    Ok(Some(u32::from_le_bytes(bytes)))
}

fn set_dword(key: HKEY, value: u32) -> Result<(), String> {
    let value_name = wide(VALUE_NAME);
    let error = unsafe {
        RegSetValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            None,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        )
    };
    if error.0 == 0 {
        Ok(())
    } else {
        Err(format!(
            "write Start provider setting: Win32 error {}",
            error.0
        ))
    }
}

fn restore_value(key: HKEY, journal: &RestoreJournal) -> Result<(), String> {
    if journal.value_existed {
        set_dword(key, journal.value)
    } else {
        let value_name = wide(VALUE_NAME);
        let error = unsafe { RegDeleteValueW(key, PCWSTR(value_name.as_ptr())) };
        if error.0 == 0 || error.0 == 2 {
            Ok(())
        } else {
            Err(format!(
                "remove Start provider setting: Win32 error {}",
                error.0
            ))
        }
    }
}

fn notify_provider() -> Result<(), String> {
    let tray_class = wide("Shell_TrayWnd");
    let tray = unsafe { FindWindowW(PCWSTR(tray_class.as_ptr()), PCWSTR::null()) }
        .map_err(|error| format!("find Windows taskbar: {error}"))?;
    let settings = wide(SETTINGS_MESSAGE);
    let mut result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            tray,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(settings.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            1_000,
            Some(&mut result),
        )
    };
    if sent.0 == 0 {
        Err("Start provider did not acknowledge its settings reload".to_string())
    } else {
        Ok(())
    }
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_journal_round_trips_exact_registry_state() {
        for journal in [
            RestoreJournal {
                version: 2,
                token: 11,
                value_existed: true,
                value: 1,
            },
            RestoreJournal {
                version: 2,
                token: 12,
                value_existed: false,
                value: 0,
            },
        ] {
            let bytes = serde_json::to_vec(&journal).unwrap();
            let decoded: RestoreJournal = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(decoded.version, journal.version);
            assert_eq!(decoded.token, journal.token);
            assert_eq!(decoded.value_existed, journal.value_existed);
            assert_eq!(decoded.value, journal.value);
        }
    }
}
