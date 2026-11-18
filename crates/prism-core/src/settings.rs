//! The persisted settings (`prism.json`): the backend validates each save and
//! reads the startup values here. The values each setting can take are in
//! `prism_vocab::settings`, which the UI's sanitizer reads too.

use std::collections::HashSet;

use prism_vocab::settings::*;
use serde_json::Value;

/// Validates a shortcut string against the whitelist.
pub fn validate_shortcut(combo: &str) -> Result<(), String> {
    if !SHORTCUTS.contains(&combo) {
        return Err(format!("shortcut '{combo}' is not supported; pick one of: {}", SHORTCUTS.join(", ")));
    }
    Ok(())
}

fn setting<'a>(state: Option<&'a Value>, key: &str) -> Option<&'a str> {
    state.and_then(|value| value.get("settings")).and_then(|settings| settings.get(key)).and_then(Value::as_str)
}

pub fn startup_shortcut(state: Option<&Value>) -> String {
    setting(state, "shortcut").filter(|shortcut| SHORTCUTS.contains(shortcut)).unwrap_or(SHORTCUTS[0]).to_string()
}

pub fn startup_taskbar_alignment(state: Option<&Value>) -> String {
    setting(state, "taskbarAlignment")
        .filter(|alignment| TASKBAR_ALIGNMENTS.contains(alignment))
        .unwrap_or(TASKBAR_ALIGNMENTS[0])
        .to_string()
}


/// The persisted window width, when `is_window_width` accepts it.
pub fn startup_width(state: Option<&Value>) -> Option<u32> {
    let width = u32::try_from(state?.pointer("/settings/width")?.as_u64()?).ok()?;
    is_window_width(width).then_some(width)
}

/// Upgrades a version 2 state. `Ok(None)`: an unknown version, read as no state.
pub fn migrate_state_value(mut value: Value) -> Result<Option<(Value, bool)>, String> {
    let version = value.get("version").and_then(Value::as_u64);
    if version == Some(u64::from(STATE_VERSION)) {
        return Ok(Some((value, false)));
    }
    if version != Some(u64::from(LEGACY_STATE_VERSION)) {
        return Ok(None);
    }

    let settings = value
        .get_mut("settings")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "legacy state.settings must be an object".to_string())?;
    settings.insert("shortcut".to_string(), serde_json::json!(WIN_SHORTCUT));
    value["version"] = serde_json::json!(STATE_VERSION);
    validate_state(&value)?;
    Ok(Some((value, true)))
}

/// Each entry is a known value, at most once; at most `limit` entries. `key`
/// names the setting, `label` its entries in the messages.
fn validate_unique_list(value: &Value, key: &str, label: &str, known: &[&str], limit: usize) -> Result<(), String> {
    let entries = value.as_array().ok_or_else(|| format!("state.settings.{key} must be an array"))?;
    if entries.len() > limit {
        return Err(format!("state.settings.{key} has too many entries"));
    }
    let mut seen = HashSet::new();
    for entry in entries {
        let entry = entry.as_str().ok_or_else(|| format!("{label} entries must be strings"))?;
        if !known.contains(&entry) {
            return Err(format!("unknown {label} entry '{entry}'"));
        }
        if !seen.insert(entry) {
            return Err(format!("duplicate {label} entry '{entry}'"));
        }
    }
    Ok(())
}

fn validate_bool(settings: &serde_json::Map<String, Value>, key: &str) -> Result<(), String> {
    match settings.get(key) {
        Some(value) if !value.is_boolean() => Err(format!("state.settings.{key} must be a boolean")),
        _ => Ok(()),
    }
}

fn is_id(value: &Value) -> bool {
    value.as_str().is_some_and(|value| !value.is_empty() && value.len() <= ID_MAX)
}

/// Validates the persisted state shape; returns Err on anything unexpected.
pub fn validate_state(state: &Value) -> Result<(), String> {
    let obj = state.as_object().ok_or("state must be an object")?;
    let settings = obj.get("settings").and_then(Value::as_object).ok_or("state.settings must be an object")?;
    if let Some(accent) = settings.get("accent").and_then(Value::as_str).filter(|accent| !ACCENTS.contains(accent)) {
        return Err(format!("unknown accent '{accent}'"));
    }
    if let Some(theme) = settings.get("theme").and_then(Value::as_str).filter(|theme| !THEMES.contains(theme)) {
        return Err(format!("unknown theme '{theme}'"));
    }
    validate_bool(settings, "alwaysOnTop")?;
    validate_bool(settings, "taskbarScrollVolume")?;
    if let Some(alignment) = settings.get("taskbarAlignment") {
        let alignment = alignment.as_str().ok_or("state.settings.taskbarAlignment must be a string")?;
        if !TASKBAR_ALIGNMENTS.contains(&alignment) {
            return Err(format!("unknown taskbar alignment '{alignment}'"));
        }
    }
    if let Some(shortcut) = settings.get("shortcut").and_then(Value::as_str) {
        validate_shortcut(shortcut)?;
    }
    if let Some(width) =
        settings.get("width").and_then(Value::as_u64).filter(|&width| !WIDTHS.iter().any(|&known| u64::from(known) == width))
    {
        return Err(format!("unsupported width '{width}'"));
    }
    if let Some(zoom) = settings.get("viewZoom") {
        let zoom = zoom.as_u64().ok_or("state.settings.viewZoom must be an integer")?;
        if !VIEW_ZOOM_LEVELS.iter().any(|&known| u64::from(known) == zoom) {
            return Err(format!("unsupported view zoom '{zoom}'"));
        }
    }
    if let Some(quick_access) = settings.get("quickAccess") {
        validate_unique_list(quick_access, "quickAccess", "quick access", &QUICK_ACCESS_KINDS, QUICK_ACCESS_LIMIT)?;
    }
    validate_bool(settings, "quickAccessCollapsed")?;
    if let Some(section_order) = settings.get("sectionOrder") {
        validate_unique_list(section_order, "sectionOrder", "section order", &SECTION_IDS, SECTION_IDS.len())?;
    }
    if let Some(pinned_apps) = settings.get("pinnedApps") {
        let entries = pinned_apps.as_array().ok_or("state.settings.pinnedApps must be an array")?;
        if entries.len() > PINNED_APP_LIMIT {
            return Err("state.settings.pinnedApps has too many entries".to_string());
        }
        let mut seen = HashSet::new();
        for entry in entries {
            if !is_id(entry) {
                return Err("pinned app ids must be non-empty strings".to_string());
            }
            if !seen.insert(entry.as_str()) {
                return Err("state.settings.pinnedApps contains duplicates".to_string());
            }
        }
    }
    if let Some(app_groups) = settings.get("appGroups") {
        let groups = app_groups.as_array().ok_or("state.settings.appGroups must be an array")?;
        if groups.len() > APP_GROUP_LIMIT {
            return Err("state.settings.appGroups has too many entries".to_string());
        }
        let mut group_ids = HashSet::new();
        for group in groups {
            let group = group.as_object().ok_or("app group entries must be objects")?;
            let id = group
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= APP_GROUP_ID_MAX)
                .ok_or("app group ids must be non-empty strings")?;
            let name = group
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty() && value.len() <= APP_GROUP_NAME_MAX)
                .ok_or("app group names must be non-empty strings")?;
            if !group_ids.insert(id) {
                return Err("state.settings.appGroups contains duplicate ids".to_string());
            }
            if !group.get("collapsed").is_some_and(Value::is_boolean) {
                return Err("app group collapsed must be a boolean".to_string());
            }
            let app_ids = group.get("appIds").and_then(Value::as_array).ok_or("app group appIds must be an array")?;
            if app_ids.len() > APP_GROUP_APP_LIMIT {
                return Err(format!("app group '{name}' has too many apps"));
            }
            let mut seen = HashSet::new();
            for app_id in app_ids {
                if !is_id(app_id) {
                    return Err("app group app ids must be non-empty strings".to_string());
                }
                if !seen.insert(app_id.as_str()) {
                    return Err(format!("app group '{name}' contains duplicate apps"));
                }
            }
        }
    }
    if let Some(history) = obj.get("history") {
        let entries = history.as_array().ok_or("state.history must be an array")?;
        if entries.len() > HISTORY_LIMIT {
            return Err("state.history has too many entries".to_string());
        }
        for entry in entries {
            let entry = entry.as_object().ok_or("history entries must be objects")?;
            let valid_id = entry.get("id").is_some_and(is_id);
            let valid_title = entry
                .get("title")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty() && value.len() <= HISTORY_TITLE_MAX);
            let valid_timestamp = entry.get("ts").is_some_and(Value::is_u64);
            if !valid_id || !valid_title || !valid_timestamp {
                return Err("invalid history entry".to_string());
            }
        }
    }
    Ok(())
}
