//! Shared data shapes and the small pure helpers from `src/lib/types.ts`.
//! Field names serialize in camelCase, as the TypeScript and the backend use them.

use serde::{Deserialize, Serialize};

use crate::js;

pub type TileTint = &'static str; // "iris" | "azure" | "mint" | "amber" | "rose" | "slate"

/// A lucide icon, named by its React display name (`Home` is `House` in lucide).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LucideIcon(pub &'static str);

#[derive(Clone, Debug, PartialEq)]
pub enum PaletteIcon {
    Tile { icon: LucideIcon, tint: TileTint },
    Emoji { char: String },
    Image { src: String, name: String },
    App { name: String, icon: Option<String> },
}

/// One backend call, named by the original Tauri command; `args` are the
/// JSON arguments the TypeScript passed to `invoke`.
#[derive(Clone, Debug, PartialEq)]
pub struct BackendCall {
    pub cmd: &'static str,
    pub args: serde_json::Value,
}

/// A row action as data (RULEBOOK §1, A2). The UI executes it; the judge adapter records it.
#[derive(Clone, Debug, PartialEq)]
pub enum RowAction {
    None,
    Backend(Vec<BackendCall>),
    /// Writes the text to the clipboard; the palette stays open.
    Clipboard(String),
    /// Reads the pin state at click time, then flips it.
    TogglePin { path: String },
}

/// What a row action runs against: the backend (named by the original Tauri
/// command) and the clipboard. A rejected call is `Err(message)`; a call
/// without a reply is `Ok(Value::Null)` (`undefined` and `null` act the same here).
pub trait Invoke {
    fn invoke(&mut self, cmd: &'static str, args: serde_json::Value) -> Result<serde_json::Value, String>;
    fn write_clipboard(&mut self, text: &str) -> Result<(), String>;
}

impl RowAction {
    pub fn call(cmd: &'static str, args: serde_json::Value) -> Self {
        RowAction::Backend(vec![BackendCall { cmd, args }])
    }

    /// Runs the action. The first failed call stops it, as an awaited rejection does.
    pub fn execute(&self, host: &mut impl Invoke) -> Result<(), String> {
        match self {
            RowAction::None => Ok(()),
            RowAction::Backend(calls) => {
                for call in calls {
                    host.invoke(call.cmd, call.args.clone())?;
                }
                Ok(())
            }
            RowAction::Clipboard(text) => host.write_clipboard(text),
            RowAction::TogglePin { path } => {
                // `isPinnedToTaskbar(path).catch(() => false)`, then `!pinned` with JS truthiness.
                let pinned = host
                    .invoke("is_pinned_to_taskbar", serde_json::json!({ "path": path }))
                    .is_ok_and(|reply| js::truthy(Some(&reply)));
                host.invoke("set_taskbar_pinned", serde_json::json!({ "path": path, "pinned": !pinned }))?;
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaletteItem {
    /// Stable unique id, e.g. "app::{appId}" or "file::d::{path}".
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub icon: PaletteIcon,
    pub run: RowAction,
    pub run_as_admin: Option<RowAction>,
    pub open_location: Option<RowAction>,
    pub shell_path: Option<String>,
    /// What "Copy as path" copies (quoted, as Explorer does): a file or
    /// folder path, an app's exe or shortcut, `shell:AppsFolder\{AUMID}`, or
    /// a settings URI.
    pub copy_path: Option<String>,
    pub toggle_taskbar_pin: Option<RowAction>,
    pub show_properties: Option<RowAction>,
    pub is_picture: Option<bool>,
    pub drag_file: Option<RowAction>,
    /// Display title used when persisting to history.
    pub history_title: String,
    /// Extra line shown in the toast after a clipboard-style run.
    pub toast_detail: Option<String>,
    pub app_id: Option<String>,
    pub quick_access_kind: Option<String>,
    pub power_plan_guid: Option<String>,
}

impl PaletteItem {
    /// An item with every optional field empty.
    pub fn new(id: String, title: String, icon: PaletteIcon, history_title: String, run: RowAction) -> Self {
        Self {
            id,
            title,
            subtitle: None,
            keywords: None,
            icon,
            run,
            run_as_admin: None,
            open_location: None,
            shell_path: None,
            copy_path: None,
            toggle_taskbar_pin: None,
            show_properties: None,
            is_picture: None,
            drag_file: None,
            history_title,
            toast_detail: None,
            app_id: None,
            quick_access_kind: None,
            power_plan_guid: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub title: String,
    pub ts: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    pub name: String,
    pub app_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Lowercased, punctuation-stripped name for exact matching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_name: Option<String>,
    /// Resolved launch target (exe path or URL).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    /// Original shortcut location (.lnk/.url path), when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aumid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppGroup {
    pub id: String,
    pub name: String,
    pub app_ids: Vec<String>,
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub parent: String,
    pub is_directory: bool,
    /// Small data URL preview for supported image files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

pub const QUICK_ACCESS_KINDS: [&str; 7] = ["home", "desktop", "downloads", "documents", "pictures", "music", "videos"];
pub const DEFAULT_QUICK_ACCESS: [&str; 6] = ["home", "desktop", "downloads", "documents", "pictures", "music"];
pub const QUICK_ACCESS_LIMIT: usize = 6;
pub const PINNED_APP_LIMIT: usize = 64;
pub const APP_GROUP_LIMIT: usize = 16;
pub const APP_GROUP_APP_LIMIT: usize = 64;
pub const DEFAULT_SECTION_ORDER: [&str; 4] = ["pinned", "recent", "quick", "apps"];
pub const SECTION_ORDER_LIMIT: usize = DEFAULT_SECTION_ORDER.len();

/// `/^[a-z][a-z0-9+.-]*:\/\//i`
fn has_url_scheme(path: &str) -> bool {
    let bytes = path.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let mut i = 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'+' | b'.' | b'-')) {
        i += 1;
    }
    path[i..].starts_with("://")
}

/// The lowercased extension after the last dot of the file name, when the dot is not first.
fn file_extension(path: Option<&str>) -> Option<String> {
    let path = path.filter(|p| !p.is_empty())?;
    if has_url_scheme(path) {
        return None;
    }
    let filename = path.rsplit(['\\', '/']).next().unwrap_or("");
    let boundary = filename.rfind('.')?;
    (boundary > 0).then(|| js::lower(&filename[boundary + 1..]))
}

const ELEVATABLE_EXTENSIONS: [&str; 8] = ["exe", "com", "bat", "cmd", "ps1", "vbs", "js", "wsf"];
const PINNABLE_EXTENSIONS: [&str; 5] = ["exe", "lnk", "bat", "cmd", "msc"];
pub const PICTURE_EXTENSIONS: [&str; 12] =
    ["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "ico", "tiff", "tif", "avif", "heic"];

pub fn is_elevatable_path(path: Option<&str>) -> bool {
    file_extension(path).is_some_and(|ext| ELEVATABLE_EXTENSIONS.contains(&ext.as_str()))
}

/// Windows' taskbar-pin shell verb applies to launchable targets only.
pub fn is_taskbar_pinable_path(path: Option<&str>) -> bool {
    file_extension(path).is_some_and(|ext| PINNABLE_EXTENSIONS.contains(&ext.as_str()))
}

/// Recognizes common image and picture file extensions for drag-out and preview.
pub fn is_picture_path(path: Option<&str>) -> bool {
    file_extension(path).is_some_and(|ext| PICTURE_EXTENSIONS.contains(&ext.as_str()))
}

fn reorder_at<T: Clone>(items: &[T], source: Option<usize>, target: Option<usize>) -> Vec<T> {
    let mut reordered = items.to_vec();
    if let (Some(source), Some(target)) = (source, target)
        && source != target
    {
        let moved = reordered.remove(source);
        reordered.insert(target, moved);
    }
    reordered
}

fn index_of(items: &[String], value: &str) -> Option<usize> {
    items.iter().position(|item| item == value)
}

pub fn reorder_pinned_apps(pinned: &[String], source: &str, target: &str) -> Vec<String> {
    reorder_at(pinned, index_of(pinned, source), index_of(pinned, target))
}

pub fn reorder_quick_access(quick: &[String], source: &str, target: &str) -> Vec<String> {
    reorder_at(quick, index_of(quick, source), index_of(quick, target))
}

pub fn reorder_sections(order: &[String], source: &str, target: &str) -> Vec<String> {
    reorder_at(order, index_of(order, source), index_of(order, target))
}

pub fn reorder_app_groups(groups: &[AppGroup], source: &str, target: &str) -> Vec<AppGroup> {
    reorder_at(
        groups,
        groups.iter().position(|g| g.id == source),
        groups.iter().position(|g| g.id == target),
    )
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuickAccessEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeCoverage {
    pub drive: String,
    pub state: String,
    pub indexed_count: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_progress: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileSearchError {
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchResponse {
    pub items: Vec<FileEntry>,
    pub ready: bool,
    pub indexing: bool,
    pub path_browse: bool,
    pub volumes: Vec<VolumeCoverage>,
    pub total_indexed: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<FileSearchError>,
}

pub const VIEW_ZOOM_LEVELS: [f64; 9] = [70.0, 80.0, 90.0, 100.0, 110.0, 120.0, 130.0, 140.0, 150.0];

/// A level not in the list counts as index -1, as `indexOf` returns.
pub fn step_view_zoom(current: f64, direction: i32) -> f64 {
    let current_index = VIEW_ZOOM_LEVELS.iter().position(|z| *z == current).map_or(-1, |i| i as i64);
    let next = (current_index + direction as i64).clamp(0, VIEW_ZOOM_LEVELS.len() as i64 - 1);
    VIEW_ZOOM_LEVELS[next as usize]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub accent: String,
    pub width: f64,
    pub view_zoom: f64,
    pub shortcut: String,
    pub always_on_top: bool,
    pub taskbar_alignment: String,
    pub taskbar_scroll_volume: bool,
    pub theme: String,
    pub quick_access: Vec<String>,
    pub quick_access_collapsed: bool,
    pub pinned_apps: Vec<String>,
    pub app_groups: Vec<AppGroup>,
    pub section_order: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            accent: "iris".into(),
            width: 640.0,
            view_zoom: 100.0,
            shortcut: "Win".into(),
            always_on_top: true,
            taskbar_alignment: "center".into(),
            taskbar_scroll_volume: true,
            theme: "system".into(),
            quick_access: DEFAULT_QUICK_ACCESS.iter().map(|s| s.to_string()).collect(),
            quick_access_collapsed: false,
            pinned_apps: Vec::new(),
            app_groups: Vec::new(),
            section_order: DEFAULT_SECTION_ORDER.iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersistedState {
    /// Schema version for the state file; written by the backend.
    pub version: u32,
    pub settings: Settings,
    pub history: Vec<HistoryEntry>,
}

// PORT STATUS: confidence=high todos=0
