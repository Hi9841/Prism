//! Reversible integration with installed Start-menu providers.
//!
//! StartAllBack keeps its Explorer hook active even when its classic menu is
//! disabled. `WinkeyFunction=1` forwards standalone Win to native Start;
//! `2` consumes it. Prism temporarily selects `2` while it owns Win and
//! restores the exact previous registry state afterward.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, LPARAM, WAIT_TIMEOUT, WPARAM};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};
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
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const WATCHDOG_POLL_MS: u32 = 500;
const WATCHDOG_READY_TIMEOUT: Duration = Duration::from_secs(2);

static OVERRIDE_ACTIVE: AtomicBool = AtomicBool::new(false);

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

/// Makes the installed provider consume standalone Win. Returns `false` when
/// a supported StartAllBack installation is unavailable, allowing the
/// generic hook fallback to mask Start instead.
pub fn enable(app: &AppHandle) -> Result<bool, String> {
    if OVERRIDE_ACTIVE.load(Ordering::Acquire) {
        return Ok(true);
    }

    if !supported_provider_installed() {
        return Ok(false);
    }

    let journal_path = journal_path(app)?;
    if journal_path.exists() {
        restore_from_journal(&journal_path)?;
    }

    let Some((key, original)) = open_provider_key()? else {
        return Ok(false);
    };
    let key_guard = RegistryKey(key);
    if original == Some(DO_NOTHING) {
        return Ok(true);
    }

    let journal = RestoreJournal {
        version: 2,
        token: journal_token(),
        value_existed: original.is_some(),
        value: original.unwrap_or_default(),
    };
    write_journal(&journal_path, &journal)?;
    if let Err(error) = start_watchdog(&journal_path, journal.token) {
        let _ = std::fs::remove_file(&journal_path);
        return Err(error);
    }

    if let Err(error) = set_dword(key_guard.0, DO_NOTHING).and_then(|_| notify_provider()) {
        if restore_value(key_guard.0, &journal)
            .and_then(|_| notify_provider())
            .is_ok()
        {
            let _ = std::fs::remove_file(&journal_path);
        }
        return Err(error);
    }

    OVERRIDE_ACTIVE.store(true, Ordering::Release);
    Ok(true)
}

/// Restores the exact value (including absence) that existed before Prism
/// claimed Win.
pub fn restore(app: &AppHandle) -> Result<(), String> {
    let result = restore_from_journal(&journal_path(app)?);
    if result.is_ok() {
        OVERRIDE_ACTIVE.store(false, Ordering::Release);
    }
    result
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
        // The provider was uninstalled while Prism was active. There is no
        // setting left to restore, so retire the journal and let its watchdog
        // exit instead of retrying forever.
        std::fs::remove_file(path)
            .map_err(|error| format!("remove obsolete Start restore journal: {error}"))?;
        return Ok(());
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

fn journal_token() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    nanos ^ ((std::process::id() as u64) << 32)
}

fn start_watchdog(journal_path: &Path, token: u64) -> Result<(), String> {
    let ready_path = watchdog_ready_path(journal_path, token);
    let _ = std::fs::remove_file(&ready_path);
    let executable = std::env::current_exe()
        .map_err(|error| format!("locate Prism crash-recovery executable: {error}"))?;
    let mut child = Command::new(executable)
        .arg(WATCHDOG_ARGUMENT)
        .arg(std::process::id().to_string())
        .arg(journal_path)
        .arg(token.to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| format!("start Prism crash-recovery watchdog: {error}"))?;

    let deadline = std::time::Instant::now() + WATCHDOG_READY_TIMEOUT;
    while !ready_path.exists() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Prism crash-recovery watchdog did not become ready".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_file(ready_path);
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

fn write_journal(path: &Path, journal: &RestoreJournal) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Start restore journal has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create Start restore directory: {error}"))?;
    let bytes = serde_json::to_vec(journal).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("create Start restore journal: {error}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("persist Start restore journal: {error}"))
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
    if error.0 == 2 {
        return Ok(None);
    }
    if error.0 != 0 || key.0.is_null() {
        return Err(format!(
            "open Start provider registry key: Win32 error {}",
            error.0
        ));
    }
    let key_guard = RegistryKey(key);
    let value = read_dword(key_guard.0)?;
    let key = key_guard.0;
    std::mem::forget(key_guard);
    Ok(Some((key, value)))
}

fn supported_provider_installed() -> bool {
    ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .map(|base| base.join("StartAllBack").join("StartAllBackX64.dll"))
        .any(|dll| file_version(&dll).is_some_and(supported_provider_version))
}

fn supported_provider_version(version: (u16, u16, u16, u16)) -> bool {
    // The inspected utility and StartAllBack's current registry contract both
    // target the v3 product line. Minor/build revisions retain WinkeyFunction
    // semantics, so pinning one exact build strands otherwise compatible users.
    version.0 == 3
}

fn file_version(path: &Path) -> Option<(u16, u16, u16, u16)> {
    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(path_wide.as_ptr()), None) };
    if size == 0 {
        return None;
    }
    let mut bytes = vec![0u8; size as usize];
    unsafe {
        GetFileVersionInfoW(
            PCWSTR(path_wide.as_ptr()),
            None,
            size,
            bytes.as_mut_ptr().cast(),
        )
        .ok()?;
    }
    let root = wide(r"\");
    let mut fixed = std::ptr::null_mut();
    let mut fixed_size = 0u32;
    if !unsafe {
        VerQueryValueW(
            bytes.as_ptr().cast(),
            PCWSTR(root.as_ptr()),
            &mut fixed,
            &mut fixed_size,
        )
    }
    .as_bool()
        || fixed.is_null()
        || fixed_size < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
    {
        return None;
    }
    let fixed = unsafe { &*(fixed as *const VS_FIXEDFILEINFO) };
    Some((
        (fixed.dwFileVersionMS >> 16) as u16,
        fixed.dwFileVersionMS as u16,
        (fixed.dwFileVersionLS >> 16) as u16,
        fixed.dwFileVersionLS as u16,
    ))
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

    #[test]
    fn every_startallback_v3_build_uses_provider_suppression() {
        assert!(supported_provider_version((3, 0, 0, 0)));
        assert!(supported_provider_version((3, 9, 24, 5377)));
        assert!(supported_provider_version((
            3,
            u16::MAX,
            u16::MAX,
            u16::MAX
        )));
        assert!(!supported_provider_version((2, 9, 24, 5377)));
        assert!(!supported_provider_version((4, 0, 0, 0)));
    }
}
