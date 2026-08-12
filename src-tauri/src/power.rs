use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use windows::Win32::System::Shutdown::LockWorkStation;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerAction {
    Lock,
    Shutdown,
    Restart,
}

impl PowerAction {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "lock" => Ok(Self::Lock),
            "shutdown" => Ok(Self::Shutdown),
            "restart" => Ok(Self::Restart),
            _ => Err(format!("unsupported power action '{value}'")),
        }
    }

    fn shutdown_arguments(self) -> Option<[&'static str; 5]> {
        match self {
            Self::Lock => None,
            Self::Shutdown => Some(["/s", "/t", "0", "/d", "p:0:0"]),
            Self::Restart => Some(["/r", "/t", "0", "/d", "p:0:0"]),
        }
    }
}

pub fn perform(value: &str) -> Result<(), String> {
    let action = PowerAction::parse(value)?;
    if action == PowerAction::Lock {
        return unsafe { LockWorkStation() }.map_err(|error| format!("lock workstation: {error}"));
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
                PowerAction::Lock => unreachable!(),
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
    fn accepts_only_the_three_visible_actions() {
        assert_eq!(PowerAction::parse("lock"), Ok(PowerAction::Lock));
        assert_eq!(PowerAction::parse("shutdown"), Ok(PowerAction::Shutdown));
        assert_eq!(PowerAction::parse("restart"), Ok(PowerAction::Restart));
        for invalid in ["", "sleep", "hibernate", "logoff", "shutdown /f"] {
            assert!(PowerAction::parse(invalid).is_err());
        }
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
