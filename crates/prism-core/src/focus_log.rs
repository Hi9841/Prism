//! One line per palette open that is still open at its focus verdict: did
//! Prism get the foreground, and if not, which window kept it. Always on, so a
//! rare "the palette opened but takes no keys" leaves evidence on the owner's
//! machine. The file is `%LOCALAPPDATA%\Prism\logs\palette-focus.log`; past
//! 256 KB it moves to `palette-focus.old.log`.

use serde::Serialize;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::PWSTR;

/// What one open achieved, measured from the open.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFocus {
    /// When Prism first held the foreground; `None`: not by the verdict.
    pub foreground_after_ms: Option<u64>,
    /// Requests posted to the Explorer bridge in this open.
    pub explorer_requests: u32,
    /// Explorer's answer to the last request; `None`: no answer yet.
    pub explorer_granted: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Record<'a> {
    unix_ms: u128,
    version: &'a str,
    #[serde(flatten)]
    open: OpenFocus,
    /// The window that holds the foreground at the verdict.
    foreground_class: String,
    foreground_exe: String,
}

/// The foreground window's class and executable name. Read on the calling
/// thread: the foreground can change once the call returns.
pub fn foreground_owner() -> (String, String) {
    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() {
            return (String::new(), String::new());
        }
        let mut class = [0u16; 128];
        let length = GetClassNameW(window, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..length]);
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        let mut exe = String::new();
        // An elevated owner denies the query; the empty name says so.
        if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut path = [0u16; 512];
            let mut size = path.len() as u32;
            if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut size).is_ok() {
                let full = String::from_utf16_lossy(&path[..size as usize]);
                exe = full.rsplit('\\').next().unwrap_or_default().to_string();
            }
            let _ = CloseHandle(process);
        }
        (class, exe)
    }
}

/// Appends one record. File I/O: call it off the UI thread.
pub fn record(open: OpenFocus, owner: (String, String), version: &str) {
    if let Some(dir) = crate::logs::dir() {
        record_in(&dir, open, owner, version);
    }
}

fn record_in(dir: &std::path::Path, open: OpenFocus, owner: (String, String), version: &str) {
    let record = Record {
        unix_ms: crate::logs::unix_ms(),
        version,
        open,
        foreground_class: owner.0,
        foreground_exe: owner.1,
    };
    crate::logs::append(dir, "palette-focus", &record);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::MAX_BYTES;
    use std::path::PathBuf;

    fn open(after: Option<u64>) -> OpenFocus {
        OpenFocus { foreground_after_ms: after, explorer_requests: 2, explorer_granted: Some(false) }
    }

    #[test]
    fn writes_one_json_line_per_open_and_rotates_past_the_cap() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/focus-log-test");
        let _ = std::fs::remove_dir_all(&dir);
        record_in(&dir, open(None), ("UnrealWindow".into(), "game.exe".into()), "9.9.9");
        record_in(&dir, open(Some(42)), (String::new(), String::new()), "9.9.9");
        let text = std::fs::read_to_string(dir.join("palette-focus.log")).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["foregroundAfterMs"], serde_json::Value::Null);
        assert_eq!(lines[0]["foregroundExe"], "game.exe");
        assert_eq!(lines[0]["explorerGranted"], false);
        assert_eq!(lines[1]["foregroundAfterMs"], 42);

        std::fs::write(dir.join("palette-focus.log"), vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        record_in(&dir, open(Some(7)), (String::new(), String::new()), "9.9.9");
        assert!(dir.join("palette-focus.old.log").exists());
        assert_eq!(std::fs::read_to_string(dir.join("palette-focus.log")).unwrap().lines().count(), 1);
    }

    #[test]
    fn foreground_owner_names_a_window() {
        // Any desktop has a foreground window or none; neither may panic.
        let (class, exe) = foreground_owner();
        assert!(class.is_empty() || !class.contains('\0'));
        assert!(exe.is_empty() || exe.ends_with(".exe") || exe.ends_with(".EXE"));
    }
}
