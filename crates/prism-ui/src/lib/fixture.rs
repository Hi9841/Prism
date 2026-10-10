//! Parity fixture: canned backend answers for side-by-side captures of the
//! original (headless Chrome with a mocked Tauri layer, `migration/parity`)
//! and the port. Port infrastructure; the shipping app never installs one.
//! Both sides read the same JSON, so they see the same data.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Fixture {
    pub version: String,
    pub system_theme: String,
    pub state: Value,
    pub apps: Vec<Value>,
    pub quick_access: Value,
    pub existing_paths: Vec<String>,
    pub thumbnails: HashMap<String, String>,
    pub files: HashMap<String, Value>,
    pub file_status: Value,
    pub phase1: HashMap<String, Value>,
    pub power_plans: HashMap<String, Value>,
    pub taskbar: Value,
    pub pinned_to_taskbar: Vec<String>,
}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

pub fn install(fixture: Fixture) {
    let _ = FIXTURE.set(fixture);
}

pub fn get() -> Option<&'static Fixture> {
    FIXTURE.get()
}

impl Fixture {
    /// `get_apps`: the list without icon payloads.
    pub fn apps_without_icons(&self) -> Value {
        Value::Array(
            self.apps
                .iter()
                .map(|app| {
                    let mut app = app.clone();
                    if let Some(map) = app.as_object_mut() {
                        map.remove("icon");
                    }
                    app
                })
                .collect(),
        )
    }

    pub fn app_icons(&self, ids: &[String]) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for app in &self.apps {
            let id = app.get("appId").and_then(Value::as_str).unwrap_or_default();
            if ids.iter().any(|candidate| candidate == id)
                && let Some(icon) = app.get("icon").and_then(Value::as_str)
            {
                out.insert(id.to_owned(), icon.to_owned());
            }
        }
        out
    }

    fn status(&self) -> Value {
        if self.file_status.is_null() {
            json!({"ready": true, "indexing": false, "volumes": [{"drive": "C:\\", "state": "ready", "indexedCount": 1}], "totalIndexed": 1})
        } else {
            self.file_status.clone()
        }
    }

    /// `search_files`: the canned response for the normalized query, else no matches.
    pub fn search_files(&self, query: &str) -> Value {
        let key = crate::js::lower(crate::js::trim(query));
        let status = self.status();
        let mut response = self.files.get(&key).cloned().unwrap_or_else(|| json!({ "items": [], "pathBrowse": false }));
        if let (Some(response), Some(status)) = (response.as_object_mut(), status.as_object()) {
            for (k, v) in status {
                response.entry(k.clone()).or_insert(v.clone());
            }
            response.entry("items").or_insert(json!([]));
            response.entry("pathBrowse").or_insert(json!(false));
        }
        response
    }

    pub fn phase1(&self, query: &str) -> Value {
        let key = crate::js::lower(crate::js::trim(query));
        let mut response = self
            .phase1
            .get(&key)
            .cloned()
            .unwrap_or_else(|| json!({"recents": [], "windows": [], "apps": [], "actions": []}));
        response["query"] = json!(query);
        response
    }

    pub fn power_plans(&self, query: &str) -> Value {
        let key = crate::js::lower(crate::js::trim(query));
        self.power_plans.get(&key).cloned().unwrap_or_else(|| json!([]))
    }

    pub fn thumbnails(&self, paths: &[String]) -> Vec<Option<String>> {
        paths.iter().map(|p| self.thumbnails.get(p).cloned()).collect()
    }

    pub fn existing(&self, paths: &[String]) -> Vec<String> {
        paths.iter().filter(|p| self.existing_paths.contains(p)).cloned().collect()
    }
}

// PORT STATUS: confidence=high todos=0
