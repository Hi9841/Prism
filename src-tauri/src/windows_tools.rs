//! Native Windows tools exposed through the palette.
//!
//! The Start menu is backed by several shell providers, not just Start Menu
//! shortcuts. This module keeps the launch surface typed and native while
//! discovering Control Panel applets from the current system.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, HKEY,
    HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ,
    REG_VALUE_TYPE,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_SFObject, IEnumIDList, ILFree, IShellFolder, IShellItem, SHCreateItemFromParsingName,
    SHCreateItemWithParent, SHGetIDListFromObject, SHLoadIndirectString, ShellExecuteExW,
    ShellExecuteW, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN,
    SHCONTF_NONFOLDERS, SHELLEXECUTEINFOW, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

#[derive(Clone, Debug)]
pub struct WindowsTool {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub keywords: Vec<String>,
    pub icon_key: &'static str,
    pub icon: Option<String>,
    target: ToolTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ToolTarget {
    Shell {
        target: String,
        parameters: Option<String>,
    },
    ControlPanel(String),
    // Some Windows Tools parsing names are shell-only and cannot be passed
    // back to ShellExecuteW, so keep the absolute PIDL for invocation.
    ShellItem {
        pidl: Vec<u8>,
        parameter: String,
    },
}

struct CatalogState {
    loaded_at: Instant,
    tools: Arc<Vec<WindowsTool>>,
}

static CATALOG: OnceLock<RwLock<CatalogState>> = OnceLock::new();

fn catalog_slot() -> &'static RwLock<CatalogState> {
    CATALOG.get_or_init(|| {
        RwLock::new(CatalogState {
            loaded_at: Instant::now(),
            tools: Arc::new(Vec::new()),
        })
    })
}

/// Refreshes the native tool catalog. Discovery is intentionally explicit so a
/// later app refresh can pick up newly installed Control Panel applets.
pub fn refresh() -> Arc<Vec<WindowsTool>> {
    let tools = Arc::new(discover());
    if let Ok(mut slot) = catalog_slot().write() {
        *slot = CatalogState {
            loaded_at: Instant::now(),
            tools: tools.clone(),
        };
    }
    tools
}

pub fn warm() {
    let _ = refresh();
}

pub fn cached() -> Option<Arc<Vec<WindowsTool>>> {
    let slot = catalog_slot().read().ok()?;
    (!slot.tools.is_empty()).then(|| slot.tools.clone())
}

pub fn snapshot() -> Arc<Vec<WindowsTool>> {
    if let Some(tools) = cached() {
        if catalog_slot()
            .read()
            .ok()
            .is_some_and(|slot| slot.loaded_at.elapsed() < Duration::from_secs(5 * 60))
        {
            return tools;
        }
    }
    refresh()
}

pub fn execute(id: &str) -> Result<(), String> {
    let tools = snapshot();
    let tool = tools
        .iter()
        .find(|tool| tool.id == id)
        .ok_or_else(|| format!("unknown Windows tool '{id}'"))?;
    execute_target(&tool.target)
}

struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    fn init() -> Self {
        Self {
            initialized: unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok(),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}

fn discover() -> Vec<WindowsTool> {
    let _com = ComGuard::init();
    let mut tools = Vec::new();
    add_fallback_tools(&mut tools);
    add_control_panel_applets(&mut tools);
    add_shell_control_panel_items(&mut tools);

    tools
}

fn add_fallback_tools(tools: &mut Vec<WindowsTool>) {
    add_shell_tool(
        tools,
        "windows.control-panel",
        "Control Panel",
        "Windows tool",
        "control.exe",
        None,
        "control-panel",
        &["control panel", "classic settings", "system tools"],
    );
    add_control_tool(
        tools,
        "windows.administrative-tools",
        "Administrative Tools",
        "Windows tool",
        "Microsoft.AdministrativeTools",
        "control-panel",
        &["administrative tools", "system tools", "windows tools"],
    );
    add_control_tool(
        tools,
        "windows.tools-folder",
        "Windows Tools",
        "Windows tool",
        "Microsoft.WindowsTools",
        "control-panel",
        &["windows tools", "system tools", "administrative tools"],
    );
    add_control_tool(
        tools,
        "windows.credential-manager",
        "Credential Manager",
        "Windows tool",
        "Microsoft.CredentialManager",
        "control-panel",
        &["credentials", "passwords", "windows credentials"],
    );
    add_control_tool(
        tools,
        "windows.auto-play",
        "AutoPlay settings",
        "Windows tool",
        "Microsoft.AutoPlay",
        "control-panel",
        &["autoplay", "media", "automatic playback"],
    );
    add_control_tool(
        tools,
        "windows.default-programs",
        "Default Programs",
        "Windows tool",
        "Microsoft.DefaultPrograms",
        "control-panel",
        &["default programs", "file associations", "defaults"],
    );

    add_system_tool(
        tools,
        "windows.device-manager",
        "Device Manager",
        "Windows tool",
        "devmgmt.msc",
        "device",
        &[
            "device manager",
            "hardware",
            "drivers",
            "devices",
            "plug and play",
        ],
    );
    add_system_tool(
        tools,
        "windows.performance-options",
        "Performance options",
        "Windows tool",
        "SystemPropertiesPerformance.exe",
        "performance",
        &[
            "performance",
            "best performance",
            "visual effects",
            "animations",
            "speed",
            "adjust performance",
        ],
    );
    add_system_tool(
        tools,
        "windows.computer-management",
        "Computer Management",
        "Windows tool",
        "compmgmt.msc",
        "control-panel",
        &[
            "computer management",
            "system tools",
            "services",
            "storage",
            "users",
        ],
    );
    add_system_tool(
        tools,
        "windows.disk-management",
        "Disk Management",
        "Windows tool",
        "diskmgmt.msc",
        "storage",
        &["disk management", "volumes", "partitions", "storage"],
    );
    add_system_tool(
        tools,
        "windows.event-viewer",
        "Event Viewer",
        "Windows tool",
        "eventvwr.msc",
        "control-panel",
        &["event viewer", "event log", "logs", "system events"],
    );
    add_system_tool(
        tools,
        "windows.services",
        "Services",
        "Windows tool",
        "services.msc",
        "control-panel",
        &["services", "service manager", "background services"],
    );
    add_system_tool(
        tools,
        "windows.task-manager",
        "Task Manager",
        "Windows tool",
        "taskmgr.exe",
        "performance",
        &["task manager", "processes", "performance", "startup"],
    );
    add_system_tool(
        tools,
        "windows.advanced-system-properties",
        "Advanced system settings",
        "Windows tool",
        "SystemPropertiesAdvanced.exe",
        "device",
        &[
            "advanced system settings",
            "system properties",
            "performance",
            "startup",
            "hardware",
        ],
    );
    add_system_tool(
        tools,
        "windows.system-information",
        "System Information",
        "Windows tool",
        "msinfo32.exe",
        "device",
        &[
            "system information",
            "msinfo32",
            "hardware",
            "windows system",
        ],
    );
    add_system_tool(
        tools,
        "windows.registry-editor",
        "Registry Editor",
        "Windows tool",
        "regedit.exe",
        "control-panel",
        &["registry", "registry editor", "regedit"],
    );
    add_system_tool(
        tools,
        "windows.performance-monitor",
        "Performance Monitor",
        "Windows tool",
        "perfmon.exe",
        "performance",
        &[
            "performance monitor",
            "perfmon",
            "cpu",
            "memory",
            "performance",
        ],
    );
    add_system_tool(
        tools,
        "windows.resource-monitor",
        "Resource Monitor",
        "Windows tool",
        "resmon.exe",
        "performance",
        &[
            "resource monitor",
            "resmon",
            "cpu",
            "memory",
            "disk",
            "network",
        ],
    );
    add_system_tool(
        tools,
        "windows.memory-diagnostics",
        "Memory Diagnostics",
        "Windows tool",
        "mdsched.exe",
        "device",
        &["memory diagnostics", "windows memory diagnostic", "ram"],
    );
    add_system_tool(
        tools,
        "windows.optimize-drives",
        "Optimize Drives",
        "Windows tool",
        "dfrgui.exe",
        "storage",
        &["optimize drives", "defragment", "defrag", "trim", "ssd"],
    );
    add_system_tool(
        tools,
        "windows.disk-cleanup",
        "Disk Cleanup",
        "Windows tool",
        "cleanmgr.exe",
        "storage",
        &[
            "disk cleanup",
            "cleanmgr",
            "free disk space",
            "temporary files",
            "storage",
        ],
    );
    add_system_tool(
        tools,
        "windows.system-configuration",
        "System Configuration",
        "Windows tool",
        "msconfig.exe",
        "control-panel",
        &[
            "system configuration",
            "msconfig",
            "startup",
            "boot",
            "services",
        ],
    );
    add_system_tool(
        tools,
        "windows.system-restore",
        "System Restore",
        "Windows tool",
        "rstrui.exe",
        "update",
        &[
            "system restore",
            "restore point",
            "recovery",
            "rollback",
            "previous versions",
        ],
    );
    add_system_tool(
        tools,
        "windows.optional-features",
        "Windows Features",
        "Windows tool",
        "OptionalFeatures.exe",
        "control-panel",
        &[
            "windows features",
            "optional features",
            "features",
            "components",
            "turn windows features",
        ],
    );
    add_system_tool(
        tools,
        "windows.directx-diagnostics",
        "DirectX Diagnostic Tool",
        "Windows tool",
        "dxdiag.exe",
        "performance",
        &[
            "directx diagnostic",
            "dxdiag",
            "graphics",
            "display adapter",
            "diagnostics",
        ],
    );
    add_system_tool(
        tools,
        "windows.local-users-and-groups",
        "Local Users and Groups",
        "Windows tool",
        "lusrmgr.msc",
        "control-panel",
        &[
            "local users and groups",
            "users",
            "groups",
            "accounts",
            "user management",
        ],
    );
    add_system_tool(
        tools,
        "windows.local-security-policy",
        "Local Security Policy",
        "Windows tool",
        "secpol.msc",
        "control-panel",
        &[
            "local security policy",
            "security policy",
            "audit policy",
            "account policy",
        ],
    );
    add_system_tool(
        tools,
        "windows.task-scheduler",
        "Task Scheduler",
        "Windows tool",
        "taskschd.msc",
        "control-panel",
        &[
            "task scheduler",
            "scheduled tasks",
            "taskschd",
            "automations",
        ],
    );
    add_system_tool(
        tools,
        "windows.windows-firewall",
        "Windows Defender Firewall",
        "Windows tool",
        "wf.msc",
        "network",
        &[
            "windows defender firewall",
            "firewall",
            "wf.msc",
            "network security",
            "inbound rules",
        ],
    );
    add_system_tool(
        tools,
        "windows.management-console",
        "Microsoft Management Console",
        "Windows tool",
        "mmc.exe",
        "control-panel",
        &[
            "microsoft management console",
            "mmc",
            "management console",
            "snap-ins",
        ],
    );
    add_system_tool(
        tools,
        "windows.command-prompt",
        "Command Prompt",
        "Windows tool",
        "cmd.exe",
        "tool",
        &["command prompt", "cmd", "command line", "shell"],
    );
    add_system_tool(
        tools,
        "windows.powershell",
        "Windows PowerShell",
        "Windows tool",
        "powershell.exe",
        "tool",
        &["powershell", "windows powershell", "command line", "shell"],
    );
    add_system_tool(
        tools,
        "windows.powershell-ise",
        "Windows PowerShell ISE",
        "Windows tool",
        "powershell_ise.exe",
        "tool",
        &[
            "powershell ise",
            "powershell",
            "scripting",
            "integrated scripting environment",
        ],
    );
    add_system_tool(
        tools,
        "windows.remote-desktop",
        "Remote Desktop Connection",
        "Windows tool",
        "mstsc.exe",
        "network",
        &[
            "remote desktop",
            "remote desktop connection",
            "mstsc",
            "rdp",
        ],
    );
    add_system_tool(
        tools,
        "windows.character-map",
        "Character Map",
        "Windows tool",
        "charmap.exe",
        "tool",
        &["character map", "characters", "symbols", "fonts"],
    );
    add_system_tool(
        tools,
        "windows.steps-recorder",
        "Steps Recorder",
        "Windows tool",
        "psr.exe",
        "tool",
        &[
            "steps recorder",
            "problem steps recorder",
            "psr",
            "record steps",
        ],
    );
    add_system_tool(
        tools,
        "windows.recovery-drive",
        "Recovery Drive",
        "Windows tool",
        "RecoveryDrive.exe",
        "storage",
        &[
            "recovery drive",
            "recovery usb",
            "windows recovery",
            "recovery media",
        ],
    );
    add_system_tool(
        tools,
        "windows.media-player-legacy",
        "Windows Media Player Legacy",
        "Windows tool",
        "wmplayer.exe",
        "sound",
        &[
            "windows media player",
            "media player",
            "legacy media player",
            "wmplayer",
        ],
    );

    add_control_tool(
        tools,
        "windows.hardware-wizard",
        "Add Hardware",
        "Windows tool",
        "hdwwiz.cpl",
        "device",
        &[
            "add hardware",
            "hardware wizard",
            "hdwwiz",
            "hardware and sound",
        ],
    );
    add_control_tool(
        tools,
        "windows.programs-and-features",
        "Programs and Features",
        "Windows tool",
        "appwiz.cpl",
        "control-panel",
        &[
            "programs and features",
            "installed programs",
            "uninstall",
            "appwiz",
        ],
    );
    add_control_tool(
        tools,
        "windows.classic-firewall",
        "Windows Defender Firewall",
        "Windows tool",
        "firewall.cpl",
        "network",
        &[
            "windows firewall",
            "firewall",
            "firewall properties",
            "network security",
        ],
    );
    add_system_tool(
        tools,
        "windows.odbc-data-sources",
        "ODBC Data Sources",
        "Windows tool",
        "odbcad32.exe",
        "control-panel",
        &[
            "odbc data sources",
            "odbc",
            "database connections",
            "drivers",
        ],
    );
    add_control_tool(
        tools,
        "windows.power-options",
        "Power Options",
        "Windows tool",
        "powercfg.cpl",
        "power-plan",
        &[
            "power options",
            "power plans",
            "power plan",
            "energy",
            "battery",
        ],
    );
    add_control_tool(
        tools,
        "windows.system-properties",
        "System Properties",
        "Windows tool",
        "sysdm.cpl",
        "device",
        &[
            "system properties",
            "about this computer",
            "hardware",
            "computer name",
        ],
    );
    add_control_tool(
        tools,
        "windows.display-properties",
        "Display properties",
        "Windows tool",
        "desk.cpl",
        "display",
        &["display properties", "screen", "resolution", "monitor"],
    );
    add_control_tool(
        tools,
        "windows.internet-options",
        "Internet Options",
        "Windows tool",
        "inetcpl.cpl",
        "network",
        &[
            "internet options",
            "network settings",
            "connections",
            "lan settings",
        ],
    );
    add_control_tool(
        tools,
        "windows.region",
        "Region",
        "Windows tool",
        "intl.cpl",
        "control-panel",
        &["region", "language", "locale", "formats"],
    );
    add_control_tool(
        tools,
        "windows.date-time",
        "Date and Time",
        "Windows tool",
        "timedate.cpl",
        "control-panel",
        &["date and time", "clock", "time zone", "time settings"],
    );
    add_control_tool(
        tools,
        "windows.user-accounts",
        "User Accounts",
        "Windows tool",
        "netplwiz.cpl",
        "control-panel",
        &["user accounts", "passwords", "account settings", "family"],
    );
    add_control_tool(
        tools,
        "windows.mouse-keyboard",
        "Mouse and keyboard properties",
        "Windows tool",
        "main.cpl",
        "device",
        &["mouse properties", "keyboard properties", "pointer", "keys"],
    );
    add_control_tool(
        tools,
        "windows.sound-properties",
        "Sound properties",
        "Windows tool",
        "mmsys.cpl",
        "sound",
        &["sound properties", "speakers", "microphone", "audio"],
    );
}

fn add_control_tool(
    tools: &mut Vec<WindowsTool>,
    id: &str,
    title: &str,
    subtitle: &str,
    applet: &str,
    icon_key: &'static str,
    keywords: &[&str],
) {
    let extension = Path::new(applet)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "cpl" | "dll" | "msc" | "exe")
        && !system32_path(applet).is_file()
    {
        return;
    }
    add_tool(
        tools,
        WindowsTool {
            id: id.to_string(),
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            keywords: keywords
                .iter()
                .map(|keyword| (*keyword).to_string())
                .collect(),
            icon_key,
            icon: None,
            target: ToolTarget::ControlPanel(applet.to_string()),
        },
    );
}

fn add_system_tool(
    tools: &mut Vec<WindowsTool>,
    id: &str,
    title: &str,
    subtitle: &str,
    file: &str,
    icon_key: &'static str,
    keywords: &[&str],
) {
    let path = system32_path(file);
    if !path.is_file() {
        return;
    }
    add_shell_tool_with_keywords(tools, id, title, subtitle, &path, None, icon_key, keywords);
}

#[allow(clippy::too_many_arguments)]
fn add_shell_tool(
    tools: &mut Vec<WindowsTool>,
    id: &str,
    title: &str,
    subtitle: &str,
    file: &str,
    parameters: Option<&str>,
    icon_key: &'static str,
    keywords: &[&str],
) {
    let path = system32_path(file);
    if !path.is_file() {
        return;
    }
    add_shell_tool_with_keywords(
        tools, id, title, subtitle, &path, parameters, icon_key, keywords,
    );
}

#[allow(clippy::too_many_arguments)]
fn add_shell_tool_with_keywords(
    tools: &mut Vec<WindowsTool>,
    id: &str,
    title: &str,
    subtitle: &str,
    path: &Path,
    parameters: Option<&str>,
    icon_key: &'static str,
    keywords: &[&str],
) {
    add_tool(
        tools,
        WindowsTool {
            id: id.to_string(),
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            keywords: keywords
                .iter()
                .map(|keyword| (*keyword).to_string())
                .collect(),
            icon_key,
            icon: None,
            target: ToolTarget::Shell {
                target: path.to_string_lossy().into_owned(),
                parameters: parameters.map(str::to_string),
            },
        },
    );
}

fn add_shell_control_panel_items(tools: &mut Vec<WindowsTool>) {
    unsafe {
        let root = wide("shell:::{21EC2020-3AEA-1069-A2DD-08002B30309D}");
        let root_item: IShellItem = match SHCreateItemFromParsingName(PCWSTR(root.as_ptr()), None) {
            Ok(item) => item,
            Err(_) => return,
        };
        let Ok(folder) = root_item.BindToHandler::<_, IShellFolder>(None, &BHID_SFObject) else {
            return;
        };
        enumerate_shell_items(tools, &folder, true);
    }
}

unsafe fn enumerate_shell_items(
    tools: &mut Vec<WindowsTool>,
    folder: &IShellFolder,
    recurse_folders: bool,
) {
    let mut enumerator: Option<IEnumIDList> = None;
    let flags =
        SHCONTF_FOLDERS.0 as u32 | SHCONTF_NONFOLDERS.0 as u32 | SHCONTF_INCLUDEHIDDEN.0 as u32;
    if !folder
        .EnumObjects(HWND(std::ptr::null_mut()), flags, &mut enumerator)
        .is_ok()
    {
        return;
    }
    let Some(enumerator) = enumerator else {
        return;
    };
    let Ok(parent_pidl) = SHGetIDListFromObject(folder) else {
        return;
    };
    loop {
        let mut child_pidls: [*mut ITEMIDLIST; 1] = [std::ptr::null_mut()];
        let result = enumerator.Next(&mut child_pidls, None);
        let child_pidl = child_pidls[0];
        if !result.is_ok() {
            if !child_pidl.is_null() {
                ILFree(Some(child_pidl));
            }
            break;
        }
        if child_pidl.is_null() {
            break;
        }
        if let Some(item) = shell_item_with_parent(folder, parent_pidl, child_pidl) {
            if let Some(tool) = shell_control_panel_item(&item) {
                let recurse = recurse_folders && is_windows_tools_folder(&tool);
                add_tool(tools, tool);
                if recurse {
                    if let Ok(nested) = item.BindToHandler::<_, IShellFolder>(None, &BHID_SFObject)
                    {
                        enumerate_shell_items(tools, &nested, false);
                    }
                }
            }
        }
        ILFree(Some(child_pidl));
    }
    ILFree(Some(parent_pidl));
}

unsafe fn shell_item_with_parent(
    folder: &IShellFolder,
    parent_pidl: *const ITEMIDLIST,
    child_pidl: *mut ITEMIDLIST,
) -> Option<IShellItem> {
    SHCreateItemWithParent(Some(parent_pidl), folder, child_pidl).ok()
}

fn is_windows_tools_folder(tool: &WindowsTool) -> bool {
    if tool.title.eq_ignore_ascii_case("Windows Tools")
        || tool.title.eq_ignore_ascii_case("Administrative Tools")
    {
        return true;
    }
    matches!(
        &tool.target,
        ToolTarget::ShellItem { parameter, .. }
            if parameter
                .to_ascii_lowercase()
                .contains("d20ea4e1-3957-11d2-a40b-0c5020524153")
    )
}

unsafe fn shell_control_panel_item(item: &IShellItem) -> Option<WindowsTool> {
    let title_pointer = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
    let title = title_pointer.to_string().unwrap_or_default();
    CoTaskMemFree(Some(title_pointer.as_ptr() as *const c_void));
    let title = title.trim().to_string();
    if title.is_empty() || title.starts_with("::{") {
        return None;
    }
    let parameter_pointer = item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING).ok()?;
    let parameter = parameter_pointer.to_string().unwrap_or_default();
    CoTaskMemFree(Some(parameter_pointer.as_ptr() as *const c_void));
    let parameter = parameter.trim().to_string();
    let parameter = if parameter.is_empty() {
        format!("title:{}", slug(&title))
    } else {
        parameter
    };
    let pidl_pointer = SHGetIDListFromObject(item).ok()?;
    let pidl = copy_pidl(pidl_pointer);
    ILFree(Some(pidl_pointer));
    let icon = crate::apps::shell_item_icon_data_url(item);
    Some(WindowsTool {
        id: format!("control-panel::shell-{}", slug(&title)),
        title: title.clone(),
        subtitle: "Control Panel".to_string(),
        keywords: vec![
            "control panel".to_string(),
            title,
            parameter.clone(),
            "windows tool".to_string(),
        ],
        icon_key: "control-panel",
        icon,
        target: ToolTarget::ShellItem {
            pidl: pidl?,
            parameter,
        },
    })
}

fn add_control_panel_applets(tools: &mut Vec<WindowsTool>) {
    // Built on the first registered applet only: most machines have none.
    let titles = OnceCell::new();
    for (root, path) in [
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Control Panel\Cpls",
        ),
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Control Panel\Cpls",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Control Panel\Cpls",
        ),
    ] {
        let Some(key) = (unsafe { open_registry_key(root, path) }) else {
            continue;
        };

        for index in 0..256u32 {
            let mut name_buffer = [0u16; 512];
            let mut name_length = name_buffer.len() as u32;
            let result = unsafe {
                RegEnumKeyExW(
                    key,
                    index,
                    Some(PWSTR(name_buffer.as_mut_ptr())),
                    &mut name_length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if result.0 != 0 {
                break;
            }
            let name = String::from_utf16_lossy(&name_buffer[..name_length as usize]);
            let default_value = unsafe { read_registry_string(key, "") };
            add_cpl_candidate(tools, &titles, &name, default_value.as_deref());
        }

        for index in 0..256u32 {
            let mut name_buffer = [0u16; 512];
            let mut name_length = name_buffer.len() as u32;
            let mut value_type = 0u32;
            let mut value_length = 0u32;
            let result = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    Some(PWSTR(name_buffer.as_mut_ptr())),
                    &mut name_length,
                    None,
                    Some(&mut value_type),
                    None,
                    Some(&mut value_length),
                )
            };
            if result.0 != 0 {
                break;
            }
            if value_type != REG_SZ.0 && value_type != REG_EXPAND_SZ.0 {
                continue;
            }
            let name = String::from_utf16_lossy(&name_buffer[..name_length as usize]);
            let value = unsafe { read_registry_string(key, &name) };
            if let Some(value) = value {
                add_cpl_candidate(tools, &titles, &name, Some(&value));
            }
        }
        unsafe {
            let _ = RegCloseKey(key);
        }
    }
}

fn add_cpl_candidate(
    tools: &mut Vec<WindowsTool>,
    titles: &OnceCell<HashMap<String, String>>,
    name: &str,
    value: Option<&str>,
) {
    let Some(applet) = normalize_cpl_target(name, value) else {
        return;
    };
    let title = cpl_title(&applet, titles.get_or_init(control_panel_titles));
    let mut keywords = vec![
        "control panel".to_string(),
        name.to_string(),
        applet.clone(),
        title.clone(),
    ];
    keywords.sort_by_key(|keyword| keyword.to_lowercase());
    keywords.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    let id = format!("control-panel::{}", slug(name));
    add_tool(
        tools,
        WindowsTool {
            id,
            title,
            subtitle: "Control Panel".to_string(),
            keywords,
            icon_key: "control-panel",
            icon: None,
            target: ToolTarget::ControlPanel(applet),
        },
    );
}

fn normalize_cpl_target(name: &str, value: Option<&str>) -> Option<String> {
    let raw = value.unwrap_or(name).trim().trim_matches('"');
    let raw = raw.strip_prefix('@').unwrap_or(raw);
    let (path, resource_id) = split_cpl_resource(raw);
    let path = path.trim().trim_matches('"');
    if path.is_empty() {
        return None;
    }
    let expanded = expand_environment(path);
    let candidate = if expanded.contains('\\') {
        PathBuf::from(expanded)
    } else {
        system32_path(&expanded)
    };
    let extension = candidate
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "cpl" | "dll" | "msc" | "exe")
        || !is_windows_system_path(&candidate)
    {
        return None;
    }
    let mut parameter = candidate.to_string_lossy().into_owned();
    if let Some(resource_id) = resource_id {
        parameter.push(',');
        parameter.push_str(resource_id);
    }
    Some(parameter)
}

fn is_windows_system_path(path: &Path) -> bool {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
        .to_string_lossy()
        .to_ascii_lowercase();
    let path = path.to_string_lossy().to_ascii_lowercase();
    [
        format!("{root}\\system32\\"),
        format!("{root}\\syswow64\\"),
        format!("{root}\\windows\\"),
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

fn split_cpl_resource(value: &str) -> (&str, Option<&str>) {
    value
        .rsplit_once(',')
        .filter(|(_, resource_id)| resource_id.parse::<i32>().is_ok())
        .map_or((value, None), |(path, resource_id)| {
            (path, Some(resource_id))
        })
}

fn cpl_path(parameter: &str) -> &str {
    split_cpl_resource(parameter).0
}

const CONTROL_PANEL_NAMESPACE: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ControlPanel\NameSpace";

/// The title Explorer shows for an applet, in the user's language. An
/// explicit string resource in the registry value (`desk.cpl,-1`) wins, then
/// the Control Panel namespace title for that `.cpl` file, then the file
/// stem.
fn cpl_title(applet: &str, titles: &HashMap<String, String>) -> String {
    let (path, resource) = split_cpl_resource(applet);
    resource
        .filter(|id| id.starts_with('-'))
        .and_then(|id| load_indirect_string(&format!("@{path},{id}")))
        .or_else(|| titles.get(&path.to_lowercase()).cloned())
        .unwrap_or_else(|| friendly_cpl_name(path))
}

/// Localized Control Panel titles keyed by lowercase `.cpl` path. Windows
/// registers each built-in applet as a namespace CLSID: its open command
/// names the `.cpl` file and applet index, and its `LocalizedString` is the
/// title Explorer shows. Only string resources are read - an applet's own
/// `CPlApplet` entry point can block (desk.cpl hangs in `CPL_INIT`), so it is
/// never called. A file with several applets keeps its lowest index, the one
/// that opens when the file itself is run.
fn control_panel_titles() -> HashMap<String, String> {
    let Some(namespace) =
        (unsafe { open_registry_key(HKEY_LOCAL_MACHINE, CONTROL_PANEL_NAMESPACE) })
    else {
        return HashMap::new();
    };
    let classes = unsafe { registry_subkeys(namespace) };
    unsafe {
        let _ = RegCloseKey(namespace);
    }

    let mut best: HashMap<String, (u32, String)> = HashMap::new();
    for clsid in classes {
        let class_path = format!(r"CLSID\{clsid}");
        let localized = unsafe { read_class_string(&class_path, "LocalizedString") };
        let Some(localized) = localized.map(|value| expand_environment(&value)) else {
            continue;
        };
        let command =
            unsafe { read_class_string(&format!(r"{class_path}\Shell\Open\Command"), "") };
        let Some((path, index)) = command
            .as_deref()
            .and_then(control_rundll_target)
            .or_else(|| indirect_cpl_target(&localized))
        else {
            continue;
        };
        let key = expand_environment(&path).to_lowercase();
        if best
            .get(&key)
            .is_some_and(|(existing, _)| *existing <= index)
        {
            continue;
        }
        if let Some(title) = load_indirect_string(&localized) {
            best.insert(key, (index, title));
        }
    }
    best.into_iter()
        .map(|(path, (_, title))| (path, title))
        .collect()
}

/// `.cpl` path and applet index from a namespace open command such as
/// `rundll32.exe shell32.dll,Control_RunDLL C:\Windows\System32\main.cpl,@1`.
fn control_rundll_target(command: &str) -> Option<(String, u32)> {
    let lower = command.to_ascii_lowercase();
    let start = lower.find("control_rundll")? + "control_rundll".len();
    let rest = command[start..].trim_start().trim_start_matches('"');
    let end = rest.to_ascii_lowercase().find(".cpl")? + ".cpl".len();
    let index = rest[end..]
        .trim_start_matches(['"', ',', ' '])
        .strip_prefix('@')
        .and_then(|value| {
            value
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or(0);
    Some((rest[..end].to_string(), index))
}

/// `.cpl` path from a `LocalizedString` such as `@C:\...\appwiz.cpl,-159`,
/// for namespace items registered without an open command.
fn indirect_cpl_target(localized: &str) -> Option<(String, u32)> {
    let (path, _) = localized.strip_prefix('@')?.rsplit_once(',')?;
    path.to_ascii_lowercase()
        .ends_with(".cpl")
        .then(|| (path.to_string(), 0))
}

/// Resolves an `@file,-id` string resource without running the file's code.
fn load_indirect_string(source: &str) -> Option<String> {
    let source = wide(source);
    let mut output = [0u16; 256];
    unsafe { SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut output, None) }.ok()?;
    let end = output
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(output.len());
    let value = String::from_utf16_lossy(&output[..end]).trim().to_string();
    (!value.is_empty() && !value.starts_with('@')).then_some(value)
}

unsafe fn read_class_string(path: &str, name: &str) -> Option<String> {
    let key = open_registry_key(HKEY_CLASSES_ROOT, path)?;
    let value = read_registry_string(key, name);
    let _ = RegCloseKey(key);
    value
}

unsafe fn registry_subkeys(key: HKEY) -> Vec<String> {
    let mut names = Vec::new();
    for index in 0..1024u32 {
        let mut name_buffer = [0u16; 256];
        let mut name_length = name_buffer.len() as u32;
        let result = RegEnumKeyExW(
            key,
            index,
            Some(PWSTR(name_buffer.as_mut_ptr())),
            &mut name_length,
            None,
            None,
            None,
            None,
        );
        if result.0 != 0 {
            break;
        }
        names.push(String::from_utf16_lossy(
            &name_buffer[..name_length as usize],
        ));
    }
    names
}

fn friendly_cpl_name(path: &str) -> String {
    let file = Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path);
    let stem = file
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file)
        .replace(['_', '-'], " ");
    let name = stem
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let name = if name.is_empty() {
        "Control Panel applet".to_string()
    } else {
        name
    };
    let suffix = if Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("cpl"))
    {
        ""
    } else {
        " applet"
    };
    format!("{name}{suffix}")
}

fn expand_environment(value: &str) -> String {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    let root = root.to_string_lossy();
    value
        .replace("%SystemRoot%", root.as_ref())
        .replace("%systemroot%", root.as_ref())
        .replace("%windir%", root.as_ref())
        .replace("%Windir%", root.as_ref())
}

fn slug(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
        } else if !output.ends_with('-') {
            output.push('-');
        }
    }
    let slug = output.trim_matches('-').to_string();
    if slug.is_empty() {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in value.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("item-{hash:016x}")
    } else {
        slug
    }
}

fn target_key(target: &ToolTarget) -> String {
    match target {
        ToolTarget::Shell { target, parameters } => format!(
            "shell:{}:{}",
            target.to_ascii_lowercase(),
            parameters
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
        ),
        ToolTarget::ControlPanel(parameter) => {
            let path = cpl_path(parameter);
            let file = Path::new(path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(path);
            format!(
                "control:{}:{}",
                file.to_ascii_lowercase(),
                split_cpl_resource(parameter).1.unwrap_or_default()
            )
        }
        ToolTarget::ShellItem { parameter, .. } => {
            format!("shell-item:{}", parameter.to_ascii_lowercase())
        }
    }
}

fn target_rank(target: &ToolTarget) -> u8 {
    match target {
        ToolTarget::ShellItem { .. } => 4,
        ToolTarget::Shell { .. } => 2,
        ToolTarget::ControlPanel(_) => 1,
    }
}

fn add_tool(tools: &mut Vec<WindowsTool>, tool: WindowsTool) {
    let new_target_rank = target_rank(&tool.target);
    if let Some(existing) = tools.iter_mut().find(|existing| {
        target_key(&existing.target) == target_key(&tool.target)
            || existing.title.eq_ignore_ascii_case(&tool.title)
    }) {
        for keyword in tool.keywords {
            if !existing
                .keywords
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&keyword))
            {
                existing.keywords.push(keyword);
            }
        }
        if let Some(icon) = tool.icon.clone() {
            existing.icon = Some(icon);
        }
        if new_target_rank > target_rank(&existing.target) {
            existing.target = tool.target;
        }
        return;
    }
    tools.push(tool);
}

fn execute_target(target: &ToolTarget) -> Result<(), String> {
    match target {
        ToolTarget::Shell { target, parameters } => shell_execute(target, parameters.as_deref()),
        ToolTarget::ControlPanel(applet) => {
            let parameter = control_panel_parameter(applet);
            shell_execute(
                &system32_path("control.exe").to_string_lossy(),
                Some(&parameter),
            )
        }
        ToolTarget::ShellItem { pidl, .. } => shell_execute_pidl(pidl),
    }
}

fn control_panel_parameter(applet: &str) -> String {
    let extension = Path::new(applet)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "cpl" | "dll" | "msc" | "exe") {
        applet.to_string()
    } else {
        format!("/name {applet}")
    }
}

const MAX_PIDL_BYTES: usize = 64 * 1024;

unsafe fn copy_pidl(pidl: *const ITEMIDLIST) -> Option<Vec<u8>> {
    if pidl.is_null() {
        return None;
    }
    let base = pidl.cast::<u8>();
    let mut length = 0usize;
    loop {
        if length + 2 > MAX_PIDL_BYTES {
            return None;
        }
        let size = std::ptr::read_unaligned(base.add(length).cast::<u16>()) as usize;
        if size == 0 {
            length += 2;
            break;
        }
        if size < 2 || length + size > MAX_PIDL_BYTES {
            return None;
        }
        length += size;
    }
    let mut bytes = vec![0u8; length];
    std::ptr::copy_nonoverlapping(base, bytes.as_mut_ptr(), length);
    Some(bytes)
}

fn shell_execute_pidl(pidl: &[u8]) -> Result<(), String> {
    if pidl.len() < 2 || pidl.len() > MAX_PIDL_BYTES || !pidl.len().is_multiple_of(2) {
        return Err("invalid Windows shell item".to_string());
    }
    let _com = ComGuard::init();
    let operation = wide("open");
    let mut storage = vec![0u16; pidl.len().div_ceil(2)];
    unsafe {
        std::ptr::copy_nonoverlapping(pidl.as_ptr(), storage.as_mut_ptr().cast::<u8>(), pidl.len());
    }
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC,
        hwnd: HWND::default(),
        lpVerb: PCWSTR(operation.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        lpIDList: storage.as_mut_ptr().cast::<c_void>(),
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }
        .map_err(|error| format!("Windows could not open shell item ({error})"))
}

fn shell_execute(target: &str, parameters: Option<&str>) -> Result<(), String> {
    let operation = wide("open");
    let target_text = target;
    let target = wide(target_text);
    let parameters = parameters.map(wide);
    let result = unsafe {
        ShellExecuteW(
            None,
            Some(&PCWSTR(operation.as_ptr())),
            PCWSTR(target.as_ptr()),
            parameters
                .as_deref()
                .map(|value| PCWSTR(value.as_ptr()))
                .as_ref(),
            None,
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not open {target_text} (Shell error {code})"
        ))
    }
}

fn system32_path(file: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join(file)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn open_registry_key(root: HKEY, path: &str) -> Option<HKEY> {
    let wide_path = wide(path);
    let mut key = HKEY(std::ptr::null_mut());
    let result = RegOpenKeyExW(root, PCWSTR(wide_path.as_ptr()), None, KEY_READ, &mut key);
    (result.0 == 0 && !key.0.is_null()).then_some(key)
}

unsafe fn read_registry_string(key: HKEY, name: &str) -> Option<String> {
    let wide_name = wide(name);
    let mut value_type = REG_VALUE_TYPE(0);
    let mut length = 0u32;
    let result = RegQueryValueExW(
        key,
        PCWSTR(wide_name.as_ptr()),
        None,
        Some(&mut value_type),
        None,
        Some(&mut length),
    );
    if result.0 != 0 || length == 0 {
        return None;
    }
    let mut buffer = vec![0u8; length as usize];
    let result = RegQueryValueExW(
        key,
        PCWSTR(wide_name.as_ptr()),
        None,
        Some(&mut value_type),
        Some(buffer.as_mut_ptr()),
        Some(&mut length),
    );
    if result.0 != 0 || (value_type.0 != REG_SZ.0 && value_type.0 != REG_EXPAND_SZ.0) {
        return None;
    }
    let units = buffer
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| u16::from_le_bytes(*chunk))
        .collect::<Vec<_>>();
    let end = units
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(units.len());
    Some(String::from_utf16_lossy(&units[..end]).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_commands_name_the_cpl_file_and_applet() {
        assert_eq!(
            control_rundll_target(
                r"C:\WINDOWS\System32\rundll32.exe C:\WINDOWS\System32\shell32.dll,Control_RunDLL C:\WINDOWS\System32\main.cpl,@1"
            ),
            Some((r"C:\WINDOWS\System32\main.cpl".to_string(), 1))
        );
        assert_eq!(
            control_rundll_target(
                r#"rundll32.exe shell32.dll,Control_RunDLL "C:\WINDOWS\System32\tabletpc.cpl" @0"#
            ),
            Some((r"C:\WINDOWS\System32\tabletpc.cpl".to_string(), 0))
        );
        assert_eq!(
            control_rundll_target(r"rundll32.exe shell32.dll,Control_RunDLL C:\x\mmsys.cpl"),
            Some((r"C:\x\mmsys.cpl".to_string(), 0))
        );
        assert_eq!(control_rundll_target(r"C:\Windows\explorer.exe"), None);
        assert_eq!(
            indirect_cpl_target(r"@C:\WINDOWS\system32\appwiz.cpl,-159#immutable1"),
            Some((r"C:\WINDOWS\system32\appwiz.cpl".to_string(), 0))
        );
        assert_eq!(indirect_cpl_target(r"@C:\x\shell32.dll,-22072"), None);
    }

    #[test]
    fn built_in_applets_get_their_localized_titles() {
        // Locale-independent: the title must come from the system's string
        // resources, never the file-stem fallback ("Main", "Mmsys").
        let titles = control_panel_titles();
        for file in ["main.cpl", "mmsys.cpl", "appwiz.cpl"] {
            let applet = system32_path(file).to_string_lossy().into_owned();
            let title = cpl_title(&applet, &titles);
            assert!(!title.is_empty(), "{file}");
            assert_ne!(
                title,
                friendly_cpl_name(&applet),
                "{file} fell back to the file stem"
            );
        }
    }

    #[test]
    fn cpl_targets_accept_registry_names_and_indirect_values() {
        let powercfg = system32_path("powercfg.cpl").to_string_lossy().into_owned();
        let desk = format!("{},-1", system32_path("desk.cpl").to_string_lossy());
        assert_eq!(
            normalize_cpl_target("powercfg.cpl", None).as_deref(),
            Some(powercfg.as_str())
        );
        assert_eq!(
            normalize_cpl_target("ignored", Some("@%SystemRoot%\\System32\\desk.cpl,-1"))
                .as_deref(),
            Some(desk.as_str())
        );
        assert!(normalize_cpl_target("not-a-cpl", None).is_none());
        assert!(normalize_cpl_target("ignored", Some(r"C:\Users\Public\evil.cpl")).is_none());
    }

    #[test]
    fn copies_shell_pidl_bytes_without_retaining_shell_memory() {
        let words = [2u16, 0];
        let copied = unsafe { copy_pidl(words.as_ptr().cast()) }.expect("valid test PIDL");
        assert_eq!(copied, vec![2, 0, 0, 0]);
    }

    #[test]
    fn canonical_control_panel_names_use_control_name_syntax() {
        assert_eq!(
            control_panel_parameter("Microsoft.AdministrativeTools"),
            "/name Microsoft.AdministrativeTools"
        );
        assert_eq!(control_panel_parameter("powercfg.cpl"), "powercfg.cpl");
    }

    #[test]
    fn canonical_control_panel_names_are_not_treated_as_files() {
        let mut tools = Vec::new();
        add_control_tool(
            &mut tools,
            "windows.administrative-tools",
            "Administrative Tools",
            "Windows tool",
            "Microsoft.AdministrativeTools",
            "control-panel",
            &["administrative tools"],
        );
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].target,
            ToolTarget::ControlPanel("Microsoft.AdministrativeTools".to_string())
        );
    }

    #[test]
    fn slugs_are_stable_and_safe_for_action_ids() {
        assert_eq!(slug("Desk.CPL"), "desk-cpl");
        assert_eq!(slug("Power Options"), "power-options");
    }
}
