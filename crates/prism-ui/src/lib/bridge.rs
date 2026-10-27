//! The backend surface (`src/lib/bridge.ts`). No IPC (RULEBOOK §1): every
//! command is a direct call on the `prism-core` backend, run on its runtime so
//! blocking work never touches the UI thread (A7). Window operations go to the
//! [`Shell`] that `prism-app` installs as a GPUI global.

use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;

use futures::FutureExt;
use futures::future::BoxFuture;
use gpui::{App, Global};
use prism_core::backend::backend;
use serde::{Deserialize, Serialize};

use crate::lib::fixture;
use crate::lib::query::{Phase1Response, PowerPlanHit, RecentCommand};
use crate::lib::types::{AppEntry, FileSearchResponse, QuickAccessEntry};

pub type BridgeResult<T> = Result<T, String>;

/// Runs a backend future on the prism-core runtime and returns a future the
/// GPUI executor can await. A panicked task is reported as an error.
fn run<T: Send + 'static>(future: impl Future<Output = BridgeResult<T>> + Send + 'static) -> impl Future<Output = BridgeResult<T>> {
    let handle = prism_core::runtime::spawn(future);
    async move { handle.await.map_err(|error| error.to_string())? }
}

fn run_infallible<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> impl Future<Output = BridgeResult<T>> {
    let handle = prism_core::runtime::spawn(future);
    async move { handle.await.map_err(|error| error.to_string()) }
}

/// Converts a backend value into the UI's own shape through JSON (the same
/// camelCase contract the WebView used).
fn reshape<From: Serialize, To: for<'de> Deserialize<'de>>(value: From) -> BridgeResult<To> {
    serde_json::to_value(value).and_then(serde_json::from_value).map_err(|error| error.to_string())
}

pub type PowerAction = &'static str;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomStartIcon {
    pub id: String,
    /// Base64 PNG preview (96 x 96).
    pub preview: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskbarSettings {
    pub thickness: String,
    pub auto_hide: bool,
    pub combine_buttons: String,
    pub start_icon: String,
    pub selected_custom_icon: Option<String>,
    pub custom_start_icons: Vec<CustomStartIcon>,
}

/* ---------------- window (owned by prism-app) ---------------- */

/// The window operations the WebView reached through Tauri's window API.
pub trait Shell: 'static {
    fn present_palette(&self, cx: &mut App) -> BridgeResult<bool>;
    fn hide_palette(&self, defer: bool, cx: &mut App) -> BridgeResult<()>;
    fn is_window_visible(&self, cx: &mut App) -> bool;
    fn set_always_on_top(&self, on: bool, cx: &mut App) -> BridgeResult<()>;
    fn set_window_style(&self, theme: &str, cx: &mut App) -> BridgeResult<()>;
    fn set_window_width(&self, width: u32, cx: &mut App) -> BridgeResult<()>;
    fn set_taskbar_alignment(&self, alignment: &str, cx: &mut App) -> BridgeResult<()>;
    /// Runs the OLE drag from a foreground task (A7) and resolves when it ends.
    fn start_file_drag(&self, paths: Vec<String>, cx: &mut App) -> gpui::Task<BridgeResult<bool>>;
    fn quit(&self, cx: &mut App) -> gpui::Task<BridgeResult<()>>;
    fn app_version(&self) -> String;
    /// The window gained or lost activation (the native `Focused` window event).
    fn window_activation_changed(&self, active: bool, cx: &mut App);
    /// The foreground window belongs to Prism (a file dialog it opened): a
    /// focus loss to it is not a click-away.
    fn owns_foreground(&self) -> bool {
        false
    }
    /// The native side still holds the palette open (it declined a focus
    /// loss, for example inside the activation grace period).
    fn palette_held_open(&self) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct ShellHandle(pub Rc<dyn Shell>);

impl Global for ShellHandle {}

pub fn shell(cx: &App) -> Rc<dyn Shell> {
    cx.global::<ShellHandle>().0.clone()
}

pub fn get_app_version(cx: &App) -> String {
    shell(cx).app_version()
}

/* ---------------- events ---------------- */

/// Backend and window events the original received through `listen(...)`.
#[derive(Clone, Debug)]
pub enum BackendEvent {
    /// `prism-toggle`: the desired palette state from the shell.
    Toggle { open: bool },
    /// The window gained or lost focus (`onFocusChanged`).
    WindowFocused(bool),
    FileIndexUpdated,
    AppsUpdated,
    SystemThemeChanged(String),
    WinModeFailed(String),
    /// `prism:close` (a UI request to hide the palette).
    CloseRequested,
    /// `prism:dismiss-transient-ui` (src/lib/transientUi.ts).
    DismissTransientUi,
}

pub struct EventHub;

impl gpui::EventEmitter<BackendEvent> for EventHub {}

#[derive(Clone)]
pub struct EventHubHandle(pub gpui::Entity<EventHub>);

impl Global for EventHubHandle {}

pub fn event_hub(cx: &App) -> gpui::Entity<EventHub> {
    cx.global::<EventHubHandle>().0.clone()
}

pub fn install_event_hub(cx: &mut App) -> gpui::Entity<EventHub> {
    use gpui::AppContext;
    let hub = cx.new(|_| EventHub);
    cx.set_global(EventHubHandle(hub.clone()));
    hub
}

/// Delivers one event to every subscriber.
pub fn emit_event(event: BackendEvent, cx: &mut App) {
    let hub = event_hub(cx);
    hub.update(cx, |_, cx| cx.emit(event));
}

/* ---------------- clipboard ---------------- */

pub fn copy_text(text: &str, cx: &mut App) {
    fixture::record("plugin:clipboard-manager|write_text", serde_json::json!({ "text": text }));
    if fixture::get().is_some() {
        return; // parity runs never touch the real clipboard
    }
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_owned()));
}

/* ---------------- row actions ---------------- */

/// Runs row-action backend calls synchronously (from a GPUI background thread,
/// never a runtime thread) for [`crate::lib::types::RowAction::execute`].
pub struct CoreInvoker;

impl crate::lib::types::Invoke for CoreInvoker {
    fn invoke(&mut self, cmd: &'static str, args: serde_json::Value) -> BridgeResult<serde_json::Value> {
        use serde_json::Value;
        let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        fixture::record(cmd, args.clone());
        if let Some(fixture) = fixture::get() {
            if matches!(cmd, "launch_app" | "launch_app_as_admin" | "open_path" | "run_path_as_admin") {
                std::thread::sleep(std::time::Duration::from_millis(fixture.launch_delay_ms));
            }
            return Ok(match cmd {
                "is_pinned_to_taskbar" => Value::Bool(fixture.pinned_to_taskbar.contains(&text("path"))),
                _ => Value::Null,
            });
        }
        let core = backend();
        let unit = |result: BridgeResult<()>| result.map(|()| Value::Null);
        prism_core::runtime::block_on(async {
            match cmd {
                "launch_app" => unit(core.launch_app(text("id")).await),
                "launch_app_as_admin" => unit(core.launch_app_as_admin(text("id")).await),
                "open_path" => unit(core.open_path(text("path")).await),
                "open_path_location" => unit(core.open_path_location(text("path")).await),
                "run_path_as_admin" => unit(core.run_path_as_admin(text("path")).await),
                "show_path_properties" => unit(core.show_path_properties(text("path")).await),
                "is_pinned_to_taskbar" => Ok(Value::Bool(core.is_pinned_to_taskbar(text("path")).await)),
                "set_taskbar_pinned" => {
                    let pinned = args.get("pinned").and_then(Value::as_bool).unwrap_or(false);
                    unit(core.set_taskbar_pinned(text("path"), pinned).await)
                }
                "execute_action" => unit(core.execute_action(text("id")).await),
                "set_power_plan" => unit(core.set_power_plan(text("guid")).await),
                "focus_window" => {
                    let hwnd = args.get("hwnd").and_then(Value::as_f64).unwrap_or(0.0) as i64;
                    unit(core.focus_window(hwnd))
                }
                other => Err(format!("{other} runs on the UI thread")),
            }
        })
    }

    fn write_clipboard(&mut self, _text: &str) -> BridgeResult<()> {
        Err("the clipboard is written on the UI thread".into())
    }
}

/* ---------------- rust commands ---------------- */

pub type Reply<T> = BoxFuture<'static, BridgeResult<T>>;

/// The fixture's answer, or the backend's.
fn answer<T: Send + 'static>(
    canned: impl FnOnce(&'static fixture::Fixture) -> BridgeResult<T>,
    live: impl Future<Output = BridgeResult<T>> + Send + 'static,
) -> Reply<T> {
    match fixture::get() {
        Some(f) => futures::future::ready(canned(f)).boxed(),
        None => run(live).boxed(),
    }
}

fn answer_infallible<T: Send + 'static>(
    canned: impl FnOnce(&'static fixture::Fixture) -> T,
    live: impl Future<Output = T> + Send + 'static,
) -> Reply<T> {
    match fixture::get() {
        Some(f) => futures::future::ready(Ok(canned(f))).boxed(),
        None => run_infallible(live).boxed(),
    }
}

/// A command with no fixture answer beyond success.
fn command(cmd: &str, args: serde_json::Value, live: impl Future<Output = BridgeResult<()>> + Send + 'static) -> Reply<()> {
    fixture::record(cmd, args);
    answer(|_| Ok(()), live)
}

pub fn get_apps() -> Reply<Vec<AppEntry>> {
    answer(|f| reshape(f.apps_without_icons()), async { reshape(backend().get_apps().await?) })
}

/// Batched lazy icon payload: only the rows actually rendered request icons.
pub fn get_app_icons(ids: Vec<String>) -> Reply<HashMap<String, String>> {
    let canned_ids = ids.clone();
    answer(move |f| Ok(f.app_icons(&canned_ids)), async move { backend().get_app_icons(ids) })
}

pub fn refresh_apps() -> Reply<Vec<AppEntry>> {
    // A rescan is an action: the fixture records it, as `command` does.
    answer(
        |f| {
            fixture::record("refresh_apps", serde_json::json!({}));
            reshape(f.apps_without_icons())
        },
        async { reshape(backend().refresh_apps().await?) },
    )
}

pub fn query_phase1(query: String, recent: Vec<RecentCommand>) -> Reply<Phase1Response> {
    let canned = query.clone();
    answer(move |f| reshape(f.phase1(&canned)), async move {
        let recent = reshape(recent)?;
        reshape(backend().query_phase1(query, recent))
    })
}

pub fn query_power_plans(query: String) -> Reply<Vec<PowerPlanHit>> {
    let canned = query.clone();
    answer(move |f| reshape(f.power_plans(&canned)), async move {
        reshape(backend().query_power_plans(query).await?)
    })
}

pub fn execute_action(id: String) -> Reply<()> {
    command("execute_action", serde_json::json!({ "id": id }), async move { backend().execute_action(id).await })
}

pub fn accept_intent(query: String, action_id: String) -> Reply<()> {
    command("accept_intent", serde_json::json!({ "query": query, "actionId": action_id }), async move { backend().accept_intent(query, action_id) })
}

pub fn focus_window(hwnd: i64) -> Reply<()> {
    command("focus_window", serde_json::json!({ "hwnd": hwnd }), async move { backend().focus_window(hwnd) })
}

pub fn search_files(query: String, limit: usize) -> Reply<FileSearchResponse> {
    let canned = query.clone();
    answer(move |f| reshape(f.search_files(&canned)), async move {
        reshape(backend().search_files(query, Some(limit)).await?)
    })
}

pub fn rebuild_file_index() -> Reply<()> {
    command("rebuild_file_index", serde_json::json!({}), async {
        backend().rebuild_file_index();
        Ok(())
    })
}

/// Thumbnails for many paths in one round trip, returned in input order.
pub fn get_file_thumbnails(paths: Vec<String>) -> Reply<Vec<Option<String>>> {
    let count = paths.len();
    let canned = paths.clone();
    answer_infallible(move |f| f.thumbnails(&canned), async move {
        let mut thumbnails = backend().get_file_thumbnails(paths).await;
        thumbnails.resize(count, None);
        thumbnails
    })
}

pub fn get_quick_access() -> Reply<Vec<QuickAccessEntry>> {
    answer(|f| reshape(f.quick_access.clone()), async { reshape(backend().get_quick_access().await) })
}

pub fn perform_power_action(action: &'static str) -> Reply<()> {
    command("perform_power_action", serde_json::json!({ "action": action }), async move { backend().perform_power_action(action.to_owned()).await })
}

pub fn existing_paths(paths: Vec<String>) -> Reply<Vec<String>> {
    let canned = paths.clone();
    answer_infallible(move |f| f.existing(&canned), async move { backend().existing_paths(paths).await })
}

pub fn is_pinned_to_taskbar(path: String) -> Reply<bool> {
    fixture::record("is_pinned_to_taskbar", serde_json::json!({ "path": path }));
    let canned = path.clone();
    answer_infallible(move |f| f.pinned_to_taskbar.contains(&canned), async move {
        backend().is_pinned_to_taskbar(path).await
    })
}

pub fn set_power_plan(guid: String) -> Reply<()> {
    command("set_power_plan", serde_json::json!({ "guid": guid }), async move { backend().set_power_plan(guid).await })
}

pub fn get_taskbar_settings() -> Reply<TaskbarSettings> {
    answer(|f| reshape(f.taskbar.clone()), async { reshape(backend().get_taskbar_settings().await?) })
}

pub fn set_taskbar_thickness(value: String) -> Reply<()> {
    command("set_taskbar_thickness", serde_json::json!({ "value": value }), async move { backend().set_taskbar_thickness(value) })
}

pub fn set_taskbar_auto_hide(enabled: bool) -> Reply<()> {
    command("set_taskbar_auto_hide", serde_json::json!({ "enabled": enabled }), async move { backend().set_taskbar_auto_hide(enabled) })
}

pub fn set_taskbar_combine_buttons(value: String) -> Reply<()> {
    command("set_taskbar_combine_buttons", serde_json::json!({ "value": value }), async move { backend().set_taskbar_combine_buttons(value) })
}

pub fn set_taskbar_start_icon(value: String) -> Reply<()> {
    command("set_taskbar_start_icon", serde_json::json!({ "value": value }), async move { backend().set_taskbar_start_icon(value).await })
}

pub fn set_custom_start_icon(png: Vec<u8>) -> Reply<()> {
    use base64::Engine;
    let base64_png = base64::engine::general_purpose::STANDARD.encode(png);
    command("set_custom_start_icon", serde_json::json!({}), async move { backend().set_custom_start_icon(base64_png).await })
}

pub fn select_custom_start_icon(id: String) -> Reply<()> {
    command("select_custom_start_icon", serde_json::json!({ "id": id }), async move { backend().select_custom_start_icon(id).await })
}

pub fn remove_custom_start_icon(id: String) -> Reply<()> {
    command("remove_custom_start_icon", serde_json::json!({ "id": id }), async move { backend().remove_custom_start_icon(id).await })
}

pub fn get_system_theme() -> String {
    match fixture::get() {
        Some(f) if !f.system_theme.is_empty() => f.system_theme.clone(),
        Some(_) => "dark".into(),
        None => backend().get_system_theme(),
    }
}

pub fn set_shortcut(combo: String) -> Reply<()> {
    command("set_shortcut", serde_json::json!({ "combo": combo }), async move { backend().set_shortcut(combo).await })
}

pub fn set_taskbar_scroll_volume(enabled: bool) {
    fixture::record("set_taskbar_scroll_volume", serde_json::json!({ "enabled": enabled }));
    if fixture::get().is_none() {
        backend().set_taskbar_scroll_volume(enabled);
    }
}

pub fn set_osd_accent(accent: &str) {
    fixture::record("set_osd_accent", serde_json::json!({ "accent": accent }));
    if fixture::get().is_none() {
        backend().set_osd_accent(accent.to_owned());
    }
}

pub fn load_state() -> Reply<serde_json::Value> {
    answer(|f| Ok(f.state.clone()), async { backend().load_state() })
}

pub fn save_state(state: serde_json::Value) -> Reply<()> {
    command("save_state", serde_json::json!({ "state": state.clone() }), async move { backend().save_state(state) })
}

// PORT STATUS: confidence=high todos=0
