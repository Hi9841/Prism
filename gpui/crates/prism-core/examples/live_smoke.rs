//! Live smoke run of the read-only backend calls that the UI makes.
//!
//! `cargo run --example live_smoke -- <data folder>`
//!
//! The data folder must be a COPY of `%APPDATA%\app.prism.launcher`: the app
//! scan writes `apps.json` and `icons\`, and opening `catalog.db` runs schema
//! work. The runner refuses the real folder. Each call prints one JSON line:
//! `{"fn", "input", "ok", "ms", "summary"}` (or `"error"` when `ok` is false).
//!
//! It never calls a setter, a launch, a power action, or `start()`.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use prism_core::backend::backend;
use prism_core::files::FileIndex;
use prism_core::runtime::block_on;
use serde_json::{json, Value};
use windows::Win32::Foundation::{POINT, RECT};

/// A host with a redirected data folder. It records events and does nothing else.
struct SmokeHost {
    data_dir: PathBuf,
    events: Mutex<Vec<String>>,
    changed: Condvar,
}

impl prism_core::host::Host for SmokeHost {
    fn emit(&self, event: &str, _payload: Value) {
        self.events.lock().unwrap().push(event.to_owned());
        self.changed.notify_all();
    }
    fn app_data_dir(&self) -> Result<PathBuf, String> {
        Ok(self.data_dir.clone())
    }
    fn palette_visible(&self) -> bool {
        false
    }
    fn toggle_from_win(&self, _start_button: Option<RECT>) {}
    fn toggle_from_taskbar(&self, _click: POINT, _start_button: Option<RECT>) {}
    fn activate(&self) {}
    fn register_shortcut(&self, _combo: &str) -> Result<(), String> {
        Err("the smoke host registers no shortcut".into())
    }
    fn unregister_shortcut(&self, _combo: &str) {}
}

impl SmokeHost {
    fn wait_for(&self, event: &str, timeout: Duration) -> bool {
        let events = self.events.lock().unwrap();
        let (events, _) = self
            .changed
            .wait_timeout_while(events, timeout, |events| !events.iter().any(|e| e == event))
            .unwrap();
        events.iter().any(|e| e == event)
    }
}

fn report<T>(
    function: &str,
    input: Value,
    call: impl FnOnce() -> Result<T, String>,
    summary: impl FnOnce(&T) -> Value,
) -> Option<T> {
    let started = Instant::now();
    let result = call();
    let ms = (started.elapsed().as_secs_f64() * 10_000.0).round() / 10.0;
    let (line, value) = match result {
        Ok(value) => (
            json!({ "fn": function, "input": input, "ok": true, "ms": ms, "summary": summary(&value) }),
            Some(value),
        ),
        Err(error) => (
            json!({ "fn": function, "input": input, "ok": false, "ms": ms, "error": error }),
            None,
        ),
    };
    println!("{line}");
    value
}

fn titles(hits: &Value, take: usize) -> Value {
    let list = hits.as_array().map(Vec::as_slice).unwrap_or_default();
    json!({
        "n": list.len(),
        "first": list.iter().take(take).map(|hit| json!([hit["title"], hit["score"], hit["actionId"].as_str().or(hit["source"].as_str()).or(hit["subtitle"].as_str())])).collect::<Vec<_>>(),
    })
}

fn phase1_summary(response: &Value) -> Value {
    json!({
        "query": response["query"],
        "intent": response["intentActionId"],
        "recents": titles(&response["recents"], 2),
        "windows": titles(&response["windows"], 2),
        "apps": titles(&response["apps"], 3),
        "actions": titles(&response["actions"], 4),
    })
}

fn files_summary(response: &prism_core::files::FileSearchResponse) -> Value {
    json!({
        "items": response.items.len(),
        "first": response.items.iter().take(3).map(|item| &item.path).collect::<Vec<_>>(),
        "ready": response.ready,
        "indexing": response.indexing,
        "pathBrowse": response.path_browse,
        "totalIndexed": response.total_indexed,
        "volumes": response.volumes.iter().map(|volume| json!([volume.drive, volume.state, volume.indexed_count])).collect::<Vec<_>>(),
        "error": response.error.as_ref().map(|error| &error.message),
    })
}

fn main() {
    let data_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: live_smoke <copy of the data folder>"),
    );
    let data_dir = std::fs::canonicalize(&data_dir).expect("the data folder must exist");
    let real = std::env::var_os("APPDATA")
        .map(|appdata| PathBuf::from(appdata).join("app.prism.launcher"));
    if real
        .and_then(|real| std::fs::canonicalize(real).ok())
        .is_some_and(|real| real == data_dir)
    {
        eprintln!(
            "refused: {} is the real data folder; pass a copy",
            data_dir.display()
        );
        std::process::exit(2);
    }
    let host = Arc::new(SmokeHost {
        data_dir: data_dir.clone(),
        events: Mutex::new(Vec::new()),
        changed: Condvar::new(),
    });
    prism_core::host::install(host.clone());
    let core = backend();
    let home = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
    let at = |relative: &str| home.join(relative).to_string_lossy().into_owned();

    report(
        "load_state",
        json!(null),
        || core.load_state(),
        |state| {
            json!({
                "version": state["version"],
                "settings": state["settings"].as_object().map(|settings| settings.keys().cloned().collect::<Vec<_>>()),
                "history": state["history"].as_array().map(Vec::len),
            })
        },
    );

    let apps = report(
        "get_apps",
        json!(null),
        || block_on(core.get_apps()),
        |apps| {
            let mut sources = std::collections::BTreeMap::<&str, usize>::new();
            for app in apps {
                *sources.entry(app.source.as_str()).or_default() += 1;
            }
            json!({
                "n": apps.len(),
                "withIcon": apps.iter().filter(|app| app.icon.is_some()).count(),
                "first": apps.iter().take(3).map(|app| &app.name).collect::<Vec<_>>(),
                "sources": sources,
            })
        },
    )
    .unwrap_or_default();

    let find = |needle: &str| apps.iter().find(|app| app.normalized_name.contains(needle));
    let mut icon_ids: Vec<String> = apps.iter().take(3).map(|app| app.app_id.clone()).collect();
    icon_ids.extend(find("chrome").map(|app| app.app_id.clone()));
    icon_ids.push("no-such-app-id".into());
    let icon_input = json!(icon_ids);
    report(
        "get_app_icons",
        icon_input,
        || core.get_app_icons(icon_ids),
        |icons| {
            json!({
                "n": icons.len(),
                "allPng": icons.values().all(|icon| icon.starts_with("data:image/png;base64,")),
                "bytes": icons.values().map(String::len).sum::<usize>(),
            })
        },
    );

    // `start()` warms these two catalogs; Phase 1 reads only the warm copies.
    report(
        "windows_tools::warm (part of start)",
        json!(null),
        || {
            prism_core::windows_tools::warm();
            Ok(())
        },
        |_| json!({ "tools": prism_core::windows_tools::cached().map_or(0, |tools| tools.len()) }),
    );
    report(
        "settings_index::warm (part of start)",
        json!(null),
        || {
            prism_core::settings_index::warm();
            Ok(())
        },
        |_| json!({ "settings": prism_core::settings_index::cached().map_or(0, |settings| settings.len()) }),
    );

    let phase1_queries = [
        "chrome",
        "c",
        "settings",
        "display",
        "wifi",
        "bluetooth",
        "2+2*3",
        "sqrt(16)",
        "10 km to mi",
        "shutdown",
        "restart",
        "lock",
        "night light",
        "xyzzy-no-match",
        "HyperType-snippets.json",
        "PaperDraw",
        "",
    ];
    for query in phase1_queries {
        report(
            "query_phase1",
            json!({ "query": query, "recent": [] }),
            || {
                serde_json::to_value(core.query_phase1(query.into(), Vec::new()))
                    .map_err(|e| e.to_string())
            },
            phase1_summary,
        );
    }
    let recent: Vec<prism_core::query::RecentCommand> =
        serde_json::from_value(json!([{ "id": "action::power.lock", "title": "Lock" }, { "id": "x", "title": "Chrome Canary" }])).unwrap();
    report(
        "query_phase1",
        json!({ "query": "lo", "recent": "[Lock, Chrome Canary]" }),
        || serde_json::to_value(core.query_phase1("lo".into(), recent)).map_err(|e| e.to_string()),
        phase1_summary,
    );

    for query in ["power plan", "balanced", "xyzzy"] {
        report(
            "query_power_plans",
            json!(query),
            || block_on(core.query_power_plans(query.into())),
            |hits| {
                json!(hits
                    .iter()
                    .map(|hit| json!([hit.name, hit.active, hit.score, hit.picker_intent]))
                    .collect::<Vec<_>>())
            },
        );
    }

    // The backend's own index is opened by `start()`, which this runner never
    // calls, so `search_files` here shows the pre-warm path (direct browse).
    for query in [at("Documents\\"), "chrome".into(), String::new()] {
        report(
            "search_files (index not opened)",
            json!(query),
            || block_on(core.search_files(query.clone(), Some(50))),
            files_summary,
        );
    }

    // The same search body on the copied catalog (Backend::search_files runs
    // FileIndex::search_with_generation on a blocking thread).
    let index = FileIndex::default();
    report(
        "FileIndex::init (copy)",
        json!(data_dir.join("catalog.db")),
        || Ok(index.init(&data_dir)),
        |opened| json!({ "opened": opened }),
    );
    report(
        "wait for name index",
        json!("file-index-updated"),
        || Ok(host.wait_for("file-index-updated", Duration::from_secs(300))),
        |loaded| json!({ "loaded": loaded }),
    );
    let file_queries = [
        "HyperType-snippets.json".to_owned(),
        "Untitled-1.txt".into(),
        "PaperDraw".into(),
        "Client Work".into(),
        "chrome".into(),
        "report".into(),
        "c".into(),
        "xyzzy-no-match".into(),
        String::new(),
        "desktop/skills/".into(),
        at("Pictures\\"),
    ];
    for query in file_queries {
        report(
            "search_files (copied catalog)",
            json!(query),
            || Ok(index.search_with_generation(&query, Some(50), index.begin_search())),
            files_summary,
        );
    }

    let thumbnails = vec![
        at("Pictures\\2image.png"),
        at("Pictures\\abstract_hills_landscape_1787170425908.jpg"),
        at("Documents\\Untitled-1.txt"),
        at("Pictures\\missing.png"),
    ];
    let thumbnails_input = json!(thumbnails);
    report(
        "get_file_thumbnails",
        thumbnails_input,
        || Ok(block_on(core.get_file_thumbnails(thumbnails))),
        |list| {
            json!(list
                .iter()
                .map(|thumbnail| thumbnail.as_ref().map(|data| data.len()))
                .collect::<Vec<_>>())
        },
    );

    report(
        "get_quick_access",
        json!(null),
        || Ok(block_on(core.get_quick_access())),
        |list| {
            json!(list
                .iter()
                .map(|entry| [&entry.kind, &entry.path])
                .collect::<Vec<_>>())
        },
    );

    let paths = vec![
        at("Documents"),
        at("Desktop\\Prism.lnk"),
        at("Documents\\no-such-file.txt"),
        "relative\\path".into(),
        String::new(),
    ];
    let paths_input = json!(paths);
    report(
        "existing_paths",
        paths_input,
        || Ok(block_on(core.existing_paths(paths))),
        |found| json!(found),
    );

    // The UI asks with the shortcut location first, then the target (sections.rs).
    let mut pin_paths: Vec<String> = ["chrome", "wezterm", "steam"]
        .iter()
        .filter_map(|needle| {
            find(needle).and_then(|app| app.location.clone().or_else(|| app.path.clone()))
        })
        .collect();
    pin_paths.push("relative.exe".into());
    for path in pin_paths {
        report(
            "is_pinned_to_taskbar",
            json!(path),
            || Ok(block_on(core.is_pinned_to_taskbar(path.clone()))),
            |pinned| json!(pinned),
        );
    }

    report(
        "get_taskbar_settings",
        json!(null),
        || block_on(core.get_taskbar_settings()),
        |settings| {
            let mut value = serde_json::to_value(settings).unwrap_or_default();
            if let Some(icons) = value["customStartIcons"].as_array_mut() {
                for icon in icons {
                    icon["preview"] = json!(icon["preview"].as_str().map_or(0, str::len));
                }
            }
            value
        },
    );

    report(
        "get_system_theme",
        json!(null),
        || Ok(core.get_system_theme()),
        |theme| json!(theme),
    );

    let events = host.events.lock().unwrap().clone();
    println!("{}", json!({ "fn": "host events", "summary": events }));
}
