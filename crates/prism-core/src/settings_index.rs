//! The Windows settings search index.
//!
//! Start menu search does not only find apps and Control Panel items. It also
//! reads `%SystemRoot%\ImmersiveControlPanel\Settings\AllSystemSettings_*.xml`:
//! every Settings page and every Control Panel task, such as "Change User
//! Account Control settings", with its localized title, its search keywords,
//! its launch target, and the condition that shows or hides it on this PC.
//! Prism reads the same files, so a setting that the Start menu finds is found
//! here too.
//!
//! The files change only with Windows updates, so the index is built once per
//! process, off the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::NetworkManagement::NetManagement::{
    NetApiBufferFree, NetGetJoinInformation, NetSetupDomainName, NETSETUP_JOIN_STATUS,
};
use windows::Win32::System::Power::{
    PlatformRoleMobile, PlatformRoleSlate, PowerDeterminePlatformRoleEx, POWER_PLATFORM_ROLE_V2,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegQueryValueExW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    REG_DWORD, REG_VALUE_TYPE,
};
use windows::Win32::UI::Shell::IsUserAnAdmin;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_DIGITIZER, SM_TABLETPC};

use crate::windows_tools::{
    expand_environment, is_windows_system_path, load_indirect_string, open_registry_key,
    read_registry_string, shell_execute, slug, system32_path,
};

/// The Windows Security host. Its deep links are page names for the
/// `windowsdefender:` protocol, not command lines.
const WINDOWS_SECURITY_HOST: &str = "{6E6DDBCB-9C89-434B-A994-D5F22239523B}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    /// A Settings app page or a Windows Security page.
    Page,
    /// A classic Control Panel task.
    Task,
}

#[derive(Clone, Debug)]
pub struct SettingEntry {
    pub id: String,
    pub title: String,
    pub subtitle: &'static str,
    pub keywords: Vec<String>,
    pub kind: SettingKind,
    pub icon_key: &'static str,
    target: Target,
}

impl SettingEntry {
    /// The URI that opens a page, for display and copy. Tasks have none.
    pub fn uri(&self) -> Option<&str> {
        match (&self.target, self.kind) {
            (Target::Uri(uri), SettingKind::Page) => Some(uri),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Uri(String),
    Command {
        file: String,
        parameters: Option<String>,
    },
}

static INDEX: OnceLock<Arc<Vec<SettingEntry>>> = OnceLock::new();

pub fn warm() {
    let _ = entries();
}

/// The index, or `None` while it is still being built.
pub fn cached() -> Option<Arc<Vec<SettingEntry>>> {
    INDEX.get().cloned()
}

/// Opens the setting with this id. `None` when the id is not a setting.
pub fn execute(id: &str) -> Option<Result<(), String>> {
    if !id.starts_with(ID_PREFIX) {
        return None;
    }
    let entries = entries();
    let Some(entry) = entries.iter().find(|entry| entry.id == id) else {
        return Some(Err(format!("unknown Windows setting '{id}'")));
    };
    Some(match &entry.target {
        Target::Uri(uri) => crate::windows_settings::open(uri),
        Target::Command { file, parameters } => shell_execute(file, parameters.as_deref()),
    })
}

const ID_PREFIX: &str = "settings-index::";

fn entries() -> Arc<Vec<SettingEntry>> {
    INDEX.get_or_init(|| Arc::new(load())).clone()
}

fn load() -> Vec<SettingEntry> {
    let sources = index_files()
        .into_iter()
        .filter_map(|path| read_xml(&path))
        .collect::<Vec<_>>();
    // The files are near copies for different Windows editions. Only some
    // carry the `ms-settings:` names of Settings pages, so a record is taken
    // from the first file that gives it a launch target.
    let mut records: Vec<Record> = Vec::new();
    let mut seen = HashMap::new();
    for source in &sources {
        for record in parse(source) {
            match seen.get(&record.filename) {
                Some(&index) => {
                    let existing: &mut Record = &mut records[index];
                    if existing.policy_id.is_none() && record.policy_id.is_some() {
                        *existing = record;
                    }
                }
                None => {
                    seen.insert(record.filename.clone(), records.len());
                    records.push(record);
                }
            }
        }
    }
    build(records, &mut Conditions::default())
}

fn index_files() -> Vec<PathBuf> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let folder = PathBuf::from(root).join(r"ImmersiveControlPanel\Settings");
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    let name = name.to_ascii_lowercase();
                    name.starts_with("allsystemsettings_") && name.ends_with(".xml")
                })
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn read_xml(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let text = match bytes.as_slice() {
        [0xFF, 0xFE, rest @ ..] => String::from_utf16_lossy(
            &rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect::<Vec<_>>(),
        ),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        rest => String::from_utf8_lossy(rest).into_owned(),
    };
    Some(text)
}

/// One `<SearchableContent>` element, before its strings are resolved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Record {
    filename: String,
    include_feature: Option<String>,
    exclude_feature: Option<String>,
    description: Option<String>,
    high_keywords: Option<String>,
    deep_link: Option<String>,
    host: Option<String>,
    policy_id: Option<String>,
    conditions: Vec<String>,
}

fn parse(xml: &str) -> Vec<Record> {
    let mut records = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<SearchableContent") {
        let block = &rest[start..];
        let Some(end) = block.find("</SearchableContent>") else {
            break;
        };
        let block = &block[..end];
        rest = &rest[start + end..];
        let head = &block[..block.find('>').unwrap_or(block.len())];
        let Some(filename) = element(block, "Filename") else {
            continue;
        };
        records.push(Record {
            filename,
            include_feature: attribute(head, "IncludeWithFeature"),
            exclude_feature: attribute(head, "ExcludeWithFeature"),
            description: element(block, "Description"),
            high_keywords: element(block, "HighKeywords"),
            deep_link: element(block, "DeepLink"),
            host: element(block, "HostID"),
            policy_id: element(block, "PolicyIds").and_then(|ids| {
                page_name(
                    element(block, "PageID").as_deref().unwrap_or_default(),
                    &ids,
                )
            }),
            conditions: elements(block, "Condition"),
        });
    }
    records
}

/// The `ms-settings:` name to open a page with. `PolicyIds` lists every name
/// that opens the page, sorted alphabetically. The name that the page id ends
/// with (`regionlanguage` for `SettingsPageTimeRegionLanguage`) is the page
/// itself; other names can open a section of it.
fn page_name(page_id: &str, ids: &str) -> Option<String> {
    let page_id = page_id.to_ascii_lowercase();
    let ids = ids
        .split(';')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    ids.iter()
        .filter(|id| page_id.ends_with(&id.replace('-', "").to_ascii_lowercase()))
        .max_by_key(|id| id.len())
        .or_else(|| ids.first())
        .map(|id| id.to_string())
}

fn element(block: &str, name: &str) -> Option<String> {
    elements(block, name).into_iter().next()
}

fn elements(block: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut values = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        let value = decode_entities(after[..end].trim());
        if !value.is_empty() {
            values.push(value);
        }
        rest = &after[end + close.len()..];
    }
    values
}

fn attribute(head: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=\"");
    let start = head.find(&marker)? + marker.len();
    let end = head[start..].find('"')? + start;
    Some(decode_entities(&head[start..end])).filter(|value| !value.is_empty())
}

fn decode_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Turns records into searchable entries. Records whose condition fails on
/// this PC, or that have no launch target, are left out, as the Start menu
/// leaves them out.
fn build(records: Vec<Record>, conditions: &mut Conditions) -> Vec<SettingEntry> {
    let replaced = replaced_by_feature(&records);
    let mut strings: HashMap<String, Option<String>> = HashMap::new();
    let mut resolve = |source: &str| -> Option<String> {
        strings
            .entry(source.to_string())
            .or_insert_with(|| load_indirect_string(source))
            .clone()
    };
    let mut entries: Vec<SettingEntry> = Vec::new();
    let mut by_title: HashMap<String, usize> = HashMap::new();
    for (index, record) in records.iter().enumerate() {
        if replaced.contains(&index) {
            continue;
        }
        if !record
            .conditions
            .iter()
            .all(|condition| conditions.holds(condition))
        {
            continue;
        }
        let Some((target, kind, subtitle)) = target(record, conditions) else {
            continue;
        };
        let Some(title) = record.description.as_deref().and_then(&mut resolve) else {
            continue;
        };
        let keywords = record
            .high_keywords
            .as_deref()
            .map(|source| {
                resources(source)
                    .filter_map(|source| resolve(&source))
                    .flat_map(|list| {
                        list.split(';')
                            .map(str::trim)
                            .filter(|keyword| !keyword.is_empty())
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // Feature variants and sibling pages often share one title; keep one
        // row and merge the keywords.
        let title_key = title.to_lowercase();
        if let Some(&existing) = by_title.get(&title_key) {
            merge_keywords(&mut entries[existing].keywords, keywords);
            continue;
        }
        let icon_key = match &target {
            Target::Uri(uri) if kind == SettingKind::Page => page_icon_key(uri),
            _ => "control-panel",
        };
        by_title.insert(title_key, entries.len());
        let mut entry = SettingEntry {
            id: format!("{ID_PREFIX}{}", slug(&record.filename)),
            title,
            subtitle,
            keywords: Vec::new(),
            kind,
            icon_key,
            target,
        };
        merge_keywords(&mut entry.keywords, keywords);
        entries.push(entry);
    }
    entries
}

fn merge_keywords(into: &mut Vec<String>, keywords: Vec<String>) {
    for keyword in keywords {
        if !into
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&keyword))
        {
            into.push(keyword);
        }
    }
}

/// The resource references in a keyword field: `@shell32.dll,-1`, or several
/// joined as `@@shell32.dll,-1@shell32.dll,-2`.
fn resources(source: &str) -> impl Iterator<Item = String> + '_ {
    let list = source.strip_prefix("@@");
    let single = list.is_none().then(|| source.to_string());
    list.into_iter()
        .flat_map(|list| list.split('@'))
        .filter(|part| !part.is_empty())
        .map(|part| format!("@{part}"))
        .chain(single)
}

/// Indexes of records that a feature-flagged variant replaces. A setting that
/// changed with a staged feature appears twice, as `Name` with
/// `IncludeWithFeature="F"` and `Name-2` with `ExcludeWithFeature="F"`.
/// Prism cannot read the feature state, so it keeps the newer variant.
fn replaced_by_feature(records: &[Record]) -> HashSet<usize> {
    let included: HashSet<(&str, &str)> = records
        .iter()
        .filter_map(|record| {
            Some((
                base_name(&record.filename),
                record.include_feature.as_deref()?,
            ))
        })
        .collect();
    records
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            record
                .exclude_feature
                .as_deref()
                .is_some_and(|feature| included.contains(&(base_name(&record.filename), feature)))
        })
        .map(|(index, _)| index)
        .collect()
}

fn base_name(filename: &str) -> &str {
    match filename.rsplit_once('-') {
        Some((base, suffix)) if suffix.chars().all(|c| c.is_ascii_digit()) => base,
        _ => filename,
    }
}

fn target(
    record: &Record,
    conditions: &mut Conditions,
) -> Option<(Target, SettingKind, &'static str)> {
    let Some(link) = record.deep_link.as_deref() else {
        let id = record.policy_id.as_deref()?;
        return Some((
            Target::Uri(format!("ms-settings:{id}")),
            SettingKind::Page,
            "Windows Settings",
        ));
    };
    if record
        .host
        .as_deref()
        .is_some_and(|host| host.eq_ignore_ascii_case(WINDOWS_SECURITY_HOST))
    {
        if !conditions.windows_security() {
            return None;
        }
        let uri = if link.eq_ignore_ascii_case("None") {
            "windowsdefender:".to_string()
        } else {
            format!("windowsdefender://{link}")
        };
        return Some((Target::Uri(uri), SettingKind::Page, "Windows Security"));
    }
    Some((command_target(link)?, SettingKind::Task, "Control Panel"))
}

/// A Control Panel deep link: a canonical name (`Microsoft.PowerOptions\page`),
/// a shell or protocol URI, or a command line under the Windows folder.
fn command_target(link: &str) -> Option<Target> {
    let link = expand_environment(link.trim());
    let lower = link.to_ascii_lowercase();
    if lower.starts_with("shell:") || lower.contains("://") {
        return Some(Target::Uri(link));
    }
    if lower.starts_with("microsoft.") {
        let (name, page) = link.split_once('\\').unwrap_or((&link, ""));
        let mut parameters = format!("/name {name}");
        if !page.is_empty() && !page.starts_with('?') {
            parameters.push_str(" /page ");
            parameters.push_str(page);
        }
        return Some(Target::Command {
            file: system32_path("control.exe").to_string_lossy().into_owned(),
            parameters: Some(parameters),
        });
    }
    let (file, parameters) = split_command(&link)?;
    let path = Path::new(&file);
    (path.is_file() && is_windows_system_path(path)).then_some(Target::Command { file, parameters })
}

/// Splits `C:\Windows\system32\mmc.exe C:\Windows\system32\gpedit.msc` after
/// the executable name.
fn split_command(command: &str) -> Option<(String, Option<String>)> {
    if let Some(quoted) = command.strip_prefix('"') {
        let (file, rest) = quoted.split_once('"')?;
        let rest = rest.trim();
        return Some((
            file.to_string(),
            (!rest.is_empty()).then(|| rest.to_string()),
        ));
    }
    let lower = command.to_ascii_lowercase();
    let mut search = 0;
    while let Some(found) = lower[search..].find(".exe") {
        let end = search + found + ".exe".len();
        let rest = &command[end..];
        if rest.is_empty() || rest.starts_with(' ') {
            let rest = rest.trim();
            return Some((
                command[..end].to_string(),
                (!rest.is_empty()).then(|| rest.to_string()),
            ));
        }
        search = end;
    }
    None
}

fn page_icon_key(uri: &str) -> &'static str {
    let page = uri.strip_prefix("ms-settings:").unwrap_or_default();
    let prefixes: &[(&str, &'static str)] = &[
        ("nightlight", "nightlight"),
        ("display", "display"),
        ("bluetooth", "bluetooth"),
        ("connecteddevices", "bluetooth"),
        ("sound", "sound"),
        ("apps-volume", "sound"),
        ("storage", "storage"),
        ("network", "network"),
        ("wifi", "network"),
        ("proxy", "network"),
        ("vpn", "network"),
        ("windowsupdate", "update"),
        ("batterysaver", "power-plan"),
        ("powersleep", "power-plan"),
    ];
    prefixes
        .iter()
        .find(|(prefix, _)| page.starts_with(prefix))
        .map_or("settings", |(_, key)| *key)
}

/// Evaluates `shcond://v1#Name;Negate;Key;Value;...` conditions. `Negate` is
/// `1` when the entry shows only if the test fails. An unknown test hides the
/// entry. Results are cached, because many entries share one test.
#[derive(Default)]
struct Conditions {
    results: HashMap<String, bool>,
    windows_security: Option<bool>,
}

impl Conditions {
    fn holds(&mut self, condition: &str) -> bool {
        if let Some(&result) = self.results.get(condition) {
            return result;
        }
        let result = parse_condition(condition)
            .and_then(|(name, negate, args)| evaluate(name, &args).map(|value| value != negate))
            .unwrap_or(false);
        self.results.insert(condition.to_string(), result);
        result
    }

    fn windows_security(&mut self) -> bool {
        *self
            .windows_security
            .get_or_insert_with(|| registry_key_exists(HKEY_CLASSES_ROOT, "windowsdefender"))
    }
}

fn parse_condition(condition: &str) -> Option<(&str, bool, HashMap<&str, &str>)> {
    let body = condition.strip_prefix("shcond://v1#")?;
    let mut parts = body.split(';');
    let name = parts.next()?;
    let negate = parts.next()? == "1";
    let mut args = HashMap::new();
    while let (Some(key), Some(value)) = (parts.next(), parts.next()) {
        args.insert(key, value);
    }
    Some((name, negate, args))
}

fn evaluate(name: &str, args: &HashMap<&str, &str>) -> Option<bool> {
    let key = || split_registry_path(args.get("Regkey")?);
    Some(match name {
        "IsServer" => product_type().is_some_and(|kind| !kind.eq_ignore_ascii_case("WinNT")),
        "RegkeyExists" => {
            let (root, path) = key()?;
            registry_key_exists(root, path)
        }
        "RegvalExists" => {
            let (root, path) = key()?;
            registry_value_text(root, path, args.get("Regval")?).is_some()
        }
        "RegvalEquals" => {
            let (root, path) = key()?;
            let expected = args.get("RegvalExpected")?;
            registry_value_text(root, path, args.get("Regval")?)
                .is_some_and(|value| value.eq_ignore_ascii_case(expected))
        }
        // Best effort: the test object is registered. Windows also asks the
        // object itself, which Prism does not do.
        "COMCondition" => {
            registry_key_exists(HKEY_CLASSES_ROOT, &format!(r"CLSID\{}", args.get("CLSID")?))
        }
        "SkuEquals" => {
            let edition = registry_value_text(
                HKEY_LOCAL_MACHINE,
                r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
                "EditionID",
            )?;
            match *args.get("Sku")? {
                "anyhome" => edition.starts_with("Core"),
                "starter" => edition.starts_with("Starter"),
                _ => return None,
            }
        }
        "IsUserAdmin" => unsafe { IsUserAnAdmin() }.as_bool(),
        "IsMachineOnDomain" => machine_on_domain(),
        "IsPenAvailable" => digitizer() & (NID_INTEGRATED_PEN | NID_EXTERNAL_PEN) != 0,
        "IsTouchAvailable" => digitizer() & (NID_INTEGRATED_TOUCH | NID_EXTERNAL_TOUCH) != 0,
        "IsTabletPC" => (unsafe { GetSystemMetrics(SM_TABLETPC) }) != 0,
        "IsMobilePC" | "IsPresentationSettingsEnabled" => mobile_pc(),
        "IsMobilityCenterEnabled" => mobile_pc() && system32_path("mblctr.exe").is_file(),
        _ => return None,
    })
}

const NID_INTEGRATED_TOUCH: i32 = 0x01;
const NID_EXTERNAL_TOUCH: i32 = 0x02;
const NID_INTEGRATED_PEN: i32 = 0x04;
const NID_EXTERNAL_PEN: i32 = 0x08;

fn digitizer() -> i32 {
    unsafe { GetSystemMetrics(SM_DIGITIZER) }
}

fn mobile_pc() -> bool {
    let role = unsafe { PowerDeterminePlatformRoleEx(POWER_PLATFORM_ROLE_V2) };
    role == PlatformRoleMobile || role == PlatformRoleSlate
}

fn machine_on_domain() -> bool {
    let mut name = PWSTR::null();
    let mut status = NETSETUP_JOIN_STATUS::default();
    let result = unsafe { NetGetJoinInformation(PCWSTR::null(), &mut name, &mut status) };
    if !name.is_null() {
        unsafe { NetApiBufferFree(Some(name.as_ptr() as *const _)) };
    }
    result == 0 && status == NetSetupDomainName
}

fn product_type() -> Option<String> {
    registry_value_text(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\ProductOptions",
        "ProductType",
    )
}

fn split_registry_path(path: &str) -> Option<(HKEY, &str)> {
    let (root, rest) = path.split_once('\\')?;
    let root = match root.to_ascii_uppercase().as_str() {
        "HKEY_LOCAL_MACHINE" | "HKLM" => HKEY_LOCAL_MACHINE,
        "HKEY_CURRENT_USER" | "HKCU" => HKEY_CURRENT_USER,
        "HKEY_CLASSES_ROOT" | "HKCR" => HKEY_CLASSES_ROOT,
        _ => return None,
    };
    Some((root, rest))
}

fn registry_key_exists(root: HKEY, path: &str) -> bool {
    match unsafe { open_registry_key(root, path) } {
        Some(key) => {
            unsafe {
                let _ = RegCloseKey(key);
            }
            true
        }
        None => false,
    }
}

/// A string or DWORD registry value as text.
fn registry_value_text(root: HKEY, path: &str, name: &str) -> Option<String> {
    let key = unsafe { open_registry_key(root, path) }?;
    let value =
        unsafe { read_registry_string(key, name) }.or_else(|| unsafe { read_dword(key, name) });
    unsafe {
        let _ = RegCloseKey(key);
    }
    value
}

unsafe fn read_dword(key: HKEY, name: &str) -> Option<String> {
    let name = crate::windows_tools::wide(name);
    let mut value_type = REG_VALUE_TYPE(0);
    let mut data = 0u32;
    let mut length = std::mem::size_of::<u32>() as u32;
    let result = RegQueryValueExW(
        key,
        PCWSTR(name.as_ptr()),
        None,
        Some(&mut value_type),
        Some((&mut data as *mut u32).cast()),
        Some(&mut length),
    );
    (result.0 == 0 && value_type == REG_DWORD).then(|| data.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<PCSettings>
  <SearchableContent>
    <Filename>Classic_{638f8e21-e157-40d7-97e0-a0c8e4c4e2b5}</Filename>
    <ApplicationInformation>
      <DeepLink>%windir%\system32\UserAccountControlSettings.exe</DeepLink>
    </ApplicationInformation>
    <SettingIdentity>
      <HostID>{12B1697E-D3A0-4DBC-B568-CCF64A3F934D}</HostID>
    </SettingIdentity>
    <SettingInformation>
      <Description>@shell32.dll,-24317</Description>
      <HighKeywords>@shell32.dll,-25347</HighKeywords>
      <Keywords>@@shell32.dll,-25078@shell32.dll,-25081</Keywords>
    </SettingInformation>
  </SearchableContent>
  <SearchableContent IncludeWithFeature="Cleanup">
    <Filename>AAA_Page</Filename>
    <SettingIdentity>
      <SettingPaths>
        <Path>
          <PageID>SettingsPageAppsDefaults</PageID>
          <PolicyIds>defaultapps;defaultbrowsersettings</PolicyIds>
        </Path>
      </SettingPaths>
      <Condition>shcond://v1#IsServer;1</Condition>
    </SettingIdentity>
    <SettingInformation>
      <Description>Default &amp; apps</Description>
    </SettingInformation>
  </SearchableContent>
  <SearchableContent ExcludeWithFeature="Cleanup">
    <Filename>AAA_Page-2</Filename>
  </SearchableContent>
</PCSettings>"#;

    #[test]
    fn parses_searchable_content() {
        let records = parse(SAMPLE);
        assert_eq!(records.len(), 3);
        let task = &records[0];
        assert_eq!(task.description.as_deref(), Some("@shell32.dll,-24317"));
        assert_eq!(task.high_keywords.as_deref(), Some("@shell32.dll,-25347"));
        assert_eq!(
            task.deep_link.as_deref(),
            Some(r"%windir%\system32\UserAccountControlSettings.exe")
        );
        let page = &records[1];
        assert_eq!(page.policy_id.as_deref(), Some("defaultapps"));
        assert_eq!(page.description.as_deref(), Some("Default & apps"));
        assert_eq!(page.include_feature.as_deref(), Some("Cleanup"));
        assert_eq!(page.conditions, vec!["shcond://v1#IsServer;1".to_string()]);
        assert_eq!(records[2].exclude_feature.as_deref(), Some("Cleanup"));
        assert_eq!(replaced_by_feature(&records), HashSet::from([2]));
    }

    #[test]
    fn pages_open_by_the_name_that_matches_the_page_id() {
        assert_eq!(
            page_name(
                "SettingsPageTimeRegionLanguage",
                "keyboard;keyboard-advanced;regionformatting;regionlanguage;regionlanguage-jpnime"
            )
            .as_deref(),
            Some("regionlanguage")
        );
        assert_eq!(
            page_name("SettingsPageBackground", "personalization-background").as_deref(),
            Some("personalization-background")
        );
        assert_eq!(page_name("SettingsPageX", " ; "), None);
    }

    #[test]
    fn keyword_fields_list_each_resource() {
        assert_eq!(
            resources("@@shell32.dll,-1@shell32.dll,-2").collect::<Vec<_>>(),
            vec!["@shell32.dll,-1", "@shell32.dll,-2"]
        );
        assert_eq!(
            resources("@shell32.dll,-1").collect::<Vec<_>>(),
            vec!["@shell32.dll,-1"]
        );
    }

    #[test]
    fn conditions_read_the_negate_flag() {
        let (name, negate, args) = parse_condition(
            r"shcond://v1#RegvalEquals;1;Regkey;HKEY_LOCAL_MACHINE\SYSTEM;Regval;A;RegvalExpected;ARM",
        )
        .unwrap();
        assert_eq!(name, "RegvalEquals");
        assert!(negate);
        assert_eq!(args.get("RegvalExpected"), Some(&"ARM"));
        let mut conditions = Conditions::default();
        assert!(!conditions.holds("shcond://v1#SomethingNew;0"));
        assert!(!conditions.holds("not a condition"));
    }

    #[test]
    fn commands_split_after_the_executable() {
        assert_eq!(
            split_command(r"C:\Windows\system32\mmc.exe C:\Windows\system32\gpedit.msc"),
            Some((
                r"C:\Windows\system32\mmc.exe".to_string(),
                Some(r"C:\Windows\system32\gpedit.msc".to_string())
            ))
        );
        assert_eq!(
            split_command(r"C:\Windows\system32\UserAccountControlSettings.exe"),
            Some((
                r"C:\Windows\system32\UserAccountControlSettings.exe".to_string(),
                None
            ))
        );
        assert_eq!(split_command("Threat"), None);
    }

    #[test]
    fn canonical_names_open_through_control() {
        let Some(Target::Command { parameters, .. }) =
            command_target(r"Microsoft.PowerOptions\pageGlobalSettings")
        else {
            panic!("expected a command");
        };
        assert_eq!(
            parameters.as_deref(),
            Some("/name Microsoft.PowerOptions /page pageGlobalSettings")
        );
        let Some(Target::Command { parameters, .. }) =
            command_target(r"Microsoft.CredentialManager\?SelectedVault=CredmanVault")
        else {
            panic!("expected a command");
        };
        assert_eq!(
            parameters.as_deref(),
            Some("/name Microsoft.CredentialManager")
        );
    }

    #[test]
    fn sample_builds_the_uac_task_and_the_page() {
        let entries = build(parse(SAMPLE), &mut Conditions::default());
        let uac = entries
            .iter()
            .find(|entry| entry.kind == SettingKind::Task)
            .expect("UAC task");
        assert_eq!(uac.subtitle, "Control Panel");
        assert!(uac
            .keywords
            .iter()
            .any(|keyword| keyword.eq_ignore_ascii_case("UAC")));
        let page = entries
            .iter()
            .find(|entry| entry.kind == SettingKind::Page)
            .expect("page");
        assert_eq!(page.title, "Default & apps");
        assert_eq!(page.uri(), Some("ms-settings:defaultapps"));
    }

    #[test]
    fn this_pc_index_has_the_start_menu_settings() {
        let entries = load();
        if index_files().is_empty() {
            return;
        }
        assert!(entries.len() > 200, "only {} settings", entries.len());
        let uac = entries
            .iter()
            .find(|entry| {
                entry
                    .keywords
                    .iter()
                    .any(|keyword| keyword.eq_ignore_ascii_case("UAC"))
            })
            .expect("a setting with the UAC keyword");
        assert!(
            matches!(&uac.target, Target::Command { file, .. }
                if file.to_ascii_lowercase().ends_with("useraccountcontrolsettings.exe")),
            "{uac:?}"
        );
        let ids = entries
            .iter()
            .map(|entry| &entry.id)
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), entries.len(), "ids are unique");
    }
}
