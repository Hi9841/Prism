//! Starting apps and files: open, run as administrator, open location,
//! properties, and packaged-app activation.

use std::path::Path;
use std::process::Command;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoAllowSetForegroundWindow, CoCreateInstance, CLSCTX_LOCAL_SERVER,
};
use windows::Win32::UI::Shell::{
    ApplicationActivationManager, IApplicationActivationManager,
    ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems,
    ShellExecuteExW, ShellExecuteW, AO_NONE, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use crate::win32::{wide, ComGuard};
use crate::apps::AppEntry;

/// Launches an app the way the real Start menu would: shortcut-backed
/// entries run through their .lnk/.url (so the shell applies the working
/// directory, arguments and environment exactly as authored), packaged apps
/// go through shell:AppsFolder by AUMID, everything else runs the resolved
/// target directly.
pub fn launch(app: &AppEntry) -> Result<(), String> {
    unsafe {
        if let Some(path) = app.path.as_deref().filter(|path| is_executable(path)) {
            if shell_open(path, app.args.as_deref(), app.working_directory.as_deref()) {
                return Ok(());
            }
        }
        if let Some(location) = &app.location {
            if !location.is_empty()
                && Path::new(location).exists()
                && shell_open(location, None, None)
            {
                return Ok(());
            }
        }
        if let Some(aumid) = &app.aumid {
            // Packaged app ids are `PackageFamilyName!AppId`. Activation always
            // fails on a desktop app id, which only the Apps folder opens.
            if aumid.contains('!') && activate_packaged_app(aumid).is_ok() {
                return Ok(());
            }
            return explorer_arg(&format!("shell:AppsFolder\\{aumid}"));
        }
        if let Some(path) = &app.path {
            if !path.is_empty()
                && shell_open(path, app.args.as_deref(), app.working_directory.as_deref())
            {
                return Ok(());
            }
        }
        Err(format!("failed to launch {}", app.name))
    }
}

/// Launches a scanned desktop application or script through Windows' `runas`
/// verb. Packaged apps have no local process target and are intentionally not
/// eligible for elevation.
pub fn launch_elevated(app: &AppEntry) -> Result<(), String> {
    let path = app
        .path
        .as_deref()
        .map(Path::new)
        .filter(|path| path.is_absolute() && path.is_file())
        .ok_or_else(|| format!("{} cannot be run as administrator", app.name))?;
    launch_path_elevated(path, app.args.as_deref(), app.working_directory.as_deref())
}

pub fn launch_path_elevated(
    path: &Path,
    args: Option<&str>,
    working_directory: Option<&str>,
) -> Result<(), String> {
    if !path.is_file() || !is_elevatable_path(path) {
        return Err(format!(
            "{} is not a supported application or script",
            path.display()
        ));
    }

    let path_text = path.to_string_lossy();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let fallback_directory = path.parent().map(|parent| parent.to_string_lossy());
    let directory = working_directory
        .filter(|value| !value.is_empty())
        .or(fallback_directory.as_deref());
    let (target, parameters) = elevated_command(&path_text, &extension, args);

    unsafe { shell_execute("runas", &target, parameters.as_deref(), directory) }.map_err(|code| {
        if code == 5 {
            "Administrator permission was not granted".to_string()
        } else {
            format!(
                "failed to run {} as administrator (Shell error {code})",
                path.display()
            )
        }
    })
}

pub fn is_elevatable_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| prism_vocab::ELEVATABLE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

fn elevated_command(path: &str, extension: &str, args: Option<&str>) -> (String, Option<String>) {
    let clean_path = path.trim().trim_matches('"');
    let trailing_args = args
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!(" {value}"))
        .unwrap_or_default();
    match extension {
        "bat" | "cmd" => (
            "cmd.exe".to_string(),
            Some(format!("/d /s /c \"\"{clean_path}\"{trailing_args}\"")),
        ),
        "ps1" => (
            "powershell.exe".to_string(),
            Some(format!(
                "-NoLogo -NoProfile -File \"{clean_path}\"{trailing_args}"
            )),
        ),
        "vbs" | "js" | "wsf" => (
            "wscript.exe".to_string(),
            Some(format!("//nologo \"{clean_path}\"{trailing_args}")),
        ),
        _ => (clean_path.to_string(), args.map(str::to_string)),
    }
}

fn activate_packaged_app(aumid: &str) -> Result<(), String> {
    let _com = ComGuard::init();
    unsafe {
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                .map_err(|error| format!("failed to create app activation manager: {error}"))?;
        // The app takes the foreground only with Prism's right to it; without
        // it the app opens behind the window the user came from.
        let _ = CoAllowSetForegroundWindow(&manager, None);
        let app_id = wide(aumid);
        manager
            .ActivateApplication(PCWSTR(app_id.as_ptr()), PCWSTR::null(), AO_NONE)
            .map(|_| ())
            .map_err(|error| format!("failed to activate {aumid}: {error}"))
    }
}

pub fn open_path(path: &Path) -> Result<(), String> {
    let value = path.to_string_lossy();
    if unsafe { shell_open(&value, None, None) } {
        Ok(())
    } else {
        Err(format!("failed to open {}", path.display()))
    }
}

pub fn open_path_location(path: &Path) -> Result<(), String> {
    let _com = ComGuard::init();
    let wide_path = wide(&path.to_string_lossy());
    unsafe {
        let item_idl = ILCreateFromPathW(PCWSTR(wide_path.as_ptr()));
        if !item_idl.is_null() {
            let hr = SHOpenFolderAndSelectItems(item_idl, None, 0);
            ILFree(Some(item_idl));
            if hr.is_ok() {
                return Ok(());
            }
        }
        let value = path.to_string_lossy();
        if shell_execute(
            "open",
            "explorer.exe",
            Some(&format!("/select,\"{value}\"")),
            None,
        )
        .is_ok()
        {
            Ok(())
        } else {
            Err(format!("failed to locate {}", path.display()))
        }
    }
}

/// Opens the Windows Properties dialog for a file or folder through the
/// shell's handler-backed `properties` verb.
pub fn show_properties(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    unsafe { shell_execute_verb("properties", path) }.map_err(|code| {
        format!(
            "failed to open properties for {} (Shell error {code})",
            path.display()
        )
    })
}

fn is_executable(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        && Path::new(path).is_file()
}

unsafe fn shell_open(path: &str, args: Option<&str>, working_directory: Option<&str>) -> bool {
    shell_execute("open", path, args, working_directory).is_ok()
}

unsafe fn shell_execute(
    operation: &str,
    path: &str,
    args: Option<&str>,
    working_directory: Option<&str>,
) -> Result<(), isize> {
    let file = wide(path);
    let operation = wide(operation);
    let params = args.map(wide);
    let directory = working_directory
        .filter(|value| !value.is_empty())
        .map(wide);
    let result = ShellExecuteW(
        None,
        Some(&PCWSTR(operation.as_ptr())),
        PCWSTR(file.as_ptr()),
        params.as_deref().map(|v| PCWSTR(v.as_ptr())).as_ref(),
        directory
            .as_deref()
            .map(|value| PCWSTR(value.as_ptr()))
            .as_ref(),
        SW_SHOWNORMAL,
    );
    // HINSTANCE values > 32 mean success; lower values are Shell error codes.
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(code)
    }
}

/// Handler-backed shell verbs (taskbar pinning, Properties) only run through
/// ShellExecuteEx with SEE_MASK_INVOKEIDLIST; `shell_execute` above uses
/// plain ShellExecuteW, which resolves static verbs only.
unsafe fn shell_execute_verb(verb: &str, path: &Path) -> Result<(), isize> {
    let _com = ComGuard::init();
    let operation = wide(verb);
    let file = wide(&path.to_string_lossy());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC,
        hwnd: HWND::default(),
        lpVerb: PCWSTR(operation.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // On failure the call returns FALSE and hInstApp carries the SE_ERR_ code.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|_| info.hInstApp.0 as isize)
}

fn explorer_arg(arg: &str) -> Result<(), String> {
    Command::new("explorer.exe")
        .arg(arg)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch: {e}"))
}
