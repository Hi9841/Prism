//! Two-phase palette query.
//!
//! Phase 1 is in-process: open windows, Start Menu / UWP apps,
//! and the typed action catalog (plus Jev phrase mapping).
//! Phase 2 is SQLite FTS5 file search and stays on `search_files`.

mod catalog;
mod jev;
mod launches;
mod open_windows;
pub mod score;

use crate::apps::AppEntry;
use serde::Serialize;

pub use jev::IntentStore;
pub use launches::LaunchStore;
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
    launches: &LaunchStore,
) -> Phase1Response {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return empty_phase1("");
    }
    let query_lower = trimmed.to_lowercase();
    let intent_action_id = jev::resolve(&query_lower, intent).map(str::to_string);

    // Prepared once: one keystroke scores it against every title and keyword.
    let prepared = score::Query::new(&query_lower);
    let window_hits = open_windows::search(&prepared, windows, WINDOW_LIMIT)
        .into_iter()
        .map(|(window, score)| window_hit(&window, score))
        .collect();
    let app_hits = search_apps(&prepared, apps, launches);
    let actions = search_actions(&prepared, intent_action_id.as_deref());

    Phase1Response {
        query: trimmed.to_string(),
        windows: window_hits,
        apps: app_hits,
        actions,
        intent_action_id,
    }
}

/// A source of action hits. Phase 1 search, `execute` and the backend start
/// all walk `ACTION_SOURCES`, so a new source is one entry there. The ids of
/// the sources do not overlap (`settings.*`, `windows.*` / `control-panel::*`,
/// `settings-index::*`).
pub(crate) struct ActionSource {
    /// Builds the source's index at start, off the UI thread.
    pub(crate) warm: Option<fn()>,
    /// Adds the source's hits for one query.
    search: fn(&score::Query, &mut Vec<Phase1Hit>),
    /// Runs the source's action `id`; `None` when `id` is not the source's.
    execute: fn(&str) -> Option<Result<(), String>>,
}

/// In search order: on an equal score and title, the earlier source's hit stays.
pub(crate) const ACTION_SOURCES: [ActionSource; 3] = [
    ActionSource { warm: None, search: search_catalog, execute: execute_catalog },
    ActionSource { warm: Some(crate::windows_tools::warm), search: search_tools, execute: crate::windows_tools::execute },
    ActionSource {
        warm: Some(crate::settings_index::warm),
        search: search_settings,
        execute: crate::settings_index::execute,
    },
];

pub fn execute(action_id: &str) -> Result<(), String> {
    ACTION_SOURCES
        .iter()
        .find_map(|source| (source.execute)(action_id))
        .unwrap_or_else(|| Err(format!("unknown Windows tool '{action_id}'")))
}

fn execute_catalog(action_id: &str) -> Option<Result<(), String>> {
    catalog::get(action_id).map(catalog::execute)
}

fn search_catalog(query: &score::Query, hits: &mut Vec<Phase1Hit>) {
    hits.extend(catalog::search(query, ACTION_LIMIT).into_iter().map(|(action, score)| action_hit(action, score, false)));
}

fn search_tools(query: &score::Query, hits: &mut Vec<Phase1Hit>) {
    if let Some(tools) = crate::windows_tools::cached() {
        for tool in tools.iter() {
            if let Some(score) = query.score_with_keywords(&tool.title, &tool.keywords) {
                hits.push(tool_hit(tool, score));
            }
        }
    }
}

fn search_settings(query: &score::Query, hits: &mut Vec<Phase1Hit>) {
    if let Some(settings) = crate::settings_index::cached() {
        for setting in settings.iter() {
            let title = query.score(&setting.title);
            let keyword = query
                .score_with_keywords("", &setting.keywords)
                .map(|score| score.min(SETTING_KEYWORD_CAP));
            if let Some(score) = title.max(keyword) {
                hits.push(setting_hit(setting, score));
            }
        }
    }
}

pub fn accept_intent(query: &str, action_id: &str, store: &IntentStore) -> Result<(), String> {
    store.remember(query, action_id)
}

pub fn focus_window(hwnd: i64) -> Result<(), String> {
    open_windows::focus(hwnd)
}

fn search_actions(query: &score::Query, intent_id: Option<&str>) -> Vec<Phase1Hit> {
    let mut hits: Vec<Phase1Hit> = Vec::new();
    for source in &ACTION_SOURCES {
        (source.search)(query, &mut hits);
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

/// The match tier of a score: 1,000 is a whole title, 9 a title prefix, 8 a
/// substring or a keyword, and lower tiers are fuzzy.
fn match_tier(score: i32) -> i32 {
    score.div_euclid(100)
}

/// Apps in one match tier go by launch count first, then by score. A launched
/// app never passes an app in a higher tier.
fn search_apps(query: &score::Query, apps: &[AppEntry], launches: &LaunchStore) -> Vec<Phase1Hit> {
    let mut ranked: Vec<(&AppEntry, i32, i32)> = Vec::new();
    for app in apps {
        let Some(score) = query.score_with_keywords(&app.name, &app.keywords) else {
            continue;
        };
        ranked.push((app, score, prism_vocab::source_rank(&app.source)));
    }
    ranked.sort_by(|a, b| {
        a.0.normalized_name
            .cmp(&b.0.normalized_name)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| b.1.cmp(&a.1))
    });
    ranked.dedup_by(|a, b| a.0.normalized_name == b.0.normalized_name);
    let mut boosted: Vec<(&AppEntry, i32, i32, u32)> = ranked
        .into_iter()
        .map(|(app, score, source)| (app, score, source, launches.boost(&app.app_id)))
        .collect();
    boosted.sort_by(|a, b| {
        match_tier(b.1)
            .cmp(&match_tier(a.1))
            .then_with(|| b.3.cmp(&a.3))
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| b.2.cmp(&a.2))
    });
    boosted.truncate(APP_LIMIT);
    boosted
        .into_iter()
        .map(|(app, score, _, _)| app_hit(app, score))
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
        source: Some(prism_vocab::WINDOWS_TOOL_SOURCE.to_string()),
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
        source: task.then(|| prism_vocab::WINDOWS_TOOL_SOURCE.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, keywords: &[&str]) -> AppEntry {
        AppEntry {
            name: name.to_string(),
            normalized_name: name.to_lowercase(),
            app_id: format!("id-{name}"),
            icon: None,
            path: None,
            args: None,
            working_directory: None,
            location: None,
            aumid: None,
            source: "startMenu".to_string(),
            keywords: keywords.iter().map(|word| word.to_string()).collect(),
        }
    }

    fn ranked(query: &str, apps: &[AppEntry], launches: &LaunchStore) -> Vec<String> {
        let prepared = score::Query::new(query);
        search_apps(&prepared, apps, launches)
            .into_iter()
            .map(|hit| hit.title)
            .collect()
    }

    fn launched(names: &[(&str, usize)]) -> LaunchStore {
        let store = LaunchStore::new();
        for (name, times) in names {
            for _ in 0..*times {
                store.record(&format!("id-{name}")).unwrap();
            }
        }
        store
    }

    #[test]
    fn a_launched_app_beats_an_unlaunched_app_in_the_same_tier() {
        // Both titles start with "note": the same tier and the same score.
        let apps = [app("Notepad", &[]), app("Notes", &[])];
        assert_eq!(ranked("note", &apps, &LaunchStore::new()), ["Notepad", "Notes"]);
        assert_eq!(ranked("note", &apps, &launched(&[("Notes", 1)])), ["Notes", "Notepad"]);
    }

    #[test]
    fn more_launches_rank_higher_in_a_tier() {
        let apps = [app("Notepad", &[]), app("Notes", &[]), app("Notion", &[])];
        let store = launched(&[("Notepad", 1), ("Notes", 7), ("Notion", 3)]);
        assert_eq!(ranked("no", &apps, &store), ["Notes", "Notion", "Notepad"]);
    }

    #[test]
    fn a_launched_app_never_passes_a_whole_title_match() {
        let apps = [app("Code", &[]), app("Code Helper", &[])];
        let store = launched(&[("Code Helper", 500)]);
        assert_eq!(ranked("code", &apps, &store), ["Code", "Code Helper"]);
    }

    #[test]
    fn a_launched_keyword_hit_never_passes_a_title_prefix() {
        let apps = [app("Terminal", &[]), app("Console Host", &["terminal"])];
        let store = launched(&[("Console Host", 500)]);
        assert_eq!(ranked("termi", &apps, &store), ["Terminal", "Console Host"]);
    }

    #[test]
    fn a_launched_substring_match_never_passes_a_title_prefix() {
        let apps = [app("Paint", &[]), app("Microsoft Paint 3D", &[])];
        let store = launched(&[("Microsoft Paint 3D", 500)]);
        assert_eq!(ranked("pain", &apps, &store), ["Paint", "Microsoft Paint 3D"]);
    }

    #[test]
    fn the_score_in_the_hit_does_not_change() {
        let apps = [app("Notepad", &[])];
        let prepared = score::Query::new("note");
        let plain = search_apps(&prepared, &apps, &LaunchStore::new());
        let boosted = search_apps(&prepared, &apps, &launched(&[("Notepad", 63)]));
        assert_eq!(plain[0].score, boosted[0].score);
    }
}
