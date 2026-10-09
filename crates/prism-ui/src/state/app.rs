//! App state (`src/state/app.tsx`). So far: the option lists and the
//! validators for persisted settings and history (app.tsx:79-96, 654-812).
//!
//! TODO(port): the `AppState` entity (the provider, app.tsx:1-653) comes with
//! subsystem 3; it needs GPUI. The queue counts this file as done before that.

use std::collections::HashSet;

use serde_json::Value;

use crate::js;
use crate::lib::types::{
    APP_GROUP_APP_LIMIT, APP_GROUP_LIMIT, AppGroup, DEFAULT_QUICK_ACCESS, DEFAULT_SECTION_ORDER, HistoryEntry,
    PINNED_APP_LIMIT, QUICK_ACCESS_KINDS, QUICK_ACCESS_LIMIT, SECTION_ORDER_LIMIT, Settings, VIEW_ZOOM_LEVELS,
};

pub struct ShortcutOption {
    pub value: &'static str,
    pub label: &'static str,
}

pub const SHORTCUT_OPTIONS: [ShortcutOption; 8] = [
    ShortcutOption { value: "Win", label: "Win key" },
    ShortcutOption { value: "Ctrl+Alt+Space", label: "Ctrl + Alt + Space" },
    ShortcutOption { value: "Ctrl+Alt+S", label: "Ctrl + Alt + S" },
    ShortcutOption { value: "Ctrl+Alt+Shift+S", label: "Ctrl + Alt + Shift + S" },
    ShortcutOption { value: "Ctrl+Alt+P", label: "Ctrl + Alt + P" },
    ShortcutOption { value: "Ctrl+Alt+Shift+P", label: "Ctrl + Alt + Shift + P" },
    ShortcutOption { value: "Ctrl+Alt+Enter", label: "Ctrl + Alt + Enter" },
    ShortcutOption { value: "Ctrl+Alt+Shift+Enter", label: "Ctrl + Alt + Shift + Enter" },
];

pub struct ThemeOption {
    pub value: &'static str,
    pub label: &'static str,
}

pub const THEME_OPTIONS: [ThemeOption; 3] = [
    ThemeOption { value: "system", label: "System" },
    ThemeOption { value: "dark", label: "Dark" },
    ThemeOption { value: "light", label: "Light" },
];

pub const HISTORY_CAP: usize = 20;

/// `allowed.includes(value) ? value : fallback` for string options.
fn pick_str(value: Option<&Value>, allowed: &[&str], fallback: &str) -> String {
    match value {
        Some(Value::String(s)) if allowed.contains(&s.as_str()) => s.clone(),
        _ => fallback.to_owned(),
    }
}

/// `allowed.includes(value) ? value : fallback` for number options.
fn pick_num(value: Option<&Value>, allowed: &[f64], fallback: f64) -> f64 {
    match value.and_then(Value::as_f64) {
        Some(n) if allowed.contains(&n) => n,
        _ => fallback,
    }
}

fn pick_bool(value: Option<&Value>, fallback: bool) -> bool {
    value.and_then(Value::as_bool).unwrap_or(fallback)
}

/// Validates loaded settings against the known value sets; anything
/// unexpected falls back to the default. Mirrors the backend validator.
/// `None` is "nothing persisted" (`undefined`).
pub fn sanitize_settings(raw: Option<&Value>) -> Settings {
    // `(raw ?? {})[key]`: a string, number, or array has none of these keys.
    let src = raw.and_then(Value::as_object);
    let field = |key: &str| src.and_then(|o| o.get(key));
    let defaults = Settings::default();
    let shortcuts: Vec<&str> = SHORTCUT_OPTIONS.iter().map(|o| o.value).collect();
    Settings {
        accent: pick_str(field("accent"), &["iris", "azure", "mint", "amber", "rose"], &defaults.accent),
        width: pick_num(field("width"), &[560.0, 640.0, 720.0], defaults.width),
        view_zoom: pick_num(field("viewZoom"), &VIEW_ZOOM_LEVELS, defaults.view_zoom),
        shortcut: pick_str(field("shortcut"), &shortcuts, &defaults.shortcut),
        always_on_top: pick_bool(field("alwaysOnTop"), defaults.always_on_top),
        taskbar_alignment: pick_str(
            field("taskbarAlignment"),
            &["left", "center", "right"],
            &defaults.taskbar_alignment,
        ),
        taskbar_scroll_volume: pick_bool(field("taskbarScrollVolume"), defaults.taskbar_scroll_volume),
        theme: pick_str(field("theme"), &["system", "dark", "light"], &defaults.theme),
        quick_access: sanitize_quick_access(field("quickAccess")),
        quick_access_collapsed: pick_bool(field("quickAccessCollapsed"), defaults.quick_access_collapsed),
        pinned_apps: sanitize_pinned_apps(field("pinnedApps")),
        app_groups: sanitize_app_groups(field("appGroups")),
        section_order: sanitize_section_order(field("sectionOrder")),
    }
}

fn sanitize_section_order(raw: Option<&Value>) -> Vec<String> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return DEFAULT_SECTION_ORDER.iter().map(|s| s.to_string()).collect();
    };
    let mut result: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for value in raw {
        let Some(value) = value.as_str() else {
            continue;
        };
        if !DEFAULT_SECTION_ORDER.contains(&value) || seen.contains(value) {
            continue;
        }
        seen.insert(value);
        result.push(value.to_owned());
        if result.len() >= SECTION_ORDER_LIMIT {
            break;
        }
    }
    for id in DEFAULT_SECTION_ORDER {
        if !seen.contains(id) {
            result.push(id.to_owned());
        }
    }
    result
}

fn sanitize_app_groups(raw: Option<&Value>) -> Vec<AppGroup> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<AppGroup> = Vec::new();
    for value in raw {
        // `!value || typeof value !== "object"` lets arrays through, but an
        // array has no `id`, so it is skipped below either way.
        let Some(src) = value.as_object() else {
            continue;
        };
        let id = src.get("id").and_then(Value::as_str).map_or("", js::trim);
        let name = src.get("name").and_then(Value::as_str).map_or("", js::trim);
        if id.is_empty() || js::len(id) > 96 || seen.contains(id) || name.is_empty() || js::len(name) > 64 {
            continue;
        }
        let app_ids = sanitize_ids(src.get("appIds"), APP_GROUP_APP_LIMIT);
        result.push(AppGroup {
            id: id.to_owned(),
            name: name.to_owned(),
            app_ids,
            collapsed: src.get("collapsed").and_then(Value::as_bool).unwrap_or(false),
        });
        seen.insert(id.to_owned());
        if result.len() >= APP_GROUP_LIMIT {
            break;
        }
    }
    result
}

fn sanitize_pinned_apps(raw: Option<&Value>) -> Vec<String> {
    sanitize_ids(raw, PINNED_APP_LIMIT)
}

fn sanitize_ids(raw: Option<&Value>, limit: usize) -> Vec<String> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut result: Vec<String> = Vec::new();
    for value in raw {
        let Some(value) = value.as_str() else {
            continue;
        };
        if value.is_empty() || js::len(value) > 4096 || seen.contains(value) {
            continue;
        }
        seen.insert(value);
        result.push(value.to_owned());
        if result.len() == limit {
            break;
        }
    }
    result
}

fn sanitize_quick_access(raw: Option<&Value>) -> Vec<String> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return DEFAULT_QUICK_ACCESS.iter().map(|s| s.to_string()).collect();
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut result: Vec<String> = Vec::new();
    for value in raw {
        let Some(kind) = value.as_str() else {
            continue;
        };
        if !QUICK_ACCESS_KINDS.contains(&kind) || seen.contains(kind) {
            continue;
        }
        seen.insert(kind);
        result.push(kind.to_owned());
        if result.len() == QUICK_ACCESS_LIMIT {
            break;
        }
    }
    result
}

pub fn sanitize_history(raw: Option<&Value>) -> Vec<HistoryEntry> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut history: Vec<HistoryEntry> = Vec::new();
    for value in raw {
        let Some(entry) = value.as_object() else {
            continue;
        };
        let (Some(id), Some(title), Some(ts)) = (
            entry.get("id").and_then(Value::as_str),
            entry.get("title").and_then(Value::as_str),
            entry.get("ts").filter(|ts| ts.is_number()).and_then(Value::as_f64),
        ) else {
            continue;
        };
        if id.is_empty()
            || js::len(id) > 4096
            || title.is_empty()
            || js::len(title) > 512
            || !ts.is_finite()
            || seen.contains(id)
        {
            continue;
        }
        seen.insert(id);
        history.push(HistoryEntry { id: id.to_owned(), title: title.to_owned(), ts });
        if history.len() == HISTORY_CAP {
            break;
        }
    }
    history
}

// PORT STATUS: confidence=high todos=1
