//! Two-phase palette query.
//!
//! Phase 1 is in-process: open windows, Start Menu / UWP apps,
//! and the typed action catalog (plus Jev phrase mapping).
//! Phase 2 is SQLite FTS5 file search and stays on `search_files`.

mod catalog;
mod jev;
mod open_windows;
pub mod score;

use crate::apps::AppEntry;
use serde::Serialize;

pub use jev::IntentStore;
pub use open_windows::list as list_open_windows;
pub use open_windows::OpenWindow;

const WINDOW_LIMIT: usize = 8;
const APP_LIMIT: usize = 6;
const ACTION_LIMIT: usize = 16;
/// Windows writes broad synonym lists for its settings ("screen saver" is a
/// keyword of an energy page). A keyword hit stays below any title that
/// contains the query (800 and up).
const SETTING_KEYWORD_CAP: i32 = 799;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase1Kind {
    Window,
    App,
    Action,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Phase1Hit {
    pub id: String,
    pub kind: Phase1Kind,
    pub title: String,
    pub subtitle: String,
    pub score: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hwnd: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Phase1Response {
    pub query: String,
    pub windows: Vec<Phase1Hit>,
    pub apps: Vec<Phase1Hit>,
    pub actions: Vec<Phase1Hit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_action_id: Option<String>,
}

pub fn empty_phase1(query: &str) -> Phase1Response {
    Phase1Response {
        query: query.to_string(),
        windows: Vec::new(),
        apps: Vec::new(),
        actions: Vec::new(),
        intent_action_id: None,
    }
}

pub fn phase1(
    query: &str,
    apps: &[AppEntry],
    windows: &[OpenWindow],
    intent: &IntentStore,
) -> Phase1Response {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return empty_phase1("");
    }
    let query_lower = trimmed.to_lowercase();
    let intent_action_id = jev::resolve(&query_lower, intent).map(str::to_string);

    let window_hits = open_windows::search(&query_lower, windows, WINDOW_LIMIT)
        .into_iter()
        .map(|(window, score)| window_hit(&window, score))
        .collect();
    let app_hits = search_apps(&query_lower, apps);
    let actions = search_actions(&query_lower, intent_action_id.as_deref());

    Phase1Response {
        query: trimmed.to_string(),
        windows: window_hits,
        apps: app_hits,
        actions,
        intent_action_id,
    }
}

pub fn execute(action_id: &str) -> Result<(), String> {
    if let Some(action) = catalog::get(action_id) {
        return catalog::execute(action);
    }
    if let Some(result) = crate::settings_index::execute(action_id) {
        return result;
    }
    crate::windows_tools::execute(action_id)
}

pub fn accept_intent(query: &str, action_id: &str, store: &IntentStore) -> Result<(), String> {
    store.remember(query, action_id)
}

pub fn focus_window(hwnd: i64) -> Result<(), String> {
    open_windows::focus(hwnd)
}

fn search_actions(query: &str, intent_id: Option<&str>) -> Vec<Phase1Hit> {
    let mut hits: Vec<Phase1Hit> = catalog::search(query, ACTION_LIMIT)
        .into_iter()
        .map(|(action, score)| action_hit(action, score, false))
        .collect();

    if let Some(tools) = crate::windows_tools::cached() {
        for tool in tools.iter() {
            let keyword_refs = tool
                .keywords
                .iter()
                .map(|keyword| keyword.as_str())
                .collect::<Vec<_>>();
            if let Some(score) = score::score_with_keywords(query, &tool.title, &keyword_refs) {
                hits.push(tool_hit(tool, score));
            }
        }
    }
    if let Some(settings) = crate::settings_index::cached() {
        for setting in settings.iter() {
            let keyword_refs = setting
                .keywords
                .iter()
                .map(|keyword| keyword.as_str())
                .collect::<Vec<_>>();
            let title = score::score_text(query, &setting.title);
            let keyword = score::score_with_keywords(query, "", &keyword_refs)
                .map(|score| score.min(SETTING_KEYWORD_CAP));
            if let Some(score) = title.max(keyword) {
                hits.push(setting_hit(setting, score));
            }
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.title.cmp(&right.title))
    });
    // The settings index repeats some curated actions and tools ("Night
    // light", "Device Manager"); show each title once, best score first.
    let mut titles = std::collections::HashSet::new();
    hits.retain(|hit| titles.insert(hit.title.to_lowercase()));
    hits.truncate(ACTION_LIMIT);

    if let Some(intent_id) = intent_id {
        if let Some(action) = catalog::get(intent_id) {
            hits.retain(|hit| hit.action_id.as_deref() != Some(intent_id));
            hits.insert(0, action_hit(action, 10_000, true));
            hits.truncate(ACTION_LIMIT);
        }
    }
    hits
}

fn search_apps(query: &str, apps: &[AppEntry]) -> Vec<Phase1Hit> {
    let mut ranked: Vec<(&AppEntry, i32, i32)> = Vec::new();
    for app in apps {
        let keyword_refs: Vec<&str> = app.keywords.iter().map(String::as_str).collect();
        let Some(score) = score::score_with_keywords(query, &app.name, &keyword_refs) else {
            continue;
        };
        ranked.push((app, score, source_rank(&app.source)));
    }
    ranked.sort_by(|a, b| {
        a.0.normalized_name
            .cmp(&b.0.normalized_name)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| b.1.cmp(&a.1))
    });
    ranked.dedup_by(|a, b| a.0.normalized_name == b.0.normalized_name);
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
    ranked.truncate(APP_LIMIT);
    ranked
        .into_iter()
        .map(|(app, score, _)| app_hit(app, score))
        .collect()
}

fn action_hit(action: &catalog::TypedAction, score: i32, from_intent: bool) -> Phase1Hit {
    Phase1Hit {
        id: format!("action::{}", action.id),
        kind: Phase1Kind::Action,
        title: action.title.to_string(),
        subtitle: if from_intent {
            format!("{} · from your wording", action.subtitle)
        } else {
            action.subtitle.to_string()
        },
        score,
        app_id: None,
        path: None,
        hwnd: None,
        action_id: Some(action.id.to_string()),
        uri: action.uri.map(str::to_string),
        icon_key: Some(action.icon_key.to_string()),
        icon: None,
        source: None,
    }
}

fn tool_hit(tool: &crate::windows_tools::WindowsTool, score: i32) -> Phase1Hit {
    Phase1Hit {
        id: format!("action::{}", tool.id),
        kind: Phase1Kind::Action,
        title: tool.title.clone(),
        subtitle: tool.subtitle.clone(),
        score,
        app_id: None,
        path: None,
        hwnd: None,
        action_id: Some(tool.id.clone()),
        uri: None,
        icon_key: Some(tool.icon_key.to_string()),
        icon: tool.icon.clone(),
        source: Some("windows-tool".to_string()),
    }
}

fn setting_hit(setting: &crate::settings_index::SettingEntry, score: i32) -> Phase1Hit {
    // Control Panel tasks list with the Windows tools; pages carry a URI and
    // list under Settings.
    let task = setting.kind == crate::settings_index::SettingKind::Task;
    Phase1Hit {
        id: format!("action::{}", setting.id),
        kind: Phase1Kind::Action,
        title: setting.title.clone(),
        subtitle: setting.subtitle.to_string(),
        score,
        app_id: None,
        path: None,
        hwnd: None,
        action_id: Some(setting.id.clone()),
        uri: setting.uri().map(str::to_string),
        icon_key: Some(setting.icon_key.to_string()),
        icon: None,
        source: task.then(|| "windows-tool".to_string()),
    }
}

fn app_hit(app: &AppEntry, score: i32) -> Phase1Hit {
    Phase1Hit {
        id: format!("app::{}", app.app_id),
        kind: Phase1Kind::App,
        title: app.name.clone(),
        subtitle: "Application".to_string(),
        score,
        app_id: Some(app.app_id.clone()),
        path: app.path.clone(),
        hwnd: None,
        action_id: None,
        uri: None,
        icon_key: None,
        icon: None,
        source: Some(app.source.clone()),
    }
}

fn window_hit(window: &OpenWindow, score: i32) -> Phase1Hit {
    let subtitle = if window.process_name.is_empty() {
        "Open window".to_string()
    } else {
        format!("Open window · {}", window.process_name)
    };
    Phase1Hit {
        id: format!("window::{}", window.hwnd),
        kind: Phase1Kind::Window,
        title: window.title.clone(),
        subtitle,
        score,
        app_id: None,
        path: None,
        hwnd: Some(window.hwnd),
        action_id: None,
        uri: None,
        icon_key: Some("window".to_string()),
        icon: None,
        source: None,
    }
}

fn source_rank(source: &str) -> i32 {
    match source {
        "taskbar" => 6,
        "startMenu" => 5,
        "desktop" => 4,
        "appsFolder" => 3,
        "registry" | "appPaths" | "applications" => 2,
        "programs" => 1,
        _ => 0,
    }
}
