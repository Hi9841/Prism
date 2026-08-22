use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use std::mem::size_of;

use windows::Win32::Foundation::{
    CloseHandle, GetLastError, SetLastError, ERROR_NOT_ALL_ASSIGNED, ERROR_SUCCESS, HANDLE, LUID,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerAction {
    Lock,
    Sleep,
    Shutdown,
    Restart,
}

impl PowerAction {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "lock" => Ok(Self::Lock),
            "sleep" => Ok(Self::Sleep),
            "shutdown" => Ok(Self::Shutdown),
            "restart" => Ok(Self::Restart),
            _ => Err(format!("unsupported power action '{value}'")),
        }
    }

    fn shutdown_arguments(self) -> Option<[&'static str; 5]> {
        match self {
            Self::Lock | Self::Sleep => None,
            Self::Shutdown => Some(["/s", "/t", "0", "/d", "p:0:0"]),
            Self::Restart => Some(["/r", "/t", "0", "/d", "p:0:0"]),
        }
    }

    fn suspend_parameters(self) -> Option<(bool, bool, bool)> {
        match self {
            Self::Sleep => Some((false, false, false)),
            Self::Lock | Self::Shutdown | Self::Restart => None,
        }
    }
}

struct OwnedToken(HANDLE);

impl Drop for OwnedToken {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct ShutdownPrivilege {
    token: OwnedToken,
    previous: TOKEN_PRIVILEGES,
}

impl ShutdownPrivilege {
    fn enable() -> Result<Self, String> {
        let mut token = HANDLE::default();
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )
        }
        .map_err(|error| format!("open process token: {error}"))?;

        let token = OwnedToken(token);
        let mut luid = LUID::default();
        unsafe { LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid) }
            .map_err(|error| format!("look up shutdown privilege: {error}"))?;

        let requested = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let mut previous = TOKEN_PRIVILEGES::default();
        let mut previous_size = 0;

        unsafe {
            SetLastError(ERROR_SUCCESS);
            AdjustTokenPrivileges(
                token.0,
                false,
                Some(&requested),
                size_of::<TOKEN_PRIVILEGES>() as u32,
                Some(&mut previous),
                Some(&mut previous_size),
            )
        }
        .map_err(|error| format!("enable shutdown privilege: {error}"))?;

        if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
            return Err("enable shutdown privilege: privilege is not available".to_string());
        }

        Ok(Self { token, previous })
    }
}

impl Drop for ShutdownPrivilege {
    fn drop(&mut self) {
        unsafe {
            let _ = AdjustTokenPrivileges(self.token.0, false, Some(&self.previous), 0, None, None);
        }
    }
}

fn sleep_computer() -> Result<(), String> {
    let _privilege = ShutdownPrivilege::enable()?;
    let (hibernate, force, disable_wake_events) = PowerAction::Sleep
        .suspend_parameters()
        .expect("sleep has suspend parameters");

    if unsafe { SetSuspendState(hibernate, force, disable_wake_events) } {
        Ok(())
    } else {
        Err(format!(
            "put computer to sleep: {}",
            windows::core::Error::from_win32()
        ))
    }
}

pub fn perform(value: &str) -> Result<(), String> {
    let action = PowerAction::parse(value)?;
    match action {
        PowerAction::Lock => {
            return unsafe { LockWorkStation() }
                .map_err(|error| format!("lock workstation: {error}"));
        }
        PowerAction::Sleep => return sleep_computer(),
        PowerAction::Shutdown | PowerAction::Restart => {}
    }

    let executable = shutdown_executable()?;
    let Some(arguments) = action.shutdown_arguments() else {
        return Err("internal error: shutdown action has no arguments".to_string());
    };
    let status = Command::new(&executable)
        .args(arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|error| format!("start {}: {error}", executable.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Windows rejected the {} request (exit code {})",
            match action {
                PowerAction::Shutdown => "shutdown",
                PowerAction::Restart => "restart",
                PowerAction::Lock | PowerAction::Sleep => unreachable!(),
            },
            status
                .code()
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        ))
    }
}

fn shutdown_executable() -> Result<PathBuf, String> {
    let system_root = std::env::var_os("SystemRoot")
        .ok_or_else(|| "Windows SystemRoot is unavailable".to_string())?;
    let path = PathBuf::from(system_root)
        .join("System32")
        .join("shutdown.exe");
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "Windows shutdown tool is missing: {}",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_the_four_visible_actions() {
        assert_eq!(PowerAction::parse("lock"), Ok(PowerAction::Lock));
        assert_eq!(PowerAction::parse("sleep"), Ok(PowerAction::Sleep));
        assert_eq!(PowerAction::parse("shutdown"), Ok(PowerAction::Shutdown));
        assert_eq!(PowerAction::parse("restart"), Ok(PowerAction::Restart));
        for invalid in ["", "hibernate", "logoff", "shutdown /f"] {
            assert!(PowerAction::parse(invalid).is_err());
        }
    }

    #[test]
    fn sleep_uses_standby_and_preserves_wake_events() {
        assert_eq!(
            PowerAction::Sleep.suspend_parameters(),
            Some((false, false, false))
        );
        assert_eq!(PowerAction::Sleep.shutdown_arguments(), None);
    }

    #[test]
    fn shutdown_commands_never_force_close_apps() {
        assert_eq!(PowerAction::Lock.shutdown_arguments(), None);
        for action in [PowerAction::Shutdown, PowerAction::Restart] {
            let arguments = action.shutdown_arguments().unwrap();
            assert_eq!(arguments[1..], ["/t", "0", "/d", "p:0:0"]);
            assert!(!arguments.contains(&"/f"));
        }
    }
}
