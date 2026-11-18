//! Reversible integration with installed Start-menu providers.
//!
//! StartAllBack keeps its Explorer hook active even when its classic menu is
//! disabled. `WinkeyFunction=1` forwards standalone Win to native Start;
//! `2` consumes it. Prism temporarily selects `2` while it owns Win and
//! restores the exact previous registry state afterward.

use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, LPARAM, WAIT_TIMEOUT, WPARAM};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};
use windows::Win32::System::Registry::{
    RegDeleteValueW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE,
};
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};
use crate::win32::{registry, wide, WriteError};

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
pub fn recover_stale() -> Result<(), String> {
    restore_from_journal(&journal_path())
}

/// Makes the installed provider consume standalone Win. Returns `false` when
/// a supported StartAllBack installation is unavailable, allowing the
/// generic hook fallback to mask Start instead.
pub fn enable() -> Result<bool, String> {
    if OVERRIDE_ACTIVE.load(Ordering::Acquire) {
        return Ok(true);
    }

    if !supported_provider_installed() {
        return Ok(false);
    }

    let journal_path = journal_path();
    if journal_path.exists() {
        restore_from_journal(&journal_path)?;
    }

    let Some((key_guard, original)) = open_provider_key()? else {
        return Ok(false);
    };
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
pub fn restore() -> Result<(), String> {
    let result = restore_from_journal(&journal_path());
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

    let Some((key_guard, current)) = open_provider_key()? else {
        // The provider was uninstalled while Prism was active. There is no
        // setting left to restore, so retire the journal and let its watchdog
        // exit instead of retrying forever.
        std::fs::remove_file(path)
            .map_err(|error| format!("remove obsolete Start restore journal: {error}"))?;
        return Ok(());
    };
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

fn journal_path() -> PathBuf {
    crate::host::data_dir().join(JOURNAL_NAME)
}

fn write_journal(path: &Path, journal: &RestoreJournal) -> Result<(), String> {
    let bytes = serde_json::to_vec(journal).map_err(|error| error.to_string())?;
    // Synced: the crash-recovery path must never find a torn journal.
    crate::win32::write_atomic_synced(path, &bytes).map_err(|error| match error {
        WriteError::Folder(e) => format!("create Start restore directory: {e}"),
        WriteError::Create(e) => format!("create Start restore journal: {e}"),
        WriteError::Write(e) => format!("persist Start restore journal: {e}"),
        WriteError::Replace(e) => e,
    })
}

fn open_provider_key() -> Result<Option<(registry::Key, Option<u32>)>, String> {
    let key = registry::Key::open(HKEY_CURRENT_USER, KEY_PATH, KEY_QUERY_VALUE | KEY_SET_VALUE)
        .map_err(|code| format!("open Start provider registry key: Win32 error {code}"))?;
    let Some(key) = key else {
        return Ok(None);
    };
    let value = read_dword(key.0)?;
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
    registry::dword(key, VALUE_NAME).map_err(|error| match error {
        registry::DwordError::Win32(code) => format!("read Start provider setting: Win32 error {code}"),
        registry::DwordError::NotDword => "Start provider setting is not a DWORD".to_string(),
    })
}

fn set_dword(key: HKEY, value: u32) -> Result<(), String> {
    registry::set_dword(key, VALUE_NAME, value).map_err(|code| format!("write Start provider setting: Win32 error {code}"))
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

