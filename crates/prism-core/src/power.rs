use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use std::mem::size_of;

use windows::core::GUID;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, SetLastError, ERROR_MORE_DATA, ERROR_NOT_ALL_ASSIGNED,
    ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HANDLE, HLOCAL, LUID, WIN32_ERROR,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Power::{
    PowerEnumerate, PowerGetActiveScheme, PowerReadFriendlyName, SetSuspendState, ACCESS_SCHEME,
};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerPlan {
    pub guid: String,
    pub name: String,
    pub active: bool,
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

pub fn list_power_plans() -> Result<Vec<PowerPlan>, String> {
    let executable = powercfg_executable()?;
    let output = Command::new(executable)
        .arg("/list")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("list power plans: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "list power plans: powercfg exited with status {}",
            output.status
        ));
    }
    let plans = parse_power_plans(&output.stdout);
    if plans.is_empty() {
        Err("powercfg returned no power plans".to_string())
    } else {
        Ok(plans)
    }
}

/// The installed plans from the power API, in the same form as
/// [`list_power_plans`]: one child process less per query. The names are the
/// ones `powercfg /list` prints (custom names as typed, built-in names in the
/// display language).
fn read_power_plans() -> Result<Vec<PowerPlan>, String> {
    let active = active_scheme()?;
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    // A system has a handful of plans: the bound only stops a broken API.
    for index in 0..1024 {
        let mut scheme = GUID::zeroed();
        let mut size = size_of::<GUID>() as u32;
        let status = unsafe {
            PowerEnumerate(
                None,
                None,
                None,
                ACCESS_SCHEME,
                index,
                Some((&raw mut scheme).cast()),
                &mut size,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        check(status, "enumerate power plans")?;
        let guid = format_guid(&scheme);
        if !seen.insert(guid.clone()) {
            continue;
        }
        let name = friendly_name(&scheme)?
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| known_power_plan_name(&guid).to_string());
        plans.push(PowerPlan { guid, name, active: scheme == active });
    }
    Ok(plans)
}

fn check(status: WIN32_ERROR, what: &str) -> Result<(), String> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: {}", windows::core::Error::from(status.to_hresult())))
    }
}

fn active_scheme() -> Result<GUID, String> {
    let mut pointer: *mut GUID = std::ptr::null_mut();
    check(unsafe { PowerGetActiveScheme(None, &mut pointer) }, "read the active power plan")?;
    if pointer.is_null() {
        return Err("read the active power plan: no plan".to_string());
    }
    // The API allocates the GUID; LocalFree releases it.
    let active = unsafe { pointer.read() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(pointer.cast())));
    }
    Ok(active)
}

/// The plan's display name, or `None` when the plan has none.
fn friendly_name(scheme: &GUID) -> Result<Option<String>, String> {
    let mut size = 0u32;
    let status = unsafe { PowerReadFriendlyName(None, Some(scheme), None, None, None, &mut size) };
    if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
        check(status, "read a power plan name")?;
    }
    if size == 0 {
        return Ok(None);
    }
    // The size is in bytes of UTF-16 text with its terminating NUL.
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let mut bytes = (buffer.len() * 2) as u32;
    check(
        unsafe {
            PowerReadFriendlyName(
                None,
                Some(scheme),
                None,
                None,
                Some(buffer.as_mut_ptr().cast()),
                &mut bytes,
            )
        },
        "read a power plan name",
    )?;
    let text = &buffer[..(bytes as usize / 2).min(buffer.len())];
    let end = text.iter().position(|&unit| unit == 0).unwrap_or(text.len());
    Ok(Some(String::from_utf16_lossy(&text[..end])))
}

/// The lower-case text form that `powercfg` prints and [`normalize_guid`] gives.
fn format_guid(guid: &GUID) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}

pub fn set_active_power_plan(guid: &str) -> Result<(), String> {
    let normalized = ensure_in_catalog(guid, &list_power_plans()?)?;
    let executable = powercfg_executable()?;
    let status = Command::new(executable)
        .arg("/setactive")
        .arg(&normalized)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|error| format!("set power plan: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Windows rejected the power plan change (exit status {status})"
        ))
    }
}

fn ensure_in_catalog(guid: &str, plans: &[PowerPlan]) -> Result<String, String> {
    let normalized = normalize_guid(guid).ok_or_else(|| "invalid power plan GUID".to_string())?;
    if plans.iter().any(|plan| plan.guid == normalized) {
        Ok(normalized)
    } else {
        Err("power plan is not installed".to_string())
    }
}

fn parse_power_plans(output: &[u8]) -> Vec<PowerPlan> {
    let text = String::from_utf8_lossy(output);
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for line in text.lines() {
        let Some(guid) = line.split_whitespace().find_map(normalize_guid) else {
            continue;
        };
        if !seen.insert(guid.clone()) {
            continue;
        }
        let start = line.to_ascii_lowercase().find(&guid).unwrap_or(0);
        let after = &line[start + guid.len()..];
        let name = after
            .find('(')
            .and_then(|open| {
                after[open + 1..]
                    .find(')')
                    .map(|close| after[open + 1..open + 1 + close].to_string())
            })
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                let candidate = after.trim().trim_matches('*').trim();
                (!candidate.is_empty()).then(|| candidate.to_string())
            })
            .unwrap_or_else(|| known_power_plan_name(&guid).to_string());
        plans.push(PowerPlan {
            guid,
            name,
            active: after.contains('*') || line.trim_end().ends_with('*'),
        });
    }
    plans
}

fn normalize_guid(value: &str) -> Option<String> {
    let value =
        value.trim_matches(|character: char| matches!(character, '*' | ',' | ';' | '(' | ')'));
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != 5
        || parts[0].len() != 8
        || parts[1].len() != 4
        || parts[2].len() != 4
        || parts[3].len() != 4
        || parts[4].len() != 12
        || value.len() != 36
        || !value
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
    {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

fn known_power_plan_name(guid: &str) -> &str {
    match guid {
        "381b4222-f694-41f0-9685-ff5bb260df2e" => "Balanced",
        "a1841308-3541-4fab-bc81-f71556f20b4a" => "Power saver",
        "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c" => "High performance",
        "e9a42b02-d5df-448d-aa00-03f14749eb61" => "Ultimate performance",
        _ => "Power plan",
    }
}

fn powercfg_executable() -> Result<PathBuf, String> {
    let system_root = std::env::var_os("SystemRoot")
        .ok_or_else(|| "Windows SystemRoot is unavailable".to_string())?;
    let path = PathBuf::from(system_root)
        .join("System32")
        .join("powercfg.exe");
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "Windows power configuration tool is missing: {}",
            path.display()
        ))
    }
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerPlanHit {
    pub guid: String,
    pub name: String,
    pub active: bool,
    pub score: i32,
    pub picker_intent: bool,
}

/// Runs for each typed query: the power API answers in about 2 ms, the
/// `powercfg` process in about 20 ms. `powercfg` stays the fallback.
pub fn search(query: &str) -> Result<Vec<PowerPlanHit>, String> {
    let plans = match read_power_plans() {
        Ok(plans) if !plans.is_empty() => plans,
        Ok(_) => list_power_plans()?,
        Err(error) => {
            eprintln!("Prism could not read power plans from the power API: {error}");
            list_power_plans()?
        }
    };
    Ok(rank(query, &plans))
}

pub fn set_active(guid: &str) -> Result<(), String> {
    set_active_power_plan(guid)
}

fn rank(query: &str, plans: &[PowerPlan]) -> Vec<PowerPlanHit> {
    let catalog_query = is_power_plan_query(query);
    let mut ranked: Vec<(i32, &PowerPlan)> = plans
        .iter()
        .filter_map(|plan| {
            let name_score = crate::query::score::score_with_keywords(query, &plan.name, &[]);
            if catalog_query {
                Some((name_score.map_or(700, |score| score + 100), plan))
            } else {
                name_score.map(|score| (score, plan))
            }
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.active.cmp(&left.1.active))
            .then_with(|| left.1.name.cmp(&right.1.name))
    });
    ranked
        .into_iter()
        .map(|(score, plan)| PowerPlanHit {
            guid: plan.guid.clone(),
            name: plan.name.clone(),
            active: plan.active,
            score,
            picker_intent: catalog_query,
        })
        .collect()
}

fn is_power_plan_query(query: &str) -> bool {
    let mut compact = String::new();
    for character in query.chars() {
        if character.is_alphanumeric() {
            compact.extend(character.to_lowercase());
        }
    }
    matches!(
        compact.as_str(),
        "powerplan"
            | "powerplans"
            | "choosepowerplan"
            | "choosepowerplans"
            | "chooseapowerplan"
            | "chooseapowerplans"
            | "selectpowerplan"
            | "selectpowerplans"
            | "selectapowerplan"
            | "selectapowerplans"
            | "powerplanpicker"
    )
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
    fn the_power_api_lists_the_plans_that_powercfg_lists() {
        let mut native = read_power_plans().expect("the power API lists the plans");
        let mut command = list_power_plans().expect("powercfg lists the plans");
        native.sort_by(|left, right| left.guid.cmp(&right.guid));
        command.sort_by(|left, right| left.guid.cmp(&right.guid));
        assert_eq!(native, command);
        assert_eq!(native.iter().filter(|plan| plan.active).count(), 1);
    }

    #[test]
    fn a_guid_has_the_text_form_of_powercfg() {
        let balanced = GUID::from_u128(0x381b4222_f694_41f0_9685_ff5bb260df2e);
        let text = format_guid(&balanced);
        assert_eq!(text, "381b4222-f694-41f0-9685-ff5bb260df2e");
        assert_eq!(normalize_guid(&text.to_ascii_uppercase()), Some(text));
    }
}
