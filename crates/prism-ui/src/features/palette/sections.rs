//! The palette's section policy: what results appear, grouped into which
//! sections, in what order, for an empty query and for a search query
//! (`src/features/palette/sections.ts`).
//!
//! Pure decision logic - no GPUI, no state. The palette entity feeds it a
//! `PaletteSources` snapshot and renders whatever sections come back.

use std::collections::{HashMap, HashSet};

use serde_json::json;

use crate::features::palette::phase1::{
    action_palette_item, app_hit_palette_item, is_command_action, power_plan_palette_item, window_palette_item,
};
use crate::js;
use crate::lib::emoji::sort_apps;
use crate::lib::math::{format_number, is_math_like, try_evaluate};
use crate::lib::query::{Phase1Response, PowerPlanHit, phase1_matches_query};
use crate::lib::search::{Searchable, fuzzy, fuzzy_apps};
use crate::lib::types::{
    AppEntry, AppGroup, FileEntry, HistoryEntry, LucideIcon, PaletteIcon, PaletteItem, QuickAccessEntry, RowAction,
    TileTint, is_elevatable_path, is_picture_path, is_taskbar_pinable_path,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub id: String,
    pub label: String,
    pub items: Vec<PaletteItem>,
    pub collapsible: Option<bool>,
    pub collapsed: Option<bool>,
    pub groups: Option<Vec<AppGroupSection>>,
}

impl Section {
    fn new(id: &str, label: &str, items: Vec<PaletteItem>) -> Self {
        Self { id: id.into(), label: label.into(), items, collapsible: None, collapsed: None, groups: None }
    }
}

/// An app collection inside the Apps section; always collapsible.
#[derive(Clone, Debug, PartialEq)]
pub struct AppGroupSection {
    pub group_id: String,
    pub id: String,
    pub label: String,
    pub items: Vec<PaletteItem>,
    pub collapsed: bool,
}

/// Everything the section policy may read, in one place.
pub struct PaletteSources<'a> {
    pub query: &'a str,
    /// Deduplicated app entries (the provider dedupes before calling).
    pub apps: &'a [AppEntry],
    /// Resolved pinned entries, in user order.
    pub pinned_apps: Vec<&'a AppEntry>,
    /// Resolved quick-access items, in user order.
    pub quick_items: &'a [PaletteItem],
    pub quick_access_collapsed: bool,
    pub app_groups: &'a [AppGroup],
    pub section_order: Option<&'a [String]>,
    /// `settings.pinnedApps` - ids that must not repeat in the Apps section.
    pub pinned_app_ids: &'a [String],
    pub history: &'a [HistoryEntry],
    pub existing_history_paths: &'a HashSet<String>,
    pub file_thumbnails: &'a HashMap<String, Option<String>>,
    pub app_icons: &'a HashMap<String, String>,
    pub file_results: &'a [FileEntry],
    pub file_result_query: &'a str,
    pub file_path_browse: bool,
    pub files_busy: bool,
    pub files_error: bool,
    pub file_indexing: Option<bool>,
    pub file_index_ready: Option<bool>,
    /// Backend Phase 1 hits. Used only when `phase1.query` matches `query`.
    pub phase1: Option<&'a Phase1Response>,
    pub power_plans: &'a [PowerPlanHit],
}

pub struct BuiltSections {
    pub sections: Vec<Section>,
    pub flat_items: Vec<PaletteItem>,
}

/// Caps on the displayed rows, not the data limits. The idle view lists every
/// app (no cap), so the user can scroll through all of them.
const RECENT_LIMIT: usize = 5;
const IDLE_QUICK_LIMIT: usize = 6;
const SEARCH_QUICK_LIMIT: usize = 3;
const SEARCH_APPS_LIMIT: usize = 6;

impl Searchable for PaletteItem {
    fn title(&self) -> &str {
        &self.title
    }

    fn keywords(&self) -> Option<&[String]> {
        self.keywords.as_deref()
    }
}

fn truthy(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.is_empty())
}

pub fn build_sections(sources: &PaletteSources) -> BuiltSections {
    let PaletteSources {
        query,
        apps,
        pinned_apps,
        quick_items,
        quick_access_collapsed,
        app_groups,
        section_order,
        pinned_app_ids,
        history,
        existing_history_paths,
        file_thumbnails,
        app_icons,
        file_results,
        file_result_query,
        file_path_browse,
        files_busy,
        files_error,
        file_indexing,
        file_index_ready,
        phase1,
        power_plans,
    } = sources;
    let (file_path_browse, files_busy, files_error) = (*file_path_browse, *files_busy, *files_error);
    let normalized = js::trim(query);
    let search_query = js::lower(normalized);
    let mut out: Vec<Section> = Vec::new();

    if normalized.is_empty() {
        // Only needed for the idle (empty query) view; sorting every app is
        // wasted work while the user is typing.
        let sorted_apps = sort_apps(apps);
        let pinned_items: Vec<PaletteItem> = pinned_apps.iter().map(|entry| app_palette_item(entry, app_icons)).collect();
        let pinned_item_ids: HashSet<String> = pinned_items.iter().map(|item| item.id.clone()).collect();
        if !pinned_items.is_empty() {
            out.push(Section::new("pinned", "Pinned", pinned_items));
        }
        // One lookup map for the whole idle rebuild: history rehydration and the
        // pinned filter below both need membership checks per row.
        let apps_by_id = apps_by_id(apps);
        let pinned_app_id_set: HashSet<&str> = pinned_app_ids.iter().map(String::as_str).collect();
        let mut recent_items: Vec<PaletteItem> = Vec::new();
        for entry in history.iter() {
            let item = rehydrate(entry, &apps_by_id, existing_history_paths, app_icons, file_thumbnails);
            if let Some(item) = item {
                if pinned_item_ids.contains(&item.id) {
                    continue;
                }
                recent_items.push(item);
            }
            if recent_items.len() >= RECENT_LIMIT {
                break;
            }
        }
        if !recent_items.is_empty() {
            out.push(Section::new("recent", "Recent", recent_items));
        }

        if !quick_items.is_empty() {
            let available_quick = quick_items[..quick_items.len().min(IDLE_QUICK_LIMIT)].to_vec();
            out.push(Section {
                collapsible: Some(true),
                collapsed: Some(*quick_access_collapsed),
                ..Section::new(
                    "quick",
                    "Quick Access",
                    if *quick_access_collapsed { Vec::new() } else { available_quick },
                )
            });
        }

        let unpinned: Vec<&AppEntry> =
            sorted_apps.into_iter().filter(|entry| !pinned_app_id_set.contains(entry.app_id.as_str())).collect();
        if let Some(apps_section) = build_apps_section(&unpinned, app_icons, app_groups, usize::MAX) {
            out.push(apps_section);
        }
    } else {
        if is_math_like(normalized)
            && let Some(math) = try_evaluate(normalized)
        {
            out.push(Section::new("calc", "Calculator", vec![calc_item(math.value)]));
        }

        let use_phase1 = phase1_matches_query(*phase1, normalized);
        let phase1 = phase1.filter(|_| use_phase1);
        let apps_by_id = apps_by_id(apps);
        let phase1_app_items: Vec<PaletteItem> = phase1
            .map(|p| {
                p.apps
                    .iter()
                    .filter_map(|hit| {
                        app_hit_palette_item(hit, &apps_by_id, app_icons, |entry| app_palette_item(entry, app_icons))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let app_hits: Vec<&AppEntry> = if !phase1_app_items.is_empty() {
            phase1_app_items
                .iter()
                .filter_map(|item| truthy(item.app_id.as_deref()).and_then(|id| apps_by_id.get(id).copied()))
                .collect()
        } else {
            fuzzy_apps(apps, normalized, SEARCH_APPS_LIMIT, true)
        };
        let mut app_paths: HashSet<String> = HashSet::new();
        for entry in &app_hits {
            if let Some(path) = truthy(entry.path.as_deref()) {
                app_paths.insert(js::lower(path));
            }
        }
        if let Some(phase1) = phase1 {
            for hit in &phase1.apps {
                if let Some(path) = truthy(hit.path.as_deref()) {
                    app_paths.insert(js::lower(path));
                }
            }
        }

        let browse_phase1 = phase1.filter(|_| !file_path_browse);
        let window_items: Vec<PaletteItem> =
            browse_phase1.map(|p| p.windows.iter().map(window_palette_item).collect()).unwrap_or_default();
        if !window_items.is_empty() {
            out.push(Section::new("windows", "Open windows", window_items));
        }

        let power_plan_items: Vec<PaletteItem> =
            if !file_path_browse { power_plans.iter().map(power_plan_palette_item).collect() } else { Vec::new() };
        let power_plan_picker = power_plans.iter().any(|plan| plan.picker_intent == Some(true));
        if !power_plan_items.is_empty() && power_plan_picker {
            out.push(Section::new("power-plans", "Power plans", power_plan_items.clone()));
        }

        let quick_hits = fuzzy(quick_items, normalized, SEARCH_QUICK_LIMIT);
        if !quick_hits.is_empty() {
            out.push(Section::new("quick", "Quick Access", quick_hits.iter().map(|hit| hit.item.clone()).collect()));
        }

        let deduped_files: Vec<&FileEntry> =
            file_results.iter().filter(|entry| !app_paths.contains(&js::lower(&entry.path))).collect();
        let file_items: Vec<PaletteItem> = if *file_result_query == search_query && !deduped_files.is_empty() {
            deduped_files.into_iter().map(file_palette_item).collect()
        } else {
            Vec::new()
        };

        if file_path_browse && !file_items.is_empty() {
            out.push(Section::new("files", "Folder Contents", file_items.clone()));
        }
        let actions = |keep: fn(&crate::lib::query::Phase1Hit) -> bool| -> Vec<PaletteItem> {
            browse_phase1
                .map(|p| p.actions.iter().filter(|hit| keep(hit)).map(action_palette_item).collect())
                .unwrap_or_default()
        };
        let is_tool = |hit: &crate::lib::query::Phase1Hit| hit.source.as_deref() == Some("windows-tool");
        let settings_items = actions(|hit| hit.source.as_deref() != Some("windows-tool") && !is_command_action(hit));
        let tool_items = actions(is_tool);
        let command_items = actions(|hit| hit.source.as_deref() != Some("windows-tool") && is_command_action(hit));
        if let Some(apps_section) = build_apps_section(&app_hits, app_icons, app_groups, SEARCH_APPS_LIMIT) {
            out.push(apps_section);
        } else if !phase1_app_items.is_empty() {
            out.push(Section::new(
                "apps",
                "Apps",
                phase1_app_items[..phase1_app_items.len().min(SEARCH_APPS_LIMIT)].to_vec(),
            ));
        }
        if !tool_items.is_empty() {
            out.push(Section::new("tools", "Windows Tools", tool_items));
        }
        if !command_items.is_empty() {
            out.push(Section::new("commands", "Commands", command_items));
        }
        if !settings_items.is_empty() {
            out.push(Section::new("settings", "Settings", settings_items));
        }
        if !file_path_browse && !file_items.is_empty() {
            out.push(Section::new("files", "Files & Folders", file_items));
        }
        if !power_plan_items.is_empty() && !power_plan_picker {
            out.push(Section::new("power-plans", "Power plans", power_plan_items));
        }

        let indexing_or_busy = files_busy || file_indexing.unwrap_or(false) || *file_index_ready == Some(false);
        if out.is_empty() && !indexing_or_busy && !files_error && !file_path_browse {
            out.push(Section::new("fallback", "No Local Matches", vec![copy_item(normalized)]));
        }
    }

    let sections = if normalized.is_empty() { order_sections(out, *section_order) } else { out };
    let flat_items = sections
        .iter()
        .flat_map(|section| {
            section.groups.iter().flatten().flat_map(|group| group.items.iter()).chain(section.items.iter())
        })
        .cloned()
        .collect();
    BuiltSections { sections, flat_items }
}

/// `new Map(apps.map((entry) => [entry.appId, entry]))`: the last duplicate id wins.
fn apps_by_id(apps: &[AppEntry]) -> HashMap<&str, &AppEntry> {
    apps.iter().map(|entry| (entry.app_id.as_str(), entry)).collect()
}

fn order_sections(sections: Vec<Section>, requested_order: Option<&[String]>) -> Vec<Section> {
    let Some(requested_order) = requested_order.filter(|order| !order.is_empty()) else {
        return sections;
    };
    let mut remaining: Vec<Option<Section>> = sections.into_iter().map(Some).collect();
    let mut ordered = Vec::new();
    for id in requested_order {
        if let Some(slot) = remaining.iter_mut().find(|slot| slot.as_ref().is_some_and(|s| &s.id == id)) {
            ordered.extend(slot.take());
        }
    }
    ordered.extend(remaining.into_iter().flatten());
    ordered
}

fn build_apps_section(
    entries: &[&AppEntry],
    icons: &HashMap<String, String>,
    groups: &[AppGroup],
    limit: usize,
) -> Option<Section> {
    if entries.is_empty() {
        return None;
    }
    let entry_by_id: HashMap<&str, &AppEntry> = entries.iter().map(|entry| (entry.app_id.as_str(), *entry)).collect();
    let mut grouped_ids: HashSet<String> = HashSet::new();
    let mut group_sections: Vec<AppGroupSection> = Vec::new();

    for group in groups {
        let items: Vec<PaletteItem> = group
            .app_ids
            .iter()
            .filter(|app_id| !grouped_ids.contains(*app_id))
            .filter_map(|app_id| entry_by_id.get(app_id.as_str()))
            .take(limit)
            .map(|entry| app_palette_item(entry, icons))
            .collect();
        if items.is_empty() {
            continue;
        }
        for item in &items {
            if let Some(app_id) = truthy(item.app_id.as_deref()) {
                grouped_ids.insert(app_id.to_owned());
            }
        }
        group_sections.push(AppGroupSection {
            group_id: group.id.clone(),
            id: format!("apps-group-{}", group.id),
            label: group.name.clone(),
            items: if group.collapsed { Vec::new() } else { items },
            collapsed: group.collapsed,
        });
    }

    let ungrouped: Vec<PaletteItem> = entries
        .iter()
        .filter(|entry| !grouped_ids.contains(&entry.app_id))
        .take(limit)
        .map(|entry| app_palette_item(entry, icons))
        .collect();
    if ungrouped.is_empty() && group_sections.is_empty() {
        return None;
    }
    Some(Section { groups: Some(group_sections), ..Section::new("apps", "Apps", ungrouped) })
}

/// Runs the selected result (clipboard-style ids must not hide the window).
pub fn is_clipboard_kind(id: &str) -> bool {
    id.starts_with("calc::") || id.starts_with("copy::")
}

/// The context menu labels itself from the live pin state, so the toggle
/// re-checks at click time (`RowAction::TogglePin`).
fn toggle_taskbar_pin(path: &str) -> RowAction {
    RowAction::TogglePin { path: path.to_owned() }
}

fn start_file_drag(path: &str) -> RowAction {
    RowAction::call("start_file_drag", json!({ "paths": [path] }))
}

fn path_call(cmd: &'static str, path: &str) -> RowAction {
    RowAction::call(cmd, json!({ "path": path }))
}

pub fn app_palette_item(app: &AppEntry, icons: &HashMap<String, String>) -> PaletteItem {
    // `app.location ?? (...)`: an empty location is kept (it is not nullish).
    let local_target: Option<&str> = app.location.as_deref().or_else(|| {
        truthy(app.path.as_deref()).filter(|path| path.contains(':') || path.starts_with("\\\\"))
    });
    let target = truthy(local_target);
    let mut item = PaletteItem::new(
        format!("app::{}", app.app_id),
        app.name.clone(),
        PaletteIcon::App { name: app.name.clone(), icon: icons.get(&app.app_id).cloned().or_else(|| app.icon.clone()) },
        app.name.clone(),
        RowAction::call("launch_app", json!({ "id": app.app_id })),
    );
    item.subtitle = None;
    item.app_id = Some(app.app_id.clone());
    item.drag_file = target.map(start_file_drag);
    item.run_as_admin = is_elevatable_path(app.path.as_deref())
        .then(|| RowAction::call("launch_app_as_admin", json!({ "id": app.app_id })));
    item.open_location = target.map(|t| path_call("open_path_location", t));
    item.shell_path = local_target.map(str::to_owned);
    item.toggle_taskbar_pin = target.filter(|t| is_taskbar_pinable_path(Some(t))).map(toggle_taskbar_pin);
    item.show_properties = target.map(|t| path_call("show_path_properties", t));
    item.copy_path = app_copy_path(app);
    item
}

/// The exe (or URL) an app runs, else its shortcut, else its AUMID through
/// `shell:AppsFolder` (Windows apps such as File Explorer have no file).
fn app_copy_path(app: &AppEntry) -> Option<String> {
    truthy(app.path.as_deref())
        .or_else(|| truthy(app.location.as_deref()))
        .map(str::to_owned)
        .or_else(|| apps_folder_path(app.aumid.as_deref().or(Some(app.app_id.as_str()))))
}

pub fn apps_folder_path(aumid: Option<&str>) -> Option<String> {
    truthy(aumid).map(|aumid| format!("shell:AppsFolder\\{aumid}"))
}

fn calc_item(value: f64) -> PaletteItem {
    let formatted = format_number(value);
    let mut item = PaletteItem::new(
        format!("calc::{}", js::number_to_string(value)),
        formatted.clone(),
        PaletteIcon::Tile { icon: LucideIcon("Calculator"), tint: "iris" },
        format!("Calculate {formatted}"),
        RowAction::Clipboard(formatted.clone()),
    );
    item.subtitle = Some("Copy result to clipboard".into());
    item.toast_detail = Some(formatted);
    item
}

fn copy_item(query: &str) -> PaletteItem {
    let mut item = PaletteItem::new(
        format!("copy::{query}"),
        format!("Copy \u{201C}{query}\u{201D}"),
        PaletteIcon::Tile { icon: LucideIcon("ClipboardCopy"), tint: "slate" },
        query.to_owned(),
        RowAction::Clipboard(query.to_owned()),
    );
    item.subtitle = Some("No files, folders, or apps matched".into());
    item
}

pub fn quick_access_palette_item(entry: &QuickAccessEntry) -> PaletteItem {
    // Kinds are sanitized against QUICK_ACCESS_KINDS before they get here.
    let (icon, tint): (&str, TileTint) = match entry.kind.as_str() {
        "home" => ("House", "iris"),
        "desktop" => ("Monitor", "azure"),
        "downloads" => ("Download", "mint"),
        "documents" => ("FileText", "slate"),
        "pictures" => ("Images", "amber"),
        "music" => ("Music", "rose"),
        "videos" => ("FileVideo", "rose"),
        // TODO(port): the source renders no icon for an unknown kind; the provider never passes one.
        _ => ("Folder", "slate"),
    };
    let path = entry.path.as_str();
    let mut item = PaletteItem::new(
        path_item_id(path, true),
        entry.name.clone(),
        PaletteIcon::Tile { icon: LucideIcon(icon), tint },
        entry.name.clone(),
        path_call("open_path", path),
    );
    item.subtitle = Some(entry.path.clone());
    item.keywords =
        Some(vec!["open".into(), "folder".into(), "files".into(), entry.kind.clone(), entry.path.clone()]);
    item.quick_access_kind = Some(entry.kind.clone());
    item.drag_file = truthy(Some(path)).map(start_file_drag);
    item.open_location = Some(path_call("open_path_location", path));
    item.shell_path = Some(entry.path.clone());
    item.copy_path = Some(entry.path.clone());
    item.show_properties = Some(path_call("show_path_properties", path));
    item
}

fn file_palette_item(entry: &FileEntry) -> PaletteItem {
    let (icon, tint) = file_appearance(entry);
    let path = entry.path.as_str();
    let icon = match truthy(entry.thumbnail.as_deref()) {
        Some(src) => PaletteIcon::Image { src: src.to_owned(), name: entry.name.clone() },
        None => PaletteIcon::Tile { icon: LucideIcon(icon), tint },
    };
    let mut item = PaletteItem::new(
        path_item_id(path, entry.is_directory),
        entry.name.clone(),
        icon,
        entry.name.clone(),
        path_call("open_path", path),
    );
    item.subtitle = Some(entry.parent.clone());
    item.keywords = Some(vec![entry.path.clone(), entry.parent.clone()]);
    item.is_picture = Some(!entry.is_directory && is_picture_path(Some(path)));
    item.drag_file = truthy(Some(path)).map(start_file_drag);
    item.run_as_admin =
        (!entry.is_directory && is_elevatable_path(Some(path))).then(|| path_call("run_path_as_admin", path));
    item.open_location = Some(path_call("open_path_location", path));
    item.shell_path = Some(entry.path.clone());
    item.copy_path = Some(entry.path.clone());
    item.toggle_taskbar_pin =
        (!entry.is_directory && is_taskbar_pinable_path(Some(path))).then(|| toggle_taskbar_pin(path));
    item.show_properties = Some(path_call("show_path_properties", path));
    item
}

const IMAGE_EXTENSIONS: [&str; 20] = [
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "ico", "tiff", "tif", "avif", "heic", "psd", "ai", "xd", "fig",
    "sketch", "raw", "cr2", "nef",
];
const VIDEO_EXTENSIONS: [&str; 10] = ["mp4", "mov", "mkv", "avi", "webm", "wmv", "flv", "m4v", "3gp", "ts"];
const AUDIO_EXTENSIONS: [&str; 11] = ["mp3", "wav", "flac", "m4a", "aac", "ogg", "wma", "opus", "aiff", "mid", "midi"];
const ARCHIVE_EXTENSIONS: [&str; 10] = ["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "cab", "tgz"];
const TEXT_EXTENSIONS: [&str; 38] = [
    "txt", "md", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "csv", "rtf", "odt", "ods", "odp", "json", "xml",
    "yaml", "yml", "toml", "html", "css", "js", "ts", "tsx", "jsx", "rs", "py", "c", "cpp", "h", "cs", "java", "go",
    "sql", "sh", "bat", "cmd", "ps1",
];

fn file_appearance(entry: &FileEntry) -> (&'static str, TileTint) {
    if entry.is_directory {
        return ("Folder", "azure");
    }
    // `name.split(".").pop()`: the text after the last dot, or the whole name.
    let extension = js::lower(entry.name.rsplit('.').next().unwrap_or(""));
    let extension = extension.as_str();
    if IMAGE_EXTENSIONS.contains(&extension) {
        ("FileImage", "amber")
    } else if VIDEO_EXTENSIONS.contains(&extension) {
        ("FileVideo", "rose")
    } else if AUDIO_EXTENSIONS.contains(&extension) {
        ("Music", "mint")
    } else if ARCHIVE_EXTENSIONS.contains(&extension) {
        ("FileArchive", "amber")
    } else if TEXT_EXTENSIONS.contains(&extension) {
        ("FileText", "slate")
    } else {
        ("File", "slate")
    }
}

fn path_item_id(path: &str, is_directory: bool) -> String {
    format!("file::{}::{path}", if is_directory { "d" } else { "f" })
}

const DIRECTORY_PREFIX: &str = "file::d::";
const FILE_PREFIX: &str = "file::f::";

/// Parses the persisted item id back into a filesystem path, when it is one.
pub fn history_file_path(id: &str) -> Option<&str> {
    id.strip_prefix(DIRECTORY_PREFIX).or_else(|| id.strip_prefix(FILE_PREFIX))
}

fn rehydrate(
    history: &HistoryEntry,
    apps_by_id: &HashMap<&str, &AppEntry>,
    existing_history_paths: &HashSet<String>,
    icons: &HashMap<String, String>,
    file_thumbnails: &HashMap<String, Option<String>>,
) -> Option<PaletteItem> {
    if let Some(app_id) = history.id.strip_prefix("app::") {
        return apps_by_id.get(app_id).map(|entry| app_palette_item(entry, icons));
    }
    let is_directory = history.id.starts_with(DIRECTORY_PREFIX);
    let path = history.id.strip_prefix(if is_directory { DIRECTORY_PREFIX } else { FILE_PREFIX })?;
    if !existing_history_paths.contains(path) {
        return None;
    }
    // `Math.max(lastIndexOf("\\"), lastIndexOf("/")) > 0`; both separators are ASCII,
    // so the byte index is > 0 exactly when the UTF-16 index is.
    let boundary = path.rfind(['\\', '/']).filter(|b| *b > 0);
    Some(file_palette_item(&FileEntry {
        name: history.title.clone(),
        path: path.to_owned(),
        parent: boundary.map_or(path, |b| &path[..b]).to_owned(),
        is_directory,
        thumbnail: file_thumbnails.get(path).cloned().flatten(),
    }))
}

// PORT STATUS: confidence=high todos=1
