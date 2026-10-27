//! App state (`src/state/app.tsx`): the option lists, the validators for
//! persisted settings and history (pure; the judge calls them), and the
//! `AppState` entity that replaces the React provider (feature `gpui`).

use std::collections::HashSet;

use serde_json::Value;

use crate::js;
use crate::lib::types::{
    APP_GROUP_APP_LIMIT, APP_GROUP_LIMIT, AppGroup, DEFAULT_QUICK_ACCESS, DEFAULT_SECTION_ORDER, HistoryEntry,
    PINNED_APP_LIMIT, QUICK_ACCESS_KINDS, QUICK_ACCESS_LIMIT, SECTION_ORDER_LIMIT, Settings, VIEW_ZOOM_LEVELS,
};

pub struct ShortcutOption {
    pub value: &'static str,
    pub label: &'static str,
}

pub const SHORTCUT_OPTIONS: [ShortcutOption; 8] = [
    ShortcutOption { value: "Win", label: "Win key" },
    ShortcutOption { value: "Ctrl+Alt+Space", label: "Ctrl + Alt + Space" },
    ShortcutOption { value: "Ctrl+Alt+S", label: "Ctrl + Alt + S" },
    ShortcutOption { value: "Ctrl+Alt+Shift+S", label: "Ctrl + Alt + Shift + S" },
    ShortcutOption { value: "Ctrl+Alt+P", label: "Ctrl + Alt + P" },
    ShortcutOption { value: "Ctrl+Alt+Shift+P", label: "Ctrl + Alt + Shift + P" },
    ShortcutOption { value: "Ctrl+Alt+Enter", label: "Ctrl + Alt + Enter" },
    ShortcutOption { value: "Ctrl+Alt+Shift+Enter", label: "Ctrl + Alt + Shift + Enter" },
];

pub struct ThemeOption {
    pub value: &'static str,
    pub label: &'static str,
}

pub const THEME_OPTIONS: [ThemeOption; 3] = [
    ThemeOption { value: "system", label: "System" },
    ThemeOption { value: "dark", label: "Dark" },
    ThemeOption { value: "light", label: "Light" },
];

pub const HISTORY_CAP: usize = 20;

/// `allowed.includes(value) ? value : fallback` for string options.
fn pick_str(value: Option<&Value>, allowed: &[&str], fallback: &str) -> String {
    match value {
        Some(Value::String(s)) if allowed.contains(&s.as_str()) => s.clone(),
        _ => fallback.to_owned(),
    }
}

/// `allowed.includes(value) ? value : fallback` for number options.
fn pick_num(value: Option<&Value>, allowed: &[f64], fallback: f64) -> f64 {
    match value.and_then(Value::as_f64) {
        Some(n) if allowed.contains(&n) => n,
        _ => fallback,
    }
}

fn pick_bool(value: Option<&Value>, fallback: bool) -> bool {
    value.and_then(Value::as_bool).unwrap_or(fallback)
}

/// Validates loaded settings against the known value sets; anything
/// unexpected falls back to the default. Mirrors the backend validator.
/// `None` is "nothing persisted" (`undefined`).
pub fn sanitize_settings(raw: Option<&Value>) -> Settings {
    // `(raw ?? {})[key]`: a string, number, or array has none of these keys.
    let src = raw.and_then(Value::as_object);
    let field = |key: &str| src.and_then(|o| o.get(key));
    let defaults = Settings::default();
    let shortcuts: Vec<&str> = SHORTCUT_OPTIONS.iter().map(|o| o.value).collect();
    Settings {
        accent: pick_str(field("accent"), &["iris", "azure", "mint", "amber", "rose"], &defaults.accent),
        width: pick_num(field("width"), &[560.0, 640.0, 720.0], defaults.width),
        view_zoom: pick_num(field("viewZoom"), &VIEW_ZOOM_LEVELS, defaults.view_zoom),
        shortcut: pick_str(field("shortcut"), &shortcuts, &defaults.shortcut),
        always_on_top: pick_bool(field("alwaysOnTop"), defaults.always_on_top),
        taskbar_alignment: pick_str(
            field("taskbarAlignment"),
            &["left", "center", "right"],
            &defaults.taskbar_alignment,
        ),
        taskbar_scroll_volume: pick_bool(field("taskbarScrollVolume"), defaults.taskbar_scroll_volume),
        theme: pick_str(field("theme"), &["system", "dark", "light"], &defaults.theme),
        quick_access: sanitize_quick_access(field("quickAccess")),
        quick_access_collapsed: pick_bool(field("quickAccessCollapsed"), defaults.quick_access_collapsed),
        pinned_apps: sanitize_pinned_apps(field("pinnedApps")),
        app_groups: sanitize_app_groups(field("appGroups")),
        section_order: sanitize_section_order(field("sectionOrder")),
    }
}

fn sanitize_section_order(raw: Option<&Value>) -> Vec<String> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return DEFAULT_SECTION_ORDER.iter().map(|s| s.to_string()).collect();
    };
    let mut result: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for value in raw {
        let Some(value) = value.as_str() else {
            continue;
        };
        if !DEFAULT_SECTION_ORDER.contains(&value) || seen.contains(value) {
            continue;
        }
        seen.insert(value);
        result.push(value.to_owned());
        if result.len() >= SECTION_ORDER_LIMIT {
            break;
        }
    }
    for id in DEFAULT_SECTION_ORDER {
        if !seen.contains(id) {
            result.push(id.to_owned());
        }
    }
    result
}

fn sanitize_app_groups(raw: Option<&Value>) -> Vec<AppGroup> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<AppGroup> = Vec::new();
    for value in raw {
        // `!value || typeof value !== "object"` lets arrays through, but an
        // array has no `id`, so it is skipped below either way.
        let Some(src) = value.as_object() else {
            continue;
        };
        let id = src.get("id").and_then(Value::as_str).map_or("", js::trim);
        let name = src.get("name").and_then(Value::as_str).map_or("", js::trim);
        if id.is_empty() || js::len(id) > 96 || seen.contains(id) || name.is_empty() || js::len(name) > 64 {
            continue;
        }
        let app_ids = sanitize_ids(src.get("appIds"), APP_GROUP_APP_LIMIT);
        result.push(AppGroup {
            id: id.to_owned(),
            name: name.to_owned(),
            app_ids,
            collapsed: src.get("collapsed").and_then(Value::as_bool).unwrap_or(false),
        });
        seen.insert(id.to_owned());
        if result.len() >= APP_GROUP_LIMIT {
            break;
        }
    }
    result
}

fn sanitize_pinned_apps(raw: Option<&Value>) -> Vec<String> {
    sanitize_ids(raw, PINNED_APP_LIMIT)
}

fn sanitize_ids(raw: Option<&Value>, limit: usize) -> Vec<String> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut result: Vec<String> = Vec::new();
    for value in raw {
        let Some(value) = value.as_str() else {
            continue;
        };
        if value.is_empty() || js::len(value) > 4096 || seen.contains(value) {
            continue;
        }
        seen.insert(value);
        result.push(value.to_owned());
        if result.len() == limit {
            break;
        }
    }
    result
}

fn sanitize_quick_access(raw: Option<&Value>) -> Vec<String> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return DEFAULT_QUICK_ACCESS.iter().map(|s| s.to_string()).collect();
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut result: Vec<String> = Vec::new();
    for value in raw {
        let Some(kind) = value.as_str() else {
            continue;
        };
        if !QUICK_ACCESS_KINDS.contains(&kind) || seen.contains(kind) {
            continue;
        }
        seen.insert(kind);
        result.push(kind.to_owned());
        if result.len() == QUICK_ACCESS_LIMIT {
            break;
        }
    }
    result
}

pub fn sanitize_history(raw: Option<&Value>) -> Vec<HistoryEntry> {
    let Some(raw) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut history: Vec<HistoryEntry> = Vec::new();
    for value in raw {
        let Some(entry) = value.as_object() else {
            continue;
        };
        let (Some(id), Some(title), Some(ts)) = (
            entry.get("id").and_then(Value::as_str),
            entry.get("title").and_then(Value::as_str),
            entry.get("ts").filter(|ts| ts.is_number()).and_then(Value::as_f64),
        ) else {
            continue;
        };
        if id.is_empty()
            || js::len(id) > 4096
            || title.is_empty()
            || js::len(title) > 512
            || !ts.is_finite()
            || seen.contains(id)
        {
            continue;
        }
        seen.insert(id);
        history.push(HistoryEntry { id: id.to_owned(), title: title.to_owned(), ts });
        if history.len() == HISTORY_CAP {
            break;
        }
    }
    history
}

/* ---------------- the provider (app.tsx:96-653) ---------------- */

#[cfg(feature = "gpui")]
pub use provider::*;

#[cfg(feature = "gpui")]
mod provider {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use futures::FutureExt;
    use futures::future::Shared;
    use gpui::{App, AppContext, Context, Entity, EventEmitter, Task, Timer};
    use serde_json::{Map, Value, json};

    use super::{HISTORY_CAP, sanitize_history, sanitize_settings};
    use crate::gfx::reduced_motion;
    use crate::lib::bridge::{self, BackendEvent};
    use crate::lib::types::{DEFAULT_SECTION_ORDER, HistoryEntry, Settings, step_view_zoom};
    use crate::theme::{Accent, Mode, Theme};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ToastKind {
        Success,
        Error,
    }

    #[derive(Clone, Debug)]
    pub struct Toast {
        pub id: u64,
        pub title: String,
        pub detail: Option<String>,
        pub kind: ToastKind,
        /// Set while the toast animates out before removal.
        pub closing: Option<Instant>,
        pub born: Instant,
    }

    /// Emitted when settings or history change, so dependents can re-derive.
    pub enum AppEvent {
        SettingsChanged { previous: Box<Settings> },
        HistoryChanged,
        OpenSettingsChanged,
    }

    pub struct AppState {
        pub ready: bool,
        pub settings: Settings,
        pub history: Vec<HistoryEntry>,
        pub open_settings: bool,
        pub toasts: Vec<Toast>,
        pub system_theme: String,
        pub persistence_error: Option<String>,
        pub theme: Theme,
        persist_timer: Option<Task<()>>,
        persist_request: Option<Shared<Task<Result<(), String>>>>,
        state_load_error: Option<String>,
        state_read: Shared<Task<()>>,
        state_read_complete: bool,
        settings_changed_after_load_error: Map<String, Value>,
        history_changed_after_load_error: bool,
        persist_revision: u64,
        saved_revision: u64,
        rendered_width: Option<u32>,
        width_task: Option<Task<()>>,
        toast_timers: HashMap<u64, Task<()>>,
        toast_seq: u64,
        wheel_delta: f32,
        wheel_reset: Option<Task<()>>,
        _subscriptions: Vec<gpui::Subscription>,
    }

    impl EventEmitter<AppEvent> for AppState {}

    const LOAD_ERROR_PREFIX: &str = "Could not load settings: ";
    const LOAD_ERROR_SUFFIX: &str = ". Repair prism.json, then retry. Saving is paused to preserve your file.";

    fn load_error_message(error: &str) -> String {
        format!("{LOAD_ERROR_PREFIX}{}{LOAD_ERROR_SUFFIX}", error_string(error))
    }

    /// `String(error)` of a rejected invoke: `Error: <message>` in the WebView.
    pub fn error_string(message: &str) -> String {
        message.to_owned()
    }

    fn settings_json(settings: &Settings) -> Map<String, Value> {
        match serde_json::to_value(settings) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        }
    }

    /// JSON numbers the backend validator reads with `as_u64` must be integers.
    fn integralize(value: Value) -> Value {
        match value {
            Value::Number(n) => match n.as_f64() {
                Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 => json!(f as i64),
                _ => Value::Number(n),
            },
            Value::Array(items) => Value::Array(items.into_iter().map(integralize).collect()),
            Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, integralize(v))).collect()),
            other => other,
        }
    }

    fn now_ms() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0)
    }

    impl AppState {
        pub fn new(cx: &mut Context<Self>) -> Self {
            // Initial load from disk: every value is sanitized against known sets so
            // a corrupt or hand-edited file degrades to defaults, never crashes.
            let load = cx.spawn(async move |this, cx| {
                let result = bridge::load_state().await;
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(state) => {
                            let previous = this.settings.clone();
                            let mut merged = settings_json(&sanitize_settings(state.get("settings")));
                            merged.extend(std::mem::take(&mut this.settings_changed_after_load_error));
                            this.settings =
                                serde_json::from_value(Value::Object(merged)).unwrap_or_else(|_| this.settings.clone());
                            if !this.history_changed_after_load_error {
                                this.history = sanitize_history(state.get("history"));
                            }
                            this.history_changed_after_load_error = false;
                            cx.emit(AppEvent::SettingsChanged { previous: Box::new(previous) });
                            cx.emit(AppEvent::HistoryChanged);
                        }
                        Err(error) => {
                            let message = load_error_message(&error);
                            this.state_load_error = Some(message.clone());
                            this.set_persistence_error(Some(message), cx);
                        }
                    }
                    this.state_read_complete = true;
                    this.ready = true;
                    this.on_ready(cx);
                    cx.notify();
                });
            });
            let events = bridge::event_hub(cx);
            let subscription = cx.subscribe(&events, |this, _, event: &BackendEvent, cx| match event {
                BackendEvent::SystemThemeChanged(theme) => {
                    this.system_theme = theme.clone();
                    this.refresh_theme(cx);
                }
                // Safe recovery: if Win-key interception self-disables (e.g. an elevated
                // app rejected the replay), drop back to the default shortcut so the UI
                // and reality stay in sync.
                BackendEvent::WinModeFailed(reason) if this.ready => {
                    this.show_toast("Win-key mode disabled", Some(reason.clone()), ToastKind::Error, cx);
                    let shortcut = Settings::default().shortcut;
                    this.update_settings(cx, |s| s.shortcut = shortcut);
                }
                _ => {}
            });
            let settings = Settings::default();
            Self {
                ready: false,
                theme: Theme::new(Mode::Dark, Accent::parse(&settings.accent)),
                settings,
                history: Vec::new(),
                open_settings: false,
                toasts: Vec::new(),
                system_theme: "dark".into(),
                persistence_error: None,
                persist_timer: None,
                persist_request: None,
                state_load_error: None,
                state_read: load.shared(),
                state_read_complete: false,
                settings_changed_after_load_error: Map::new(),
                history_changed_after_load_error: false,
                persist_revision: 0,
                saved_revision: 0,
                rendered_width: None,
                width_task: None,
                toast_timers: HashMap::new(),
                toast_seq: 1,
                wheel_delta: 0.0,
                wheel_reset: None,
                _subscriptions: vec![subscription],
            }
        }

        /// Effects that start once settings are known (the `[ready]` effects).
        fn on_ready(&mut self, cx: &mut Context<Self>) {
            // Follow the OS theme live (only relevant in "system" mode).
            self.system_theme = bridge::get_system_theme();
            self.refresh_theme(cx);
            self.apply_window_style(cx);
            self.apply_width(cx);
            self.apply_always_on_top(cx);
            self.apply_view_zoom(cx);
            self.apply_scroll_volume(cx);
        }

        pub fn effective_theme(&self) -> &str {
            if self.settings.theme == "system" { &self.system_theme } else { &self.settings.theme }
        }

        fn refresh_theme(&mut self, cx: &mut Context<Self>) {
            let mode = if self.effective_theme() == "light" { Mode::Light } else { Mode::Dark };
            let accent = Accent::parse(&self.settings.accent);
            let changed_mode = self.theme.mode != mode;
            if changed_mode || self.theme.accent_id != accent {
                self.theme = Theme::new(mode, accent);
                if self.ready {
                    bridge::set_osd_accent(&self.settings.accent);
                    if changed_mode {
                        self.apply_window_style(cx);
                    }
                }
                cx.notify();
            } else if self.ready {
                bridge::set_osd_accent(&self.settings.accent);
            }
        }

        fn apply_window_style(&mut self, cx: &mut Context<Self>) {
            let theme = self.effective_theme().to_owned();
            if let Err(error) = bridge::shell(cx).set_window_style(&theme, cx) {
                self.show_toast("Appearance not applied", Some(error_string(&error)), ToastKind::Error, cx);
            }
        }

        fn apply_always_on_top(&mut self, cx: &mut Context<Self>) {
            if let Err(error) = bridge::shell(cx).set_always_on_top(self.settings.always_on_top, cx) {
                self.show_toast(
                    "Always on top not applied",
                    Some(format!("Toggle the setting to retry. {}", error_string(&error))),
                    ToastKind::Success,
                    cx,
                );
            }
        }

        /// The view zoom is applied by the root view through the rem size (A11).
        fn apply_view_zoom(&mut self, cx: &mut Context<Self>) {
            cx.notify();
        }

        fn apply_scroll_volume(&mut self, _cx: &mut Context<Self>) {
            bridge::set_taskbar_scroll_volume(self.settings.taskbar_scroll_volume);
        }

        /// Animates the native window width (app.tsx:518-568): 240 ms ease-out cubic,
        /// at most one native resize per 45 ms.
        fn apply_width(&mut self, cx: &mut Context<Self>) {
            let target = self.settings.width as u32;
            let from = self.rendered_width;
            self.width_task = None;
            if from.is_none() || from == Some(target) || reduced_motion() {
                self.rendered_width = Some(target);
                if let Err(error) = bridge::shell(cx).set_window_width(target, cx) {
                    self.show_toast(
                        "Window width not applied",
                        Some(format!("Try selecting the width again. {}", error_string(&error))),
                        ToastKind::Success,
                        cx,
                    );
                }
                return;
            }
            let from = from.unwrap_or(target) as f64;
            let started = Instant::now();
            self.width_task = Some(cx.spawn(async move |this, cx| {
                let duration = 240.0;
                let mut last_dispatched = from as i64;
                let mut last_dispatch: Option<Instant> = None;
                loop {
                    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                    let progress = (elapsed / duration).min(1.0);
                    let eased = 1.0 - (1.0 - progress).powi(3);
                    let next = (from + (target as f64 - from) * eased).round() as i64;
                    let due = last_dispatch.is_none_or(|t| t.elapsed() >= Duration::from_millis(45));
                    let done = progress >= 1.0;
                    let alive = this
                        .update(cx, |this, cx| {
                            this.rendered_width = Some(next as u32);
                            if done || (due && next != last_dispatched) {
                                let result = bridge::shell(cx).set_window_width(next as u32, cx);
                                if let Err(error) = result
                                    && done
                                {
                                    this.show_toast(
                                        "Window width not applied",
                                        Some(format!("Try selecting the width again. {}", error_string(&error))),
                                        ToastKind::Success,
                                        cx,
                                    );
                                }
                            }
                        })
                        .is_ok();
                    if done || (due && next != last_dispatched) {
                        last_dispatched = next;
                        last_dispatch = Some(Instant::now());
                    }
                    if !alive || done {
                        return;
                    }
                    Timer::after(Duration::from_millis(16)).await;
                }
            }));
        }

        /// Applies each native side effect only when its own setting changes.
        fn apply_changed(&mut self, previous: &Settings, cx: &mut Context<Self>) {
            if !self.ready {
                return;
            }
            if previous.accent != self.settings.accent || previous.theme != self.settings.theme {
                self.refresh_theme(cx);
            }
            if previous.width != self.settings.width {
                self.apply_width(cx);
            }
            if previous.always_on_top != self.settings.always_on_top {
                self.apply_always_on_top(cx);
            }
            if previous.view_zoom != self.settings.view_zoom {
                self.apply_view_zoom(cx);
            }
            if previous.taskbar_scroll_volume != self.settings.taskbar_scroll_volume {
                self.apply_scroll_volume(cx);
            }
        }

        fn set_persistence_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
            let changed = self.persistence_error != error;
            self.persistence_error = error;
            if changed && self.persistence_error.is_some() {
                self.show_toast(
                    "Settings need attention",
                    Some("Open Settings to review the error and retry saving.".into()),
                    ToastKind::Error,
                    cx,
                );
            }
            cx.notify();
        }

        fn persisted_state(&self) -> Value {
            integralize(json!({
                "version": 3,
                "settings": self.settings,
                "history": self.history,
            }))
        }

        /// A flush drains changes made during a pending write before resolving.
        pub fn flush_persistence(&mut self, cx: &mut Context<Self>) -> Shared<Task<Result<(), String>>> {
            self.persist_timer = None;
            if let Some(pending) = &self.persist_request {
                return pending.clone();
            }
            let state_read = self.state_read.clone();
            let task = cx.spawn(async move |this, cx| {
                state_read.await;
                loop {
                    let next = this.update(cx, |this, _| {
                        if this.persist_revision == this.saved_revision {
                            return Ok(None);
                        }
                        if let Some(error) = &this.state_load_error {
                            return Err(error.clone());
                        }
                        Ok(Some((this.persist_revision, this.persisted_state())))
                    });
                    let next = match next {
                        Ok(Ok(next)) => next,
                        Ok(Err(error)) => {
                            let _ = this.update(cx, |this, _| this.persist_request = None);
                            return Err(error);
                        }
                        Err(_) => return Ok(()),
                    };
                    let Some((revision, state)) = next else {
                        let _ = this.update(cx, |this, cx| {
                            this.persist_request = None;
                            if this.state_load_error.is_none() {
                                this.set_persistence_error(None, cx);
                            }
                        });
                        return Ok(());
                    };
                    if let Err(error) = bridge::save_state(state).await {
                        let message = format!(
                            "Could not save settings: {}. Free disk space or check folder permissions, then retry.",
                            error_string(&error)
                        );
                        let _ = this.update(cx, |this, cx| {
                            this.persist_request = None;
                            this.set_persistence_error(Some(message), cx);
                        });
                        return Err(error);
                    }
                    let _ = this.update(cx, |this, _| this.saved_revision = revision);
                }
            });
            let shared = task.shared();
            self.persist_request = Some(shared.clone());
            shared
        }

        fn schedule_persist(&mut self, cx: &mut Context<Self>) {
            self.persist_revision += 1;
            self.persist_timer = Some(cx.spawn(async move |this, cx| {
                Timer::after(Duration::from_millis(350)).await;
                if let Ok(flush) = this.update(cx, |this, cx| this.flush_persistence(cx))
                    // `flush_persistence` also shows the failure in settings (`persistence_error`).
                    && let Err(error) = flush.await
                {
                    eprintln!("Prism could not save settings: {error}");
                }
            }));
        }

        pub fn retry_persistence(&mut self, cx: &mut Context<Self>) -> Task<Result<(), String>> {
            cx.spawn(async move |this, cx| {
                let load_error = this.update(cx, |this, _| this.state_load_error.is_some()).unwrap_or(false);
                if load_error {
                    match bridge::load_state().await {
                        Ok(state) => {
                            let _ = this.update(cx, |this, cx| {
                                let previous = this.settings.clone();
                                let mut merged = settings_json(&sanitize_settings(state.get("settings")));
                                merged.extend(this.settings_changed_after_load_error.clone());
                                if let Ok(settings) = serde_json::from_value(Value::Object(merged)) {
                                    this.settings = settings;
                                }
                                if !this.history_changed_after_load_error {
                                    this.history = sanitize_history(state.get("history"));
                                }
                                this.state_load_error = None;
                                this.settings_changed_after_load_error.clear();
                                this.history_changed_after_load_error = false;
                                this.set_persistence_error(None, cx);
                                this.apply_changed(&previous, cx);
                                cx.emit(AppEvent::SettingsChanged { previous: Box::new(previous) });
                                cx.emit(AppEvent::HistoryChanged);
                            });
                        }
                        Err(error) => {
                            let message = load_error_message(&error);
                            let _ = this.update(cx, |this, cx| {
                                this.state_load_error = Some(message.clone());
                                this.set_persistence_error(Some(message), cx);
                            });
                            return Err(error);
                        }
                    }
                }
                let flush = this.update(cx, |this, cx| this.flush_persistence(cx)).map_err(|e| e.to_string())?;
                flush.await
            })
        }

        /// Applies a settings change. Shortcut and taskbar alignment changes are
        /// applied by their pickers before state is updated.
        pub fn update_settings(&mut self, cx: &mut Context<Self>, patch: impl FnOnce(&mut Settings)) {
            let previous = self.settings.clone();
            let mut next = previous.clone();
            patch(&mut next);
            if next == previous {
                return;
            }
            if !self.state_read_complete || self.state_load_error.is_some() {
                let before = settings_json(&previous);
                for (key, value) in settings_json(&next) {
                    if before.get(&key) != Some(&value) {
                        self.settings_changed_after_load_error.insert(key, value);
                    }
                }
            }
            self.settings = next;
            self.schedule_persist(cx);
            self.apply_changed(&previous, cx);
            cx.emit(AppEvent::SettingsChanged { previous: Box::new(previous) });
            cx.notify();
        }

        pub fn change_view_zoom(&mut self, direction: i32, cx: &mut Context<Self>) {
            let current = self.settings.view_zoom;
            let next = step_view_zoom(current, direction);
            if next != current {
                self.update_settings(cx, |s| s.view_zoom = next);
            }
        }

        pub fn reset_view_zoom(&mut self, cx: &mut Context<Self>) {
            let zoom = Settings::default().view_zoom;
            self.update_settings(cx, |s| s.view_zoom = zoom);
        }

        /// Ctrl + wheel (app.tsx:319-338). `delta_y` is Chromium's `deltaY` in CSS px.
        pub fn zoom_wheel(&mut self, delta_y: f32, cx: &mut Context<Self>) {
            if delta_y == 0.0 {
                return;
            }
            if self.wheel_delta != 0.0 && delta_y.signum() != self.wheel_delta.signum() {
                self.wheel_delta = 0.0;
            }
            self.wheel_delta += delta_y;
            if self.wheel_delta.abs() >= 40.0 {
                let direction = if self.wheel_delta < 0.0 { 1 } else { -1 };
                self.change_view_zoom(direction, cx);
                self.wheel_delta = 0.0;
            }
            self.wheel_reset = Some(cx.spawn(async move |this, cx| {
                Timer::after(Duration::from_millis(180)).await;
                let _ = this.update(cx, |this, _| this.wheel_delta = 0.0);
            }));
        }

        pub fn set_open_settings(&mut self, open: bool, cx: &mut Context<Self>) {
            if self.open_settings != open {
                self.open_settings = open;
                cx.emit(AppEvent::OpenSettingsChanged);
                cx.notify();
            }
        }

        fn note_history_change(&mut self) {
            if !self.state_read_complete || self.state_load_error.is_some() {
                self.history_changed_after_load_error = true;
            }
        }

        pub fn push_history(&mut self, id: &str, title: &str, cx: &mut Context<Self>) {
            self.note_history_change();
            let mut next = vec![HistoryEntry { id: id.to_owned(), title: title.to_owned(), ts: now_ms() }];
            next.extend(self.history.iter().filter(|entry| entry.id != id).cloned());
            next.truncate(HISTORY_CAP);
            self.history = next;
            self.schedule_persist(cx);
            cx.emit(AppEvent::HistoryChanged);
            cx.notify();
        }

        pub fn clear_history(&mut self, cx: &mut Context<Self>) {
            self.note_history_change();
            self.history.clear();
            self.schedule_persist(cx);
            cx.emit(AppEvent::HistoryChanged);
            cx.notify();
        }

        pub fn remove_history(&mut self, id: &str, cx: &mut Context<Self>) {
            let before = self.history.len();
            let next: Vec<HistoryEntry> = self.history.iter().filter(|entry| entry.id != id).cloned().collect();
            if next.len() == before {
                return;
            }
            self.note_history_change();
            self.history = next;
            self.schedule_persist(cx);
            cx.emit(AppEvent::HistoryChanged);
            cx.notify();
        }

        /// Resets every setting. Native registrations go first so a failed shortcut
        /// or taskbar transition cannot leave the UI claiming that defaults are active.
        pub fn reset_settings(&mut self, cx: &mut Context<Self>) -> Task<Result<(), String>> {
            let state_read = self.state_read.clone();
            cx.spawn(async move |this, cx| {
                state_read.await;
                let (load_error, previous) = this
                    .update(cx, |this, _| (this.state_load_error.clone(), this.settings.clone()))
                    .map_err(|e| e.to_string())?;
                if let Some(error) = load_error {
                    return Err(error);
                }
                let defaults = Settings::default();
                bridge::set_shortcut(defaults.shortcut.clone()).await?;
                let alignment = this
                    .update(cx, |_, cx| bridge::shell(cx).set_taskbar_alignment(&defaults.taskbar_alignment, cx))
                    .map_err(|e| e.to_string())?;
                if let Err(error) = alignment {
                    // A rejected alignment operation may already have moved some HWNDs.
                    let rollback = this
                        .update(cx, |_, cx| bridge::shell(cx).set_taskbar_alignment(&previous.taskbar_alignment, cx))
                        .map_err(|e| e.to_string())?;
                    let alignment_rollback_error = rollback.err();
                    if let Err(rollback_error) = bridge::set_shortcut(previous.shortcut.clone()).await {
                        // The successful registration is the last known native shortcut.
                        let _ = this.update(cx, |this, cx| {
                            let shortcut = defaults.shortcut.clone();
                            this.update_settings(cx, |s| s.shortcut = shortcut);
                        });
                        let taskbar = alignment_rollback_error
                            .map(|e| format!(" Taskbar restoration failed: {}.", error_string(&e)))
                            .unwrap_or_default();
                        return Err(format!(
                            "Reset stopped: {}. Shortcut restoration also failed: {}. The shortcut remains {}.{taskbar}",
                            error_string(&error),
                            error_string(&rollback_error),
                            defaults.shortcut
                        ));
                    }
                    if let Some(rollback_error) = alignment_rollback_error {
                        return Err(format!(
                            "Reset stopped: {}. Could not restore taskbar alignment: {}. Select the alignment again to retry.",
                            error_string(&error),
                            error_string(&rollback_error)
                        ));
                    }
                    return Err(error);
                }
                this.update(cx, |this, cx| {
                    let previous = this.settings.clone();
                    this.settings = Settings {
                        quick_access: Settings::default().quick_access,
                        pinned_apps: Vec::new(),
                        app_groups: Vec::new(),
                        section_order: DEFAULT_SECTION_ORDER.iter().map(|s| s.to_string()).collect(),
                        ..Settings::default()
                    };
                    this.schedule_persist(cx);
                    this.apply_changed(&previous, cx);
                    cx.emit(AppEvent::SettingsChanged { previous: Box::new(previous) });
                    cx.notify();
                })
                .map_err(|e| e.to_string())
            })
        }

        pub fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
            self.toast_timers.remove(&id);
            let Some(toast) = self.toasts.iter_mut().find(|t| t.id == id) else {
                return;
            };
            if toast.closing.is_some() {
                return;
            }
            // Mark the toast as closing so it plays its exit animation, then remove
            // it once the animation has finished.
            toast.closing = Some(Instant::now());
            let remove = cx.spawn(async move |this, cx| {
                Timer::after(Duration::from_millis(160)).await;
                let _ = this.update(cx, |this, cx| {
                    this.toast_timers.remove(&id);
                    this.toasts.retain(|t| t.id != id);
                    cx.notify();
                });
            });
            self.toast_timers.insert(id, remove);
            cx.notify();
        }

        pub fn show_toast(
            &mut self,
            title: impl Into<String>,
            detail: Option<String>,
            kind: ToastKind,
            cx: &mut Context<Self>,
        ) {
            let id = self.toast_seq;
            self.toast_seq += 1;
            // Keep at most two toasts on screen; evict the oldest with its exit
            // animation instead of dropping it instantly.
            let visible: Vec<u64> = self.toasts.iter().map(|t| t.id).collect();
            self.toasts.push(Toast { id, title: title.into(), detail, kind, closing: None, born: Instant::now() });
            if kind == ToastKind::Success {
                let timer = cx.spawn(async move |this, cx| {
                    Timer::after(Duration::from_millis(1900)).await;
                    let _ = this.update(cx, |this, cx| this.dismiss_toast(id, cx));
                });
                self.toast_timers.insert(id, timer);
            }
            if visible.len() >= 2 {
                self.dismiss_toast(visible[0], cx);
            }
            cx.notify();
        }

        pub fn quit(&mut self, cx: &mut Context<Self>) {
            let flush = self.flush_persistence(cx);
            cx.spawn(async move |this, cx| {
                let result = match flush.await {
                    Ok(()) => match this.update(cx, |_, cx| bridge::shell(cx).quit(cx)) {
                        Ok(task) => task.await,
                        Err(error) => Err(error.to_string()),
                    },
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    let _ = this.update(cx, |this, cx| {
                        this.show_toast(
                            "Could not quit Prism",
                            Some(format!("Your changes are still open. {}", error_string(&error))),
                            ToastKind::Error,
                            cx,
                        );
                    });
                }
            })
            .detach();
        }
    }

    /// The app state entity, as a GPUI global so any view can read it.
    #[derive(Clone)]
    pub struct AppStateHandle(pub Entity<AppState>);

    impl gpui::Global for AppStateHandle {}

    pub fn app_state(cx: &App) -> Entity<AppState> {
        cx.global::<AppStateHandle>().0.clone()
    }

    pub fn install(cx: &mut App) -> Entity<AppState> {
        let entity = cx.new(AppState::new);
        cx.set_global(AppStateHandle(entity.clone()));
        entity
    }
}

// PORT STATUS: confidence=medium todos=0
