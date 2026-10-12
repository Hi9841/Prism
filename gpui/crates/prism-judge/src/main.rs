//! Judge adapter for the port: one JSON call on stdin, one JSON result on stdout.
//! The contract (ops, input markers, number encoding, action records) is the
//! header of `migration/judge/adapter.mjs`. No expected values live here.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::process::ExitCode;

use prism_ui::features::palette::sections::{
    AppGroupSection, PaletteSources, Section, build_sections, is_clipboard_kind, quick_access_palette_item,
};
use prism_ui::features::updater::update_policy::{
    MIN_BACKGROUND_CHECK_INTERVAL_MS, MIN_FORCED_CHECK_INTERVAL_MS, should_check_for_update,
};
use prism_ui::features::updater::update_progress::update_percent;
use prism_ui::js;
use prism_ui::lib::icon_loading::{AppIconRequestState, app_icon_retry_delay, select_app_icon_request_ids};
use prism_ui::lib::launcher_motion::{
    HideOrigin, LauncherEvent, LauncherPhase, hide_palette_invocation, reduce_launcher_phase,
};
use prism_ui::lib::math::{format_number, is_math_like, try_evaluate};
use prism_ui::lib::query::{Phase1Response, PowerPlanHit, phase1_matches_query};
use prism_ui::lib::search::{Searchable, dedupe_apps, fuzzy, fuzzy_apps, fuzzy_score};
use prism_ui::lib::types::{
    AppEntry, AppGroup, FileEntry, HistoryEntry, Invoke, PaletteIcon, PaletteItem, QuickAccessEntry, RowAction,
    VolumeCoverage, is_elevatable_path, is_picture_path, is_taskbar_pinable_path, reorder_app_groups,
    reorder_pinned_apps, reorder_quick_access, reorder_sections, step_view_zoom,
};
use prism_ui::state::app::{sanitize_history, sanitize_settings};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

type Failure = String;

fn main() -> ExitCode {
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("stdin: {error}");
        return ExitCode::from(2);
    }
    let result = serde_json::from_str::<Value>(&input).map_err(|e| format!("bad call: {e}")).and_then(|call| run(&call));
    match result {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

/// Numbers JSON cannot carry are `{"$num": ...}` (adapter contract).
fn num(n: f64) -> Value {
    if n.is_nan() {
        json!({ "$num": "NaN" })
    } else if n.is_infinite() {
        json!({ "$num": if n > 0.0 { "Infinity" } else { "-Infinity" } })
    } else if n == 0.0 && n.is_sign_negative() {
        json!({ "$num": "-0" })
    } else {
        json!(n)
    }
}

fn opt_num(n: Option<f64>) -> Value {
    n.map_or(Value::Null, num)
}

fn field<T: DeserializeOwned>(call: &Value, key: &str) -> Result<T, Failure> {
    serde_json::from_value(call.get(key).cloned().unwrap_or(Value::Null)).map_err(|e| format!("{key}: {e}"))
}

/// Like `field`, but `undefined` and `null` give the type's default.
fn field_or_default<T: DeserializeOwned + Default>(call: &Value, key: &str) -> Result<T, Failure> {
    match call.get(key) {
        None | Some(Value::Null) => Ok(T::default()),
        Some(_) => field(call, key),
    }
}

fn number(call: &Value, key: &str) -> Result<f64, Failure> {
    call.get(key).and_then(Value::as_f64).ok_or_else(|| format!("{key}: expected a number"))
}

fn string<'a>(call: &'a Value, key: &str) -> Result<&'a str, Failure> {
    call.get(key).and_then(Value::as_str).ok_or_else(|| format!("{key}: expected a string"))
}

fn truthy(call: &Value, key: &str) -> bool {
    js::truthy(call.get(key))
}

#[derive(Deserialize)]
struct PoolItem {
    id: String,
    title: String,
    keywords: Option<Vec<String>>,
}

impl Searchable for PoolItem {
    fn title(&self) -> &str {
        &self.title
    }

    fn keywords(&self) -> Option<&[String]> {
        self.keywords.as_deref()
    }
}

fn run(call: &Value) -> Result<Value, Failure> {
    let op = string(call, "op")?;
    Ok(match op {
        "math.tryEvaluate" => match try_evaluate(string(call, "input")?) {
            None => json!({ "returned": null }),
            Some(evaluated) => json!({ "returned": "value", "value": num(evaluated.value) }),
        },
        "math.formatNumber" => {
            let input = match call.get("input") {
                Some(Value::String(s)) if s == "inf" => f64::INFINITY,
                _ => number(call, "input")?,
            };
            json!({ "text": format_number(input) })
        }
        "math.isMathLike" => json!({ "value": is_math_like(string(call, "input")?) }),
        "search.fuzzyScore" => {
            let pairs: Vec<(String, String)> = field(call, "pairs")?;
            let scores: Vec<Value> = pairs.iter().map(|(q, t)| opt_num(fuzzy_score(q, t))).collect();
            json!({ "scores": scores })
        }
        "search.fuzzy" => {
            let pool: Vec<PoolItem> = field(call, "pool")?;
            let hits = fuzzy(&pool, string(call, "query")?, field(call, "limit")?);
            json!({ "ids": hits.iter().map(|hit| &hit.item.id).collect::<Vec<_>>() })
        }
        "search.fuzzyApps" => {
            let apps: Vec<AppEntry> = field(call, "apps")?;
            let hits = fuzzy_apps(&apps, string(call, "query")?, field(call, "limit")?, false);
            json!({
                "names": hits.iter().map(|app| &app.name).collect::<Vec<_>>(),
                "appIds": hits.iter().map(|app| &app.app_id).collect::<Vec<_>>(),
            })
        }
        "search.dedupeApps" => {
            let apps: Vec<AppEntry> = field(call, "apps")?;
            json!({ "appIds": dedupe_apps(&apps).iter().map(|app| &app.app_id).collect::<Vec<_>>() })
        }
        "types.pathFlags" => {
            let path: Option<String> = field(call, "path")?;
            let path = path.as_deref();
            json!({
                "elevatable": is_elevatable_path(path),
                "pinable": is_taskbar_pinable_path(path),
                "picture": is_picture_path(path),
            })
        }
        "types.stepViewZoom" => {
            let direction = number(call, "direction")? as i32;
            json!({ "value": num(step_view_zoom(number(call, "current")?, direction)) })
        }
        "types.reorderPinned" | "types.reorderQuick" | "types.reorderSections" => {
            let ids: Vec<String> = field(call, "ids")?;
            let (source, target) = (string(call, "source")?, string(call, "target")?);
            let reorder = match op {
                "types.reorderPinned" => reorder_pinned_apps,
                "types.reorderQuick" => reorder_quick_access,
                _ => reorder_sections,
            };
            json!({ "result": reorder(&ids, source, target), "input": ids })
        }
        "types.reorderGroups" => {
            let groups: Vec<AppGroup> = field(call, "groups")?;
            let reordered = reorder_app_groups(&groups, string(call, "source")?, string(call, "target")?);
            json!({
                "ids": reordered.iter().map(|g| &g.id).collect::<Vec<_>>(),
                "inputIds": groups.iter().map(|g| &g.id).collect::<Vec<_>>(),
            })
        }
        "query.phase1Matches" => {
            let phase1 = (!truthy(call, "nullPhase1"))
                .then(|| Ok::<_, Failure>(Phase1Response { query: string(call, "phase1Query")?.into(), ..Default::default() }))
                .transpose()?;
            json!({ "value": phase1_matches_query(phase1.as_ref(), string(call, "typed")?) })
        }
        "launcher.reduce" => {
            let phase = LauncherPhase::parse(string(call, "phase")?).ok_or("unknown phase")?;
            let event = LauncherEvent::parse(string(call, "event")?).ok_or("unknown event")?;
            json!({ "phase": reduce_launcher_phase(phase, event).as_str() })
        }
        "launcher.hide" => {
            let origin = HideOrigin::parse(string(call, "origin")?).ok_or("unknown origin")?;
            let phase = LauncherPhase::parse(string(call, "phase")?).ok_or("unknown phase")?;
            json!({ "invocation": hide_palette_invocation(origin, phase).as_str() })
        }
        "icons.select" => {
            let ids: Vec<String> = field(call, "ids")?;
            let raw = call.get("state").ok_or("state: missing")?;
            let icons: HashMap<String, String> = field_or_default(raw, "icons")?;
            let settled: HashSet<String> = field_or_default(raw, "settled")?;
            let in_flight: HashSet<String> = field_or_default(raw, "inFlight")?;
            let attempts: Vec<(String, u32)> = field_or_default(raw, "attempts")?;
            let attempts: HashMap<String, u32> = attempts.into_iter().collect();
            let state = AppIconRequestState {
                apps_loaded: truthy(raw, "appsLoaded"),
                icons: &icons,
                settled: &settled,
                in_flight: &in_flight,
                attempts: &attempts,
            };
            json!({ "ids": select_app_icon_request_ids(&ids, &state) })
        }
        "icons.delay" => json!({ "delay": opt_num(app_icon_retry_delay(number(call, "attempt")?)) }),
        "update.shouldCheck" => {
            let force = truthy(call, "force");
            let last = number(call, "last")?;
            let interval = if force { MIN_FORCED_CHECK_INTERVAL_MS } else { MIN_BACKGROUND_CHECK_INTERVAL_MS };
            let now = last + interval + number(call, "offsetMs")?;
            json!({ "value": should_check_for_update(last, now, force) })
        }
        "update.shouldCheckAt" => json!({
            "value": should_check_for_update(number(call, "last")?, number(call, "now")?, truthy(call, "force"))
        }),
        "update.percent" => {
            let total = call.get("total").and_then(Value::as_f64);
            json!({ "value": opt_num(update_percent(number(call, "downloaded")?, total)) })
        }
        "clipboard.kind" => json!({ "value": is_clipboard_kind(string(call, "id")?) }),
        "palette.sections" => palette_sections(call.get("sources").ok_or("sources: missing")?)?,
        "settings.sanitize" => {
            let raw = call.get("raw").ok_or("raw: missing")?;
            let value = if truthy(raw, "undefined") { None } else { Some(raw.get("value").unwrap_or(&Value::Null)) };
            json!({ "settings": sanitize_settings(value) })
        }
        "history.sanitize" => {
            let raw = call.get("raw").ok_or("raw: missing")?;
            let history: Vec<Value> = sanitize_history(raw.get("value"))
                .into_iter()
                .map(|entry| json!({ "id": entry.id, "title": entry.title, "ts": num(entry.ts) }))
                .collect();
            json!({ "history": history })
        }
        _ => return Err(format!("unknown op {op}")),
    })
}

/// Mirrors the provider (src/state/palette.tsx), as `paletteSources` in adapter.mjs does.
fn palette_sections(raw: &Value) -> Result<Value, Failure> {
    let settings = sanitize_settings(raw.get("settings"));
    let apps = dedupe_apps(&field::<Vec<AppEntry>>(raw, "apps")?);
    let apps_by_id: HashMap<&str, &AppEntry> = apps.iter().map(|entry| (entry.app_id.as_str(), entry)).collect();
    let quick_access: Vec<QuickAccessEntry> = field(raw, "quickAccess")?;
    let entries_by_kind: HashMap<&str, &QuickAccessEntry> =
        quick_access.iter().map(|entry| (entry.kind.as_str(), entry)).collect();
    let thumbnail_pairs: Vec<(String, Option<String>)> = field_or_default(raw, "fileThumbnails")?;
    let thumbnails: HashMap<String, Option<String>> = thumbnail_pairs.into_iter().collect();
    let query = string(raw, "query")?;
    let normalized = js::lower(js::trim(query));
    let short = js::len(&normalized) < 2;
    let files_searching = !short && truthy(raw, "filesSearching");
    let file_path_browse = !short && truthy(raw, "filePathBrowse");
    let file_index_ready = truthy(raw, "fileIndexReady");
    let file_indexing = truthy(raw, "fileIndexing");
    let files_error = if short || files_searching {
        false
    } else {
        truthy(raw, "responseError") || (!file_path_browse && !file_index_ready && !file_indexing)
    };
    let volumes: Vec<VolumeCoverage> = field_or_default(raw, "volumes")?;
    let file_results: Vec<FileEntry> = if short {
        Vec::new()
    } else {
        field::<Vec<FileEntry>>(raw, "fileResults")?
            .into_iter()
            .map(|mut entry| {
                if let Some(Some(thumbnail)) = thumbnails.get(&entry.path)
                    && !thumbnail.is_empty()
                {
                    entry.thumbnail = Some(thumbnail.clone());
                }
                entry
            })
            .collect()
    };
    let quick_items: Vec<PaletteItem> = settings
        .quick_access
        .iter()
        .filter_map(|kind| entries_by_kind.get(kind.as_str()))
        .map(|entry| quick_access_palette_item(entry))
        .collect();
    let history: Vec<HistoryEntry> = field(raw, "history")?;
    let existing_history_paths: HashSet<String> = field(raw, "existingHistoryPaths")?;
    let app_icons: HashMap<String, String> = field(raw, "appIcons")?;
    let file_result_query = if short { normalized.clone() } else { string(raw, "fileResultQuery")?.to_owned() };
    let phase1: Option<Phase1Response> = field(raw, "phase1")?;
    let power_plans: Vec<PowerPlanHit> = field(raw, "powerPlans")?;
    let replies: Map<String, Value> = field_or_default(raw, "replies")?;

    let sources = PaletteSources {
        query,
        apps: &apps,
        pinned_apps: settings.pinned_apps.iter().filter_map(|id| apps_by_id.get(id.as_str()).copied()).collect(),
        quick_items: &quick_items,
        quick_access_collapsed: settings.quick_access_collapsed,
        app_groups: &settings.app_groups,
        section_order: Some(&settings.section_order),
        pinned_app_ids: &settings.pinned_apps,
        history: &history,
        existing_history_paths: &existing_history_paths,
        file_thumbnails: &thumbnails,
        app_icons: &app_icons,
        file_results: &file_results,
        file_result_query: &file_result_query,
        file_path_browse,
        files_busy: files_searching || (!file_path_browse && !file_index_ready && file_indexing),
        files_error,
        file_indexing: Some(file_indexing || volumes.iter().any(|v| v.state == "indexing")),
        file_index_ready: Some(file_index_ready || volumes.iter().any(|v| v.state == "ready")),
        phase1: phase1.as_ref(),
        power_plans: &power_plans,
    };
    let built = build_sections(&sources);
    let sections: Vec<Value> = built.sections.iter().map(|section| section_json(section, &replies)).collect();
    Ok(json!({
        "sections": sections,
        "flatIds": built.flat_items.iter().map(|item| &item.id).collect::<Vec<_>>(),
    }))
}

/// Records backend calls against the scenario's `replies` instead of reaching a backend.
struct Recorder<'a> {
    replies: &'a Map<String, Value>,
    calls: Vec<Value>,
}

impl Invoke for Recorder<'_> {
    fn invoke(&mut self, cmd: &'static str, args: Value) -> Result<Value, String> {
        self.calls.push(json!({ "cmd": cmd, "args": args }));
        match self.replies.get(cmd) {
            Some(Value::Object(reply)) if reply.contains_key("$reject") => Err(match &reply["$reject"] {
                Value::String(message) => message.clone(),
                other => other.to_string(),
            }),
            Some(reply) => Ok(reply.clone()),
            None => Ok(Value::Null),
        }
    }

    fn write_clipboard(&mut self, text: &str) -> Result<(), String> {
        self.calls.push(json!({ "cmd": "clipboard.writeText", "args": { "text": text } }));
        Ok(())
    }
}

fn record(action: &RowAction, replies: &Map<String, Value>) -> Value {
    let mut recorder = Recorder { replies, calls: Vec::new() };
    let outcome = action.execute(&mut recorder);
    let mut out = json!({ "calls": recorder.calls });
    if let Err(message) = outcome {
        out["threw"] = Value::String(message);
    }
    out
}

fn icon_json(icon: &PaletteIcon) -> Value {
    match icon {
        PaletteIcon::Tile { icon, tint } => json!({ "kind": "tile", "name": icon.0, "tint": tint }),
        PaletteIcon::Emoji { char } => json!({ "kind": "emoji", "char": char }),
        PaletteIcon::Image { src, name } => json!({ "kind": "image", "src": src, "name": name }),
        PaletteIcon::App { name, icon } => {
            let mut out = json!({ "kind": "app", "name": name });
            if let Some(icon) = icon {
                out["icon"] = json!(icon);
            }
            out
        }
    }
}

fn item_json(item: &PaletteItem, replies: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            out.insert(key.into(), value);
        }
    };
    let action = |action: &Option<RowAction>| action.as_ref().map(|a| record(a, replies));
    put("id", Some(json!(item.id)));
    put("title", Some(json!(item.title)));
    put("subtitle", item.subtitle.as_ref().map(|s| json!(s)));
    put("keywords", item.keywords.as_ref().map(|k| json!(k)));
    put("icon", Some(icon_json(&item.icon)));
    put("run", Some(record(&item.run, replies)));
    put("runAsAdmin", action(&item.run_as_admin));
    put("openLocation", action(&item.open_location));
    put("shellPath", item.shell_path.as_ref().map(|s| json!(s)));
    put("toggleTaskbarPin", action(&item.toggle_taskbar_pin));
    put("showProperties", action(&item.show_properties));
    put("isPicture", item.is_picture.map(|b| json!(b)));
    put("dragFile", action(&item.drag_file));
    put("historyTitle", Some(json!(item.history_title)));
    put("toastDetail", item.toast_detail.as_ref().map(|s| json!(s)));
    put("appId", item.app_id.as_ref().map(|s| json!(s)));
    put("quickAccessKind", item.quick_access_kind.as_ref().map(|s| json!(s)));
    put("powerPlanGuid", item.power_plan_guid.as_ref().map(|s| json!(s)));
    Value::Object(out)
}

fn items_json(items: &[PaletteItem], replies: &Map<String, Value>) -> Vec<Value> {
    items.iter().map(|item| item_json(item, replies)).collect()
}

fn group_json(group: &AppGroupSection, replies: &Map<String, Value>) -> Value {
    json!({
        "groupId": group.group_id,
        "id": group.id,
        "label": group.label,
        "items": items_json(&group.items, replies),
        "collapsible": true,
        "collapsed": group.collapsed,
    })
}

fn section_json(section: &Section, replies: &Map<String, Value>) -> Value {
    let mut out = json!({
        "id": section.id,
        "label": section.label,
        "collapsible": section.collapsible.unwrap_or(false),
        "collapsed": section.collapsed.unwrap_or(false),
        "items": items_json(&section.items, replies),
    });
    if let Some(groups) = &section.groups {
        out["groups"] = Value::Array(groups.iter().map(|group| group_json(group, replies)).collect());
    }
    out
}
