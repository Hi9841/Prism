//! Two-phase palette query.
//!
//! Phase 1 is in-process: recent commands, open windows, Start Menu / UWP apps,
//! and the typed action catalog (plus Jev phrase mapping).
//! Phase 2 is SQLite FTS5 file search and stays on `search_files`.

mod catalog;
mod jev;
mod open_windows;
mod score;

use crate::apps::AppEntry;
use serde::{Deserialize, Serialize};

pub use jev::IntentStore;
pub use open_windows::list as list_open_windows;
pub use open_windows::OpenWindow;

const RECENT_LIMIT: usize = 5;
const WINDOW_LIMIT: usize = 8;
const APP_LIMIT: usize = 6;
const ACTION_LIMIT: usize = 8;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase1Kind {
    Recent,
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
    pub source: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Phase1Response {
    pub query: String,
    pub recents: Vec<Phase1Hit>,
    pub windows: Vec<Phase1Hit>,
    pub apps: Vec<Phase1Hit>,
    pub actions: Vec<Phase1Hit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_action_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentCommand {
    pub id: String,
    pub title: String,
}

pub fn empty_phase1(query: &str) -> Phase1Response {
    Phase1Response {
        query: query.to_string(),
        recents: Vec::new(),
        windows: Vec::new(),
        apps: Vec::new(),
        actions: Vec::new(),
        intent_action_id: None,
    }
}

pub fn phase1(
    query: &str,
    recent: &[RecentCommand],
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

    let recents = search_recents(&query_lower, recent);
    let window_hits = open_windows::search(&query_lower, windows, WINDOW_LIMIT)
        .into_iter()
        .map(|(window, score)| window_hit(&window, score))
        .collect();
    let app_hits = search_apps(&query_lower, apps);
    let actions = search_actions(&query_lower, intent_action_id.as_deref());

    Phase1Response {
        query: trimmed.to_string(),
        recents,
        windows: window_hits,
        apps: app_hits,
        actions,
        intent_action_id,
    }
}

pub fn execute(action_id: &str) -> Result<(), String> {
    let action =
        catalog::get(action_id).ok_or_else(|| format!("unknown action id '{action_id}'"))?;
    catalog::execute(action)
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

fn search_recents(query: &str, recent: &[RecentCommand]) -> Vec<Phase1Hit> {
    let mut ranked: Vec<Phase1Hit> = recent
        .iter()
        .filter_map(|entry| {
            score::score_text(query, &entry.title).map(|score| recent_hit(entry, score))
        })
        .collect();
    ranked.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    ranked.truncate(RECENT_LIMIT);
    ranked
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
        source: None,
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
        source: None,
    }
}

fn recent_hit(entry: &RecentCommand, score: i32) -> Phase1Hit {
    Phase1Hit {
        id: entry.id.clone(),
        kind: Phase1Kind::Recent,
        title: entry.title.clone(),
        subtitle: "Recent".to_string(),
        score,
        app_id: None,
        path: None,
        hwnd: None,
        action_id: None,
        uri: None,
        icon_key: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, source: &str) -> AppEntry {
        AppEntry {
            name: name.to_string(),
            normalized_name: name
                .to_lowercase()
                .replace(|ch: char| !ch.is_ascii_alphanumeric(), ""),
            app_id: format!("id-{name}"),
            icon: None,
            path: Some(format!("C:\\Apps\\{name}.exe")),
            args: None,
            working_directory: None,
            location: None,
            aumid: None,
            source: source.to_string(),
            keywords: Vec::new(),
        }
    }

    #[test]
    fn eyes_hurt_pins_night_light() {
        let store = IntentStore::new();
        let response = phase1("my eyes hurt", &[], &[], &[], &store);
        assert_eq!(
            response.intent_action_id.as_deref(),
            Some("settings.nightlight")
        );
        assert_eq!(
            response.actions[0].action_id.as_deref(),
            Some("settings.nightlight")
        );
        assert_eq!(response.actions[0].score, 10_000);
    }

    #[test]
    fn kill_audio_pins_mute() {
        let store = IntentStore::new();
        let response = phase1("kill audio", &[], &[], &[], &store);
        assert_eq!(response.actions[0].action_id.as_deref(), Some("audio.mute"));
    }

    #[test]
    fn clean_space_pins_storage_sense() {
        let store = IntentStore::new();
        let response = phase1("clean space", &[], &[], &[], &store);
        assert_eq!(
            response.actions[0].action_id.as_deref(),
            Some("settings.storagepolicies")
        );
    }

    #[test]
    fn start_menu_apps_rank_above_generic_sources() {
        let store = IntentStore::new();
        let apps = vec![app("Terminal", "programs"), app("Terminal", "startMenu")];
        let response = phase1("term", &[], &apps, &[], &store);
        assert_eq!(response.apps[0].source.as_deref(), Some("startMenu"));
        assert_eq!(response.apps.len(), 1);
    }

    #[test]
    fn recents_and_windows_are_included() {
        let store = IntentStore::new();
        let recents = vec![RecentCommand {
            id: "app::id-Chrome".into(),
            title: "Chrome".into(),
        }];
        let windows = vec![OpenWindow {
            hwnd: 42,
            title: "Chrome".into(),
            process_name: "chrome".into(),
        }];
        let response = phase1("chr", &recents, &[], &windows, &store);
        assert_eq!(response.recents[0].id, "app::id-Chrome");
        assert_eq!(response.windows[0].hwnd, Some(42));
    }

    #[test]
    fn empty_query_returns_nothing() {
        let store = IntentStore::new();
        let response = phase1("   ", &[], &[], &[], &store);
        assert!(response.actions.is_empty());
        assert!(response.apps.is_empty());
    }

    #[test]
    fn execute_rejects_unknown_ids() {
        assert!(execute("not.real").is_err());
    }
}
