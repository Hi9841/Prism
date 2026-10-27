//! Phase 1 query payloads, shaped as the backend sends them (`src/lib/query.ts`).

use serde::{Deserialize, Serialize};

use crate::js;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentCommand {
    pub id: String,
    pub title: String,
}

/// One Phase 1 hit. `kind` is "recent" | "window" | "app" | "action".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Phase1Hit {
    pub id: String,
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hwnd: Option<f64>,
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PowerPlanHit {
    pub guid: String,
    pub name: String,
    pub active: bool,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picker_intent: Option<bool>,
}

/// `EMPTY_PHASE1` is `Phase1Response::default()`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Phase1Response {
    pub query: String,
    pub recents: Vec<Phase1Hit>,
    pub windows: Vec<Phase1Hit>,
    pub apps: Vec<Phase1Hit>,
    pub actions: Vec<Phase1Hit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_action_id: Option<String>,
}

pub fn phase1_matches_query(phase1: Option<&Phase1Response>, query: &str) -> bool {
    let Some(phase1) = phase1 else {
        return false;
    };
    js::lower(js::trim(&phase1.query)) == js::lower(js::trim(query))
}

// PORT STATUS: confidence=high todos=0
