//! The settings sheet (`src/components/SettingsSheet.tsx`).

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, Context, DispatchPhase, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollHandle, SharedString, Styled, Task, Timer, Window,
    canvas, div, prelude::*, px, relative,
};

use crate::components::color_picker::{self, Hsv};
use crate::components::taskbar_customization::TaskbarCustomization;
use crate::components::text_input::{Changed, InputStyle, TextInput};
use crate::components::focus;
use crate::components::ui::{SegmentOption, row_icon, segmented, settings_row, title_tooltip, toggle, truncate, weight};
use crate::state::app::theme;
use crate::gfx::{Tween, icon, icon_stroke, r, reduced_motion, zoomed};
use crate::lib::bridge;
use crate::lib::emoji::locale_compare;
use crate::lib::types::{
    APP_GROUP_APP_LIMIT, APP_GROUP_LIMIT, AppEntry, AppGroup, PaletteIcon, QUICK_ACCESS_LIMIT, Settings,
    VIEW_ZOOM_LEVELS, step_view_zoom,
};
use crate::state::app::{AppEvent, AppState, THEME_LABELS, ToastKind, app_state, shortcut_label};
use crate::state::palette::{PaletteState, palette_state};
use crate::theme::{
    Accent, BACKGROUND_PRESETS, Rgba, TRANSPARENT, Theme, background_preset, fade, hsla, motion, shadow, tabular_nums, to_hex, white,
};
use crate::transition::{self, Timing};

/// The setting a color picker edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorTarget {
    Accent,
    Background,
}

impl ColorTarget {
    fn stored(self, s: &Settings) -> Option<&String> {
        match self {
            ColorTarget::Accent => s.accent_color.as_ref(),
            ColorTarget::Background => s.background_color.as_ref(),
        }
    }

    fn field(self, s: &mut Settings) -> &mut Option<String> {
        match self {
            ColorTarget::Accent => &mut s.accent_color,
            ColorTarget::Background => &mut s.background_color,
        }
    }

    /// What the setting paints now: the accent, or the top of the shell.
    fn painted(self, t: &Theme) -> Rgba {
        match self {
            ColorTarget::Accent => t.accent,
            ColorTarget::Background => t.shell_solid.from,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragPart {
    Plane,
    Hue,
}

/// An open color picker. It edits the setting live; Esc puts `original` back.
struct ColorPicker {
    target: ColorTarget,
    hsv: Hsv,
    original: Option<String>,
    /// What the setting painted when the picker opened (the left half of the preview).
    original_paint: Rgba,
    drag: Option<DragPart>,
    opened_at: Instant,
}

/// "Ctrl+Alt+Space" -> "Ctrl + Alt + Space"
pub fn display_shortcut(combo: &str) -> String {
    combo.replace('+', " + ")
}

const QUICK_ACCESS_OPTIONS: [(&str, &str, &str); 7] = [
    ("home", "Home", "house"),
    ("desktop", "Desktop", "monitor"),
    ("downloads", "Downloads", "download"),
    ("documents", "Documents", "file-text"),
    ("pictures", "Pictures", "images"),
    ("music", "Music", "music"),
    ("videos", "Videos", "file-video"),
];

fn field_style(t: &Theme) -> InputStyle {
    InputStyle { text: t.fg, placeholder: t.fg_quiet, selection: fade(t.accent, 40.0), caret: t.fg }
}

/// `.focus-ring:focus-visible` on a text field: 1px accent at 65% and 4px accent at 18%, outside the box.
/// An input matches `:focus-visible` on any focus, so there is no keyboard check. A shadow takes no layout space.
fn field_ring(t: &Theme, zoom: f32) -> Vec<gpui::BoxShadow> {
    // GPUI paints the list bottom-up; CSS lists the top shadow first.
    zoomed(vec![shadow(0., 0., 0., 4., fade(t.accent, 18.)), shadow(0., 0., 0., 1., fade(t.accent, 65.))], zoom)
}

fn now_base36() -> String {
    let mut n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while n > 0 {
        out.push(digits[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn random_base36(len: usize) -> String {
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1);
    let mut x = seed as u64 ^ 0x9e37_79b9_7f4a_7c15;
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            digits[(x % 36) as usize] as char
        })
        .collect()
}

pub struct SettingsSheet {
    app: Entity<AppState>,
    palette: Entity<PaletteState>,
    focus: FocusHandle,
    /// The first button (Close): focused on open.
    close_focus: FocusHandle,
    scroll: ScrollHandle,
    /// The focus already scrolled into view.
    revealed: Option<FocusHandle>,
    wheel: std::rc::Rc<std::cell::RefCell<crate::gfx::WheelScroll>>,
    opened_at: Instant,
    closing: Option<Instant>,
    close_task: Option<Task<()>>,
    version: String,
    reset_confirming: bool,
    reset_busy: bool,
    retry_save_busy: bool,
    shortcut_busy: bool,
    alignment_busy: bool,
    taskbar: Entity<TaskbarCustomization>,
    // App collections.
    new_name: Entity<TextInput>,
    group_names: HashMap<String, Entity<TextInput>>,
    open_group_id: Option<String>,
    app_query: Entity<TextInput>,
    picker_scroll: ScrollHandle,
    picker_wheel: std::rc::Rc<std::cell::RefCell<crate::gfx::WheelScroll>>,
    color_picker: Option<ColorPicker>,
    hex_input: Entity<TextInput>,
    plane_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    hue_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    plane_focus: FocusHandle,
    hue_focus: FocusHandle,
    /// The custom swatches: focus returns here when their picker closes.
    accent_custom_focus: FocusHandle,
    background_custom_focus: FocusHandle,
    _subscriptions: Vec<gpui::Subscription>,
}

impl SettingsSheet {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = app_state(cx);
        let palette = palette_state(cx);
        let (t, _) = theme(cx);
        let new_name = cx.new(|cx| TextInput::new("New collection", field_style(&t), cx).with_max_len(64));
        let app_query = cx.new(|cx| TextInput::new("Filter apps", field_style(&t), cx));
        let hex_input = cx.new(|cx| TextInput::new("#rrggbb", field_style(&t), cx).with_max_len(7));
        let hex_sub = cx.subscribe(&hex_input, |this, _, event: &Changed, cx| this.type_hex(&event.0, cx));
        let app_sub = cx.subscribe_in(&app, window, |this, app, event: &AppEvent, window, cx| {
            if matches!(event, AppEvent::OpenSettingsChanged) {
                if app.read(cx).open_settings {
                    // A fresh open resets the exit state and cancels any pending close.
                    this.close_task = None;
                    this.closing = None;
                    this.opened_at = Instant::now();
                    // The original unmounts the sheet when it closes: every
                    // open starts at the top with empty fields.
                    this.scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                    *this.wheel.borrow_mut() = Default::default();
                    this.new_name.update(cx, |input, cx| input.set_text("", cx));
                    this.app_query.update(cx, |input, cx| input.set_text("", cx));
                    this.color_picker = None;
                    window.focus(&this.close_focus);
                } else {
                    this.reset_confirming = false;
                    this.open_group_id = None;
                    this.color_picker = None;
                    // Focus returns to the settings button in the search field.
                    window.focus(&focus::settings_trigger(cx));
                }
            }
            cx.notify();
        });
        let query_sub = cx.subscribe(&app_query, |_, _, _: &Changed, cx| cx.notify());
        let palette_obs = cx.observe(&palette, |_, _, cx| cx.notify());
        let taskbar = cx.new(TaskbarCustomization::new);
        Self {
            app,
            palette,
            focus: cx.focus_handle(),
            close_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            revealed: None,
            wheel: Default::default(),
            opened_at: Instant::now(),
            closing: None,
            close_task: None,
            version: bridge::get_app_version(cx),
            reset_confirming: false,
            reset_busy: false,
            retry_save_busy: false,
            shortcut_busy: false,
            alignment_busy: false,
            taskbar,
            new_name,
            group_names: HashMap::new(),
            open_group_id: None,
            app_query,
            picker_scroll: ScrollHandle::new(),
            picker_wheel: Default::default(),
            color_picker: None,
            hex_input,
            plane_bounds: Rc::default(),
            hue_bounds: Rc::default(),
            plane_focus: cx.focus_handle().tab_stop(true),
            hue_focus: cx.focus_handle().tab_stop(true),
            accent_custom_focus: cx.focus_handle().tab_stop(true),
            background_custom_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: vec![app_sub, query_sub, palette_obs, hex_sub],
        }
    }

    /// Deliberate closes (X, backdrop) animate the sheet out first; Esc snaps.
    fn request_close(&mut self, cx: &mut Context<Self>) {
        if self.closing.is_some() {
            return;
        }
        self.closing = Some(Instant::now());
        let app = self.app.clone();
        self.close_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(150)).await;
            let _ = this.update(cx, |this, _| this.close_task = None);
            let _ = app.update(cx, |app, cx| app.set_open_settings(false, cx));
        }));
        cx.notify();
    }

    fn confirm_reset(&mut self, cx: &mut Context<Self>) {
        if self.reset_busy {
            return;
        }
        if !self.reset_confirming {
            self.reset_confirming = true;
            cx.notify();
            return;
        }
        self.reset_busy = true;
        self.color_picker = None;
        cx.notify();
        let task = self.app.update(cx, |app, cx| app.reset_settings(cx));
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.reset_busy = false;
                if result.is_ok() {
                    this.reset_confirming = false;
                }
                cx.notify();
            });
            let _ = app.update(cx, |app, cx| match result {
                Ok(()) => app.show_toast("Settings reset", Some("Prism is back to its defaults".into()), ToastKind::Success, cx),
                Err(error) => app.show_toast("Settings not reset", Some(error.to_string()), ToastKind::Error, cx),
            });
        })
        .detach();
    }

    fn retry_save(&mut self, cx: &mut Context<Self>) {
        if self.retry_save_busy {
            return;
        }
        self.retry_save_busy = true;
        cx.notify();
        let task = self.app.update(cx, |app, cx| app.retry_persistence(cx));
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.retry_save_busy = false;
                cx.notify();
            });
            if let Err(error) = result {
                let _ = app.update(cx, |app, cx| {
                    app.show_toast("Settings still not saved", Some(error.to_string()), ToastKind::Error, cx)
                });
            }
        })
        .detach();
    }

    fn apply_shortcut(&mut self, combo: String, cx: &mut Context<Self>) {
        if combo == self.app.read(cx).settings.shortcut || self.shortcut_busy {
            return;
        }
        self.shortcut_busy = true;
        cx.notify();
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            // OS first: the backend validates, registers, and only then releases
            // the previous binding. State persists only on success.
            let result = bridge::set_shortcut(combo.clone()).await;
            let _ = app.update(cx, |app, cx| match result {
                Ok(()) => {
                    let value = combo.clone();
                    app.update_settings(cx, |s| s.shortcut = value);
                    app.show_toast("Shortcut set", Some(display_shortcut(&combo)), ToastKind::Success, cx);
                }
                Err(error) => app.show_toast("Shortcut not changed", Some(error.to_string()), ToastKind::Error, cx),
            });
            let _ = this.update(cx, |this, cx| {
                this.shortcut_busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_alignment(&mut self, alignment: String, cx: &mut Context<Self>) {
        if alignment == self.app.read(cx).settings.taskbar_alignment || self.alignment_busy {
            return;
        }
        self.alignment_busy = true;
        let result = bridge::shell(cx).set_taskbar_alignment(&alignment, cx);
        self.app.update(cx, |app, cx| match result {
            Ok(()) => app.update_settings(cx, |s| s.taskbar_alignment = alignment),
            Err(error) => app.show_toast("Taskbar not changed", Some(error.to_string()), ToastKind::Error, cx),
        });
        self.alignment_busy = false;
        cx.notify();
    }

    /* ---------------- app collections ---------------- */

    fn create_group(&mut self, cx: &mut Context<Self>) {
        let name = crate::js::trim(self.new_name.read(cx).text()).to_owned();
        if name.is_empty() {
            return;
        }
        if self.app.read(cx).settings.app_groups.len() >= APP_GROUP_LIMIT {
            self.app.update(cx, |app, cx| {
                app.show_toast(
                    "Collection limit reached",
                    Some(format!("Prism supports up to {APP_GROUP_LIMIT} collections")),
                    ToastKind::Success,
                    cx,
                )
            });
            return;
        }
        let id = format!("group-{}-{}", now_base36(), random_base36(5));
        self.app.update(cx, |app, cx| {
            app.update_settings(cx, |s| s.app_groups.push(AppGroup { id, name, app_ids: Vec::new(), collapsed: false }))
        });
        self.new_name.update(cx, |input, cx| input.set_text("", cx));
    }

    fn update_group(&mut self, id: &str, cx: &mut Context<Self>, patch: impl FnOnce(&mut AppGroup)) {
        let id = id.to_owned();
        self.app.update(cx, |app, cx| {
            app.update_settings(cx, |s| {
                if let Some(group) = s.app_groups.iter_mut().find(|g| g.id == id) {
                    patch(group);
                }
            })
        });
    }

    fn remove_group(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_owned();
        self.app.update(cx, |app, cx| app.update_settings(cx, |s| s.app_groups.retain(|g| g.id != id)));
    }

    fn move_group(&mut self, id: &str, direction: i32, cx: &mut Context<Self>) {
        let id = id.to_owned();
        self.app.update(cx, |app, cx| {
            app.update_settings(cx, |s| {
                let Some(index) = s.app_groups.iter().position(|g| g.id == id) else { return };
                let target = index as i32 + direction;
                if target < 0 || target as usize >= s.app_groups.len() {
                    return;
                }
                let group = s.app_groups.remove(index);
                s.app_groups.insert(target as usize, group);
            })
        });
    }

    fn assign_app(&mut self, group_id: &str, app_id: &str, cx: &mut Context<Self>) {
        let settings = self.app.read(cx).settings.clone();
        let Some(target) = settings.app_groups.iter().find(|g| g.id == group_id) else { return };
        if app_id.is_empty() || target.app_ids.iter().any(|a| a == app_id) {
            return;
        }
        if target.app_ids.len() >= APP_GROUP_APP_LIMIT {
            self.app.update(cx, |app, cx| {
                app.show_toast(
                    "Collection is full",
                    Some(format!("Each collection supports up to {APP_GROUP_APP_LIMIT} apps")),
                    ToastKind::Success,
                    cx,
                )
            });
            return;
        }
        let (group_id, app_id) = (group_id.to_owned(), app_id.to_owned());
        self.app.update(cx, |app, cx| {
            app.update_settings(cx, |s| {
                for group in &mut s.app_groups {
                    if group.id == group_id {
                        group.app_ids.push(app_id.clone());
                    } else {
                        group.app_ids.retain(|candidate| candidate != &app_id);
                    }
                }
            })
        });
    }

    fn remove_app(&mut self, group_id: &str, app_id: &str, cx: &mut Context<Self>) {
        let app_id = app_id.to_owned();
        self.update_group(group_id, cx, |group| group.app_ids.retain(|id| id != &app_id));
    }

    fn toggle_app_picker(&mut self, group_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_group_id = if self.open_group_id.as_deref() == Some(group_id) { None } else { Some(group_id.to_owned()) };
        self.app_query.update(cx, |input, cx| input.set_text("", cx));
        if self.open_group_id.is_some() {
            self.app_query.read(cx).focus(window);
        }
        cx.notify();
    }

    fn available_apps(&self, cx: &App) -> Vec<AppEntry> {
        let mut apps = self.palette.read(cx).apps().to_vec();
        apps.sort_by(|a, b| locale_compare(&a.name, &b.name));
        apps
    }

    fn filtered_apps(&self, cx: &App) -> Vec<AppEntry> {
        let normalized = crate::js::trim(self.app_query.read(cx).text()).to_lowercase();
        let apps = self.available_apps(cx);
        if normalized.is_empty() {
            return apps;
        }
        apps.into_iter().filter(|entry| entry.name.to_lowercase().contains(&normalized)).collect()
    }

    /* ---------------- color picker ---------------- */

    fn custom_focus(&self, target: ColorTarget) -> &FocusHandle {
        match target {
            ColorTarget::Accent => &self.accent_custom_focus,
            ColorTarget::Background => &self.background_custom_focus,
        }
    }

    /// The custom swatch toggles its picker. It opens on the stored color, or on
    /// what the setting paints now, and changes nothing until the color moves.
    fn toggle_picker(&mut self, target: ColorTarget, window: &mut Window, cx: &mut Context<Self>) {
        if self.color_picker.as_ref().is_some_and(|picker| picker.target == target) {
            self.close_picker(false, true, window, cx);
            return;
        }
        let (t, _) = theme(cx);
        let original = target.stored(&self.app.read(cx).settings).cloned();
        let seed = original.as_deref().and_then(|hex| Rgba::try_from(hex).ok()).unwrap_or_else(|| target.painted(&t));
        self.color_picker = Some(ColorPicker {
            target,
            hsv: Hsv::from_rgba(seed),
            original,
            original_paint: target.painted(&t),
            drag: None,
            opened_at: Instant::now(),
        });
        self.hex_input.update(cx, |input, cx| input.set_text(to_hex(seed), cx));
        window.focus(&self.plane_focus);
        cx.notify();
    }

    /// Closes the picker. `revert` puts back the color it opened on (Esc);
    /// `refocus` returns focus to the custom swatch (a keyboard or button close).
    fn close_picker(&mut self, revert: bool, refocus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.color_picker.take() else { return };
        if revert {
            let target = picker.target;
            self.app.update(cx, |app, cx| app.update_settings(cx, |s| *target.field(s) = picker.original));
        }
        if refocus {
            window.focus(&self.custom_focus(picker.target).clone());
        }
        cx.notify();
    }

    /// A preset clears the custom color; it also closes that row's picker.
    fn choose_preset(&mut self, target: ColorTarget, window: &mut Window, cx: &mut Context<Self>, patch: impl FnOnce(&mut Settings)) {
        if self.color_picker.as_ref().is_some_and(|picker| picker.target == target) {
            self.close_picker(false, false, window, cx);
        }
        self.app.update(cx, |app, cx| app.update_settings(cx, patch));
    }

    /// Moves the picker to `hsv` and saves it (the save is debounced).
    fn set_hsv(&mut self, hsv: Hsv, cx: &mut Context<Self>) {
        let Some(picker) = self.color_picker.as_mut() else { return };
        picker.hsv = hsv;
        let target = picker.target;
        let hex = to_hex(hsv.to_rgba());
        self.app.update(cx, |app, cx| app.update_settings(cx, |s| *target.field(s) = Some(hex)));
        cx.notify();
    }

    /// A typed color applies once it is a whole `#rrggbb`, with or without the `#`.
    fn type_hex(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(picker) = self.color_picker.as_ref() else { return };
        let Some(color) = prism_vocab::settings::parse_hex_color(text).and_then(|hex| Rgba::try_from(hex.as_str()).ok()) else {
            return;
        };
        let hsv = picker.hsv.retarget(color);
        self.set_hsv(hsv, cx);
    }

    fn begin_drag(&mut self, part: DragPart, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.color_picker.as_mut() else { return };
        picker.drag = Some(part);
        let handle = match part {
            DragPart::Plane => self.plane_focus.clone(),
            DragPart::Hue => self.hue_focus.clone(),
        };
        window.focus(&handle);
        self.drag_to(part, position, cx);
        cx.notify();
    }

    fn drag_to(&mut self, part: DragPart, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(picker) = self.color_picker.as_ref() else { return };
        let bounds = match part {
            DragPart::Plane => self.plane_bounds.get(),
            DragPart::Hue => self.hue_bounds.get(),
        };
        let Some(bounds) = bounds else { return };
        let x = ((position.x - bounds.left()) / bounds.size.width).clamp(0.0, 1.0);
        let y = ((position.y - bounds.top()) / bounds.size.height).clamp(0.0, 1.0);
        let mut hsv = picker.hsv;
        match part {
            DragPart::Plane => {
                hsv.s = x;
                hsv.v = 1.0 - y;
            }
            DragPart::Hue => hsv.h = x * 360.0,
        }
        if hsv != picker.hsv {
            self.set_hsv(hsv, cx);
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = self.color_picker.as_mut()
            && picker.drag.take().is_some()
        {
            cx.notify();
        }
    }

    /// Arrows move the plane's thumb or the hue by a small step; Shift takes ten.
    fn nudge(&mut self, part: DragPart, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(picker) = self.color_picker.as_ref() else { return };
        let (dx, dy) = match event.keystroke.key.as_str() {
            "left" => (-1.0, 0.0),
            "right" => (1.0, 0.0),
            "up" => (0.0, 1.0),
            "down" => (0.0, -1.0),
            _ => return,
        };
        cx.stop_propagation();
        let big = event.keystroke.modifiers.shift;
        let mut hsv = picker.hsv;
        match part {
            DragPart::Plane => {
                let step = if big { 0.1 } else { 0.01 };
                hsv.s = (hsv.s + dx * step).clamp(0.0, 1.0);
                hsv.v = (hsv.v + dy * step).clamp(0.0, 1.0);
            }
            DragPart::Hue => {
                let step = if big { 10.0 } else { 1.0 };
                hsv.h = (hsv.h + (dx + dy) * step).clamp(0.0, 360.0);
            }
        }
        if hsv != picker.hsv {
            self.set_hsv(hsv, cx);
        }
    }

    /// A round swatch: the presets' look (lift, top highlight, `scale-110` when
    /// chosen) with a check on the chosen one, so near-identical tones still read.
    #[allow(clippy::too_many_arguments)]
    fn swatch(
        &self,
        key: gpui::ElementId,
        fill: Rgba,
        mark: Rgba,
        active: bool,
        edge: bool,
        label: &str,
        t: &Theme,
        zoom: f32,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Stateful<gpui::Div> {
        // `transition-transform duration-150`: `scale-110` when active, else `hover:scale-105`.
        let (hovered, hover) = transition::hover(&key, window, cx);
        let target = if active { 1.1 } else if hovered { 1.05 } else { 1.0 };
        let scale = transition::number(&key, "scale", target, Timing::tw(150), window, cx);
        let check = transition::number(&key, "check", if active { 1.0 } else { 0.0 }, Timing::tw(150), window, cx);
        focus::button(div().id(key), true, t)
            .tooltip(title_tooltip(label.to_owned(), t.mode))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(r(24.))
            .scale(scale)
            .rounded_full()
            .bg(hsla(fill))
            .when(edge, |d| d.border_1().border_color(hsla(t.divider)))
            .when(!active, |d| {
                d.shadow(zoomed(vec![shadow(0., 1., 3., 0., t.shade(0.4))], zoom))
                    .child(crate::components::ui::inset_top_highlight(white(0.2), 12.))
            })
            .child(icon_stroke("check", 2.5, 12., Rgba { a: mark.a * check, ..mark }))
            // `after:-inset-1`: the pseudo-element is part of the button's `:hover`.
            .child(hover.top(r(-4.)).right(r(-4.)).bottom(r(-4.)).left(r(-4.)))
    }

    /// The last swatch of a row: the custom color once there is one, else a plus.
    /// It opens and closes the picker.
    fn custom_swatch(
        &self,
        target: ColorTarget,
        custom: Option<Rgba>,
        t: &Theme,
        zoom: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.color_picker.as_ref().is_some_and(|picker| picker.target == target);
        let key: gpui::ElementId = match target {
            ColorTarget::Accent => "accent-custom".into(),
            ColorTarget::Background => "background-custom".into(),
        };
        let handle = self.custom_focus(target).clone();
        let toggle = cx.listener(move |this, _: &gpui::ClickEvent, window, cx| this.toggle_picker(target, window, cx));
        let swatch = match custom {
            Some(fill) => {
                let mark = if target == ColorTarget::Accent { t.accent_fg } else { t.fg };
                self.swatch(key, fill, mark, true, target == ColorTarget::Background, "Custom color", t, zoom, window, cx)
            }
            None => {
                let (hovered, hover) = transition::hover(&key, window, cx);
                let scale = transition::number(&key, "scale", if hovered || open { 1.05 } else { 1.0 }, Timing::tw(150), window, cx);
                let bg = transition::color(&key, "bg", if open { t.surface_active } else { t.surface }, Timing::PRESS, window, cx);
                focus::button(div().id(key), true, t)
                    .tooltip(title_tooltip("Custom color", t.mode))
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(r(24.))
                    .scale(scale)
                    .rounded_full()
                    .bg(hsla(bg))
                    .border_1()
                    .border_color(hsla(t.divider))
                    .child(icon("plus", 12., if open { t.fg } else { t.fg_tertiary }))
                    .child(hover.top(r(-4.)).right(r(-4.)).bottom(r(-4.)).left(r(-4.)))
            }
        };
        swatch.track_focus(&handle).on_click(toggle).into_any_element()
    }

    /// A color setting: its row, and under it the picker while open. A press
    /// anywhere else closes the picker and keeps the color; Esc puts it back.
    fn color_block(&self, target: ColorTarget, row: gpui::Div, t: &Theme, zoom: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let open = self.color_picker.as_ref().is_some_and(|picker| picker.target == target);
        div()
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_picker(false, false, window, cx)))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" && !this.hex_input.read(cx).is_composing() {
                            cx.stop_propagation();
                            this.close_picker(true, true, window, cx);
                        }
                    }))
            })
            .child(row)
            .when(open, |d| d.child(self.picker_panel(t, zoom, window, cx)))
            .into_any_element()
    }

    fn picker_panel(&self, t: &Theme, zoom: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(picker) = self.color_picker.as_ref() else { return div().into_any_element() };
        let hsv = picker.hsv;
        let color = hsv.to_rgba();
        // Enter: opacity with a short drop from the swatches, `--ease-out-soft`. It opens
        // a few times a session, from a click; Esc and outside presses close it at once.
        let reduced = reduced_motion();
        let p = if reduced { 1.0 } else { (picker.opened_at.elapsed().as_millis() as f32 / 160.0).min(1.0) };
        if p < 1.0 {
            window.request_animation_frame();
        }
        let eased = motion::ease_out_soft(p);
        let shift = -6.0 * (1.0 - eased);

        // A drag follows the pointer anywhere in the window until the button comes up.
        let drag = canvas(|_, _, _| {}, {
            let entity = cx.entity().downgrade();
            let part = picker.drag;
            move |_, _, window, _| {
                let Some(part) = part else { return };
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let _ = moved.update(cx, |this, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.drag_to(part, event.position, cx)
                        } else {
                            this.end_drag(cx)
                        }
                    });
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = entity.update(cx, |this, cx| this.end_drag(cx));
                    }
                });
            }
        })
        .absolute()
        .size_0();

        let bounds_probe = |cell: &Rc<Cell<Option<Bounds<Pixels>>>>| {
            let cell = cell.clone();
            canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {}).absolute().size_full()
        };

        let [hue_layer, white_layer, black_layer] = color_picker::plane_layers(hsv.h, 8.);
        let plane = focus::focus_ring(
            div()
                .id("color-plane")
                .track_focus(&self.plane_focus)
                .relative()
                .w_full()
                .h(r(128.))
                .rounded(r(8.))
                .cursor(gpui::CursorStyle::Crosshair)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| this.begin_drag(DragPart::Plane, event.position, window, cx)),
                )
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.nudge(DragPart::Plane, event, cx))),
            t,
        )
        .child(bounds_probe(&self.plane_bounds))
        .child(hue_layer)
        .child(white_layer)
        .child(black_layer)
        // A hairline keeps the white corner off a light panel.
        .child(div().absolute().inset_0().rounded(r(8.)).border_1().border_color(hsla(t.line)))
        .child(color_picker::thumb(hsv.s, 1.0 - hsv.v, color, 14., t, zoom));

        let hue = focus::focus_ring(
            div()
                .id("color-hue")
                .track_focus(&self.hue_focus)
                .relative()
                .w_full()
                .h(r(10.))
                .rounded_full()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| this.begin_drag(DragPart::Hue, event.position, window, cx)),
                )
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.nudge(DragPart::Hue, event, cx))),
            t,
        )
        .child(bounds_probe(&self.hue_bounds))
        .child(color_picker::hue_track(10.))
        .child(color_picker::thumb(hsv.h / 360.0, 0.5, Hsv { s: 1.0, v: 1.0, ..hsv }.to_rgba(), 14., t, zoom));

        // Before and after: the left half is the color it opened on, the right half
        // what the setting paints now (a custom accent keeps a readable lightness).
        let now = picker.target.painted(t);
        let preview = div()
            .flex()
            .flex_none()
            .size(r(28.))
            .rounded(r(7.))
            .border_1()
            .border_color(hsla(t.divider))
            .child(div().flex_1().h_full().rounded_l(r(6.)).bg(hsla(picker.original_paint)))
            .child(div().flex_1().h_full().rounded_r(r(6.)).bg(hsla(now)));

        let input_focused = self.hex_input.read(cx).focus_handle(cx).is_focused(window);
        self.hex_input.update(cx, |input, cx| {
            input.style = field_style(t);
            // Off focus the field follows the picker, so a partial entry reverts.
            if !input_focused {
                input.set_text(to_hex(color), cx);
            }
        });
        let hex = div()
            .flex()
            .flex_1()
            .min_w_0()
            .h(r(28.))
            .items_center()
            .rounded(r(7.))
            .bg(hsla(t.field_bg))
            .border_1()
            .border_color(hsla(if input_focused { t.accent_ring } else { t.field_line }))
            .px(r(8.))
            .text_size(r(12.))
            .line_height(relative(1.45))
            .child(tabular_nums(div().w_full().child(self.hex_input.clone())));

        let done = {
            let key = gpui::ElementId::Name("color-done".into());
            let paint = transition::hover_paint(&key, (t.fg, t.fg), (t.accent_soft, t.surface_hover), Timing::PRESS, window, cx);
            let button = transition::press(div().id(key.clone()), &key, true, window, cx);
            focus::button(button, true, t)
                .flex_none()
                .rounded(r(7.))
                .bg(hsla(paint.bg))
                .px(r(10.))
                .py(r(4.))
                .text_size(r(11.))
                .font_weight(weight(600.))
                .line_height(relative(1.45))
                .text_color(hsla(paint.text))
                .on_click(cx.listener(|this, _, window, cx| this.close_picker(false, true, window, cx)))
                .child("Done")
                .child(paint.tracker)
        };

        div()
            .id("color-picker")
            .relative()
            .top(r(shift))
            .opacity(eased)
            .mb(r(12.))
            .flex()
            .flex_col()
            .gap(r(10.))
            .rounded(r(10.))
            .bg(hsla(t.surface))
            .border_1()
            .border_color(hsla(t.line))
            .p(r(10.))
            // Enter in the field or on the plane keeps the color and closes. On the
            // release, as GPUI clicks: closing on the press would move focus to the
            // swatch, and the release would click it open again.
            .on_key_up(cx.listener(|this, event: &gpui::KeyUpEvent, window, cx| {
                if event.keystroke.key == "enter" && !this.hex_input.read(cx).is_composing() {
                    cx.stop_propagation();
                    this.close_picker(false, true, window, cx);
                }
            }))
            .child(drag)
            .child(plane)
            .child(div().px(r(7.)).child(hue))
            .child(div().flex().items_center().gap(r(8.)).child(preview).child(hex).child(done))
            .into_any_element()
    }

    fn group_name_input(&mut self, group: &AppGroup, focused: bool, t: &Theme, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(input) = self.group_names.get(&group.id) {
            if !focused {
                input.update(cx, |input, cx| input.set_text(group.name.clone(), cx));
            }
            input.update(cx, |input, _| input.style = field_style(t));
            return input.clone();
        }
        let group_id = group.id.clone();
        let name = group.name.clone();
        let input = cx.new(|cx| {
            let mut input = TextInput::new("", field_style(t), cx).with_max_len(64);
            input.set_text(name, cx);
            input
        });
        let sub = cx.subscribe(&input, move |this, _, event: &Changed, cx| {
            let name = event.0.to_string();
            this.update_group(&group_id, cx, |group| group.name = name);
        });
        self._subscriptions.push(sub);
        self.group_names.insert(group.id.clone(), input.clone());
        input
    }
}

impl Focusable for SettingsSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/* ---------------- rendering ---------------- */

fn section_title(label: &str, first: bool, t: &Theme) -> gpui::Div {
    div()
        .when(!first, |d| d.border_t_1().border_color(hsla(t.line)).pt(r(16.)))
        .pb(r(4.))
        .text_size(r(11.))
        .font_weight(weight(600.))
        .line_height(relative(1.45))
        .text_color(hsla(t.fg_quiet))
        .child(label.to_uppercase())
}

/// A small pill button used by several pickers. Colors move like `.press`.
fn chip(id: impl Into<gpui::ElementId>, active: bool, t: &Theme, window: &mut Window, cx: &mut App) -> gpui::Stateful<gpui::Div> {
    let id: gpui::ElementId = id.into();
    let paint = if active {
        transition::hover_paint(&id, (t.fg, t.fg), (t.accent_soft, t.accent_soft), Timing::PRESS, window, cx)
    } else {
        transition::hover_paint(&id, (t.fg_tertiary, t.fg_secondary), (t.surface, t.surface_hover), Timing::PRESS, window, cx)
    };
    focus::button(div().id(id), true, &t)
        .flex()
        .items_center()
        .rounded(r(9.))
        .px(r(8.))
        .py(r(4.))
        .text_size(r(11.))
        .font_weight(weight(500.))
        .line_height(relative(1.45))
        .bg(hsla(paint.bg))
        .text_color(hsla(paint.text))
        .child(paint.tracker)
}

impl SettingsSheet {
    fn keybind_picker(&self, settings: &Settings, t: &Theme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.shortcut_busy;
        div()
            .flex()
            .w_full()
            .min_w_0()
            .flex_col()
            .items_end()
            .gap(r(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(r(8.))
                    .child(
                        div()
                            .text_size(r(12.))
                            .font_weight(weight(500.))
                            .line_height(relative(1.45))
                            .text_color(hsla(t.fg))
                            .child(display_shortcut(&settings.shortcut)),
                    )
                    .child(icon("keyboard", 14., t.fg_tertiary)),
            )
            .child(div().flex().flex_wrap().justify_end().gap(r(6.)).children(prism_vocab::settings::SHORTCUTS.iter().enumerate().map(
                |(index, option)| {
                    let active = settings.shortcut == *option;
                    let value = (*option).to_owned();
                    let key: gpui::ElementId = ("shortcut", index).into();
                    let opacity = transition::number(&key, "opacity", if busy { 0.5 } else { 1.0 }, Timing::PRESS, window, cx);
                    let chip = chip(key.clone(), active, t, window, cx);
                    transition::press(chip, &key, !busy, window, cx)
                        .opacity(opacity)
                        .when(!busy, |d| d.on_click(cx.listener(move |this, _, _, cx| this.apply_shortcut(value.clone(), cx))))
                        .child(shortcut_label(option))
                },
            )))
            .when(settings.shortcut == prism_vocab::settings::WIN_SHORTCUT, |d| {
                d.child(
                    div()
                        .max_w(r(250.))
                        .text_right()
                        .text_size(r(11.))
                        .line_height(relative(1.625))
                        .text_color(hsla(t.fg_tertiary))
                        .child(crate::components::pretty_text::pretty("Standalone Win opens only Prism, including with replacement Start menus. Win + key shortcuts keep working.")),
                )
            })
            .into_any_element()
    }

    fn view_zoom_control(&self, settings: &Settings, t: &Theme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let zoom = settings.view_zoom;
        let at_min = zoom == VIEW_ZOOM_LEVELS[0];
        let at_max = zoom == VIEW_ZOOM_LEVELS[VIEW_ZOOM_LEVELS.len() - 1];
        let at_default = zoom == Settings::default().view_zoom;
        let button = |id: &'static str,
                      title: &'static str,
                      glyph: &'static str,
                      disabled: bool,
                      action: Box<dyn Fn(&mut AppState, &mut Context<AppState>)>,
                      app: Entity<AppState>,
                      window: &mut Window,
                      cx: &mut App| {
            // `.press`: color, background-color, opacity, and scale.
            let key = gpui::ElementId::Name(id.into());
            // The hover classes have no `enabled:` guard, and `:hover` also matches a disabled button.
            let paint = transition::hover_paint(&key, (t.fg_tertiary, t.fg), (TRANSPARENT, t.surface_hover), Timing::PRESS, window, cx);
            let opacity = transition::number(&key, "opacity", if disabled { 0.35 } else { 1.0 }, Timing::PRESS, window, cx);
            let button = transition::press(div().id(key.clone()), &key, !disabled, window, cx);
            focus::button(button, !disabled, &t)
                .tooltip(title_tooltip(title, t.mode))
                .flex()
                .items_center()
                .justify_center()
                .size(r(28.))
                .rounded(r(7.))
                .bg(hsla(paint.bg))
                .opacity(opacity)
                .when(!disabled, |d| d.on_click(move |_, _, cx| app.update(cx, |app, cx| action(app, cx))))
                .child(icon(glyph, 14., paint.text))
                .child(paint.tracker)
        };
        div()
            .flex()
            .items_center()
            .gap(r(4.))
            .rounded(r(10.))
            .bg(hsla(t.surface))
            .p(r(3.))
            .child(button(
                "zoom-out",
                "Zoom out",
                "minus",
                at_min,
                Box::new(move |app, cx| {
                    let next = step_view_zoom(zoom, -1);
                    app.update_settings(cx, |s| s.view_zoom = next)
                }),
                self.app.clone(),
                window,
                cx,
            ))
            .child(tabular_nums(
                div()
                    .w(r(44.))
                    .text_center()
                    .text_size(r(12.))
                    .font_weight(weight(500.))
                    .line_height(relative(1.45))
                    .text_color(hsla(t.fg))
                    .child(format!("{}%", crate::js::number_to_string(zoom))),
            ))
            .child(button(
                "zoom-in",
                "Zoom in",
                "plus",
                at_max,
                Box::new(move |app, cx| {
                    let next = step_view_zoom(zoom, 1);
                    app.update_settings(cx, |s| s.view_zoom = next)
                }),
                self.app.clone(),
                window,
                cx,
            ))
            .child(button(
                "zoom-reset",
                "Reset zoom",
                "rotate-ccw",
                at_default,
                Box::new(|app, cx| app.reset_view_zoom(cx)),
                self.app.clone(),
                window,
                cx,
            ))
            .into_any_element()
    }

    fn quick_access_picker(&self, settings: &Settings, t: &Theme, window: &mut Window, cx: &mut App) -> AnyElement {
        let app = self.app.clone();
        div()
            .grid()
            .grid_cols(2)
            .w(r(198.))
            .gap(r(6.))
            .children(QUICK_ACCESS_OPTIONS.iter().enumerate().map(|(index, (kind, label, glyph))| {
                let active = settings.quick_access.iter().any(|k| k == kind);
                let disabled = !active && settings.quick_access.len() >= QUICK_ACCESS_LIMIT;
                let kind = kind.to_string();
                let app = app.clone();
                // `.press`: color, background-color, opacity, and scale.
                let key: gpui::ElementId = ("quick-access-option", index).into();
                let paint = if active {
                    transition::hover_paint(&key, (t.fg, t.fg), (t.accent_soft, t.accent_soft), Timing::PRESS, window, cx)
                } else {
                    transition::hover_paint(&key, (t.fg_tertiary, t.fg_secondary), (t.surface, t.surface_hover), Timing::PRESS, window, cx)
                };
                let opacity = transition::number(&key, "opacity", if disabled { 0.35 } else { 1.0 }, Timing::PRESS, window, cx);
                let button = transition::press(div().id(key.clone()), &key, !disabled, window, cx);
                focus::button(button, true, &t)
                    .flex()
                    .h(r(32.))
                    .min_w_0()
                    .items_center()
                    .gap(r(6.))
                    .rounded(r(8.))
                    .px(r(8.))
                    .text_size(r(11.))
                    .font_weight(weight(500.))
                    .line_height(relative(1.45))
                    .bg(hsla(paint.bg))
                    .text_color(hsla(paint.text))
                    .opacity(opacity)
                    .when(!disabled, |d| {
                        d.on_click(move |_, _, cx| {
                            let kind = kind.clone();
                            app.update(cx, |app, cx| {
                                app.update_settings(cx, |s| {
                                    if s.quick_access.contains(&kind) {
                                        s.quick_access.retain(|k| k != &kind);
                                    } else {
                                        s.quick_access.push(kind);
                                    }
                                })
                            })
                        })
                    })
                    // The icon is `currentColor` unless active (`text-accent`): it follows the chip text.
                    .child(icon(glyph, 14., if active { t.accent } else { paint.text }))
                    .child(truncate(*label))
                    .child(paint.tracker)
            }))
            .into_any_element()
    }

    fn app_groups_picker(&mut self, settings: &Settings, t: &Theme, zoom: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let apps = self.palette.read(cx).apps().to_vec();
        let apps_loaded = self.palette.read(cx).apps_loaded;
        let new_name_focused = self.new_name.read(cx).focus_handle(cx).is_focused(window);
        self.new_name.update(cx, |input, _| input.style = field_style(t));
        self.app_query.update(cx, |input, _| input.style = field_style(t));
        let mut root = div().w(r(258.)).min_w_0().flex().flex_col().gap(r(8.)).child(
            div()
                .flex()
                .gap(r(6.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .rounded(r(8.))
                        .bg(hsla(t.surface))
                        .px(r(10.))
                        .py(r(6.))
                        .text_size(r(12.))
                        .line_height(relative(1.45))
                        .when(new_name_focused, |d| d.shadow(field_ring(t, zoom)))
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            if event.keystroke.key == "enter" && !this.new_name.read(cx).is_composing() {
                                this.create_group(cx);
                            }
                        }))
                        .child(self.new_name.clone()),
                )
                .child({
                    let key = gpui::ElementId::Name("create-collection".into());
                    let paint = transition::hover_paint(&key, (t.accent, t.accent), (t.accent_soft, t.surface_hover), Timing::PRESS, window, cx);
                    let button = transition::press(div().id(key.clone()), &key, true, window, cx);
                    focus::button(button, true, &t)
                        .tooltip(title_tooltip("Create app collection", t.mode))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(r(32.))
                        .rounded(r(8.))
                        .bg(hsla(paint.bg))
                        .on_click(cx.listener(|this, _, _, cx| this.create_group(cx)))
                        .child(icon("plus", 16., paint.text))
                        .child(paint.tracker)
                }),
        );
        let groups = settings.app_groups.clone();
        let filtered = if self.open_group_id.is_some() { self.filtered_apps(cx) } else { Vec::new() };
        for (group_index, group) in groups.iter().enumerate() {
            let name_input_focused = self
                .group_names
                .get(&group.id)
                .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window));
            let name_input = self.group_name_input(group, name_input_focused, t, cx);
            let assigned: Vec<AppEntry> =
                group.app_ids.iter().filter_map(|id| apps.iter().find(|entry| &entry.app_id == id).cloned()).collect();
            let is_open = self.open_group_id.as_deref() == Some(group.id.as_str());
            let first = group_index == 0;
            let last = group_index + 1 == groups.len();
            let gid = group.id.clone();
            let mut block = div()
                .border_t_1()
                .border_color(hsla(t.line))
                .pt(r(8.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(r(6.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(r(12.))
                                .font_weight(weight(600.))
                                .line_height(relative(1.45))
                                .when(name_input_focused, |d| d.shadow(field_ring(t, zoom)))
                                .child(name_input),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(r(2.))
                                .child(self.small_icon_button(gpui::ElementId::Name(format!("group-up-{}", group.id).into()), "Move up", "chevron-up", first, false, t, {
                                    let gid = gid.clone();
                                    cx.listener(move |this, _, _, cx| this.move_group(&gid, -1, cx))
                                }, window, cx))
                                .child(self.small_icon_button(gpui::ElementId::Name(format!("group-down-{}", group.id).into()), "Move down", "chevron-down", last, false, t, {
                                    let gid = gid.clone();
                                    cx.listener(move |this, _, _, cx| this.move_group(&gid, 1, cx))
                                }, window, cx))
                                .child(self.small_icon_button(gpui::ElementId::Name(format!("group-delete-{}", group.id).into()), "Delete collection", "trash-2", false, true, t, {
                                    let gid = gid.clone();
                                    cx.listener(move |this, _, _, cx| this.remove_group(&gid, cx))
                                }, window, cx)),
                        ),
                );
            // The picker trigger.
            // `transition-transform duration-150` with the default Tailwind timing.
            let chevron = window.use_keyed_state(
                gpui::ElementId::Name(format!("group-chevron-{}", group.id).into()),
                cx,
                |_, _| Tween::at(if is_open { 1.0 } else { 0.0 }),
            );
            chevron.update(cx, |tween, _| {
                tween.set(if is_open { 1.0 } else { 0.0 }, crate::gfx::motion_ms(150), motion::tailwind_default)
            });
            let chevron_turn = std::f32::consts::PI * chevron.read(cx).animate(window);
            // `shadow-[inset_0_0_0_1px_var(--accent-ring)]` (utilities) overrides `.focus-ring` while open.
            // `transition-colors duration-150`.
            let trigger_key: gpui::ElementId = ("group-add-apps", group_index).into();
            let paint = if is_open {
                transition::hover_paint(&trigger_key, (t.fg, t.fg), (t.surface_active, t.surface_active), Timing::tw(150), window, cx)
            } else {
                transition::hover_paint(&trigger_key, (t.fg_secondary, t.fg), (t.surface, t.surface_hover), Timing::tw(150), window, cx)
            };
            let trigger = div()
                .id(trigger_key)
                .tab_index(0)
                .tab_stop(true)
                .when(!is_open, |d| focus::focus_ring(d, &t))
                .flex()
                .h(r(36.))
                .w_full()
                .items_center()
                .gap(r(8.))
                .rounded(r(8.))
                .px(r(10.))
                .text_size(r(11.5))
                .font_weight(weight(500.))
                .line_height(relative(1.45))
                .bg(hsla(paint.bg))
                .text_color(hsla(paint.text))
                .when(is_open, |d| d.border_1().border_color(hsla(t.accent_ring)).px(r(9.)))
                .on_click(cx.listener({
                    let gid = gid.clone();
                    move |this, _, window, cx| this.toggle_app_picker(&gid, window, cx)
                }))
                .child(icon("plus", 14., t.accent))
                .child(truncate(if is_open { "Choose apps" } else { "Add apps" }).flex_1())
                .when(!group.app_ids.is_empty(), |d| {
                    d.child(tabular_nums(
                        div()
                            .flex_none()
                            .text_size(r(10.5))
                            .font_weight(weight(600.))
                            .text_color(hsla(t.fg_tertiary))
                            .child(group.app_ids.len().to_string()),
                    ))
                })
                .child(crate::gfx::rotated(icon("chevron-down", 14., t.fg_quiet), chevron_turn))
                .child(paint.tracker);
            let mut picker_block = div().mt(r(6.)).child(trigger);
            if is_open {
                picker_block = picker_block
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.open_group_id = None;
                        cx.notify();
                    }))
                    .child(self.app_picker(group, &filtered, apps_loaded, &settings.app_groups, t, zoom, window, cx));
            }
            block = block.child(picker_block);
            if !assigned.is_empty() && !is_open {
                block = block.child(div().mt(r(6.)).flex().flex_wrap().gap(r(4.)).children(assigned.iter().enumerate().map(
                    |(index, entry)| {
                        let gid = gid.clone();
                        let app_id = entry.app_id.clone();
                        let key = gpui::ElementId::Name(format!("assigned-{}-{index}", group.id).into());
                        let paint = transition::hover_paint(&key, (t.fg, t.fg), (t.accent_soft, t.surface_hover), Timing::PRESS, window, cx);
                        let button = transition::press(div().id(key.clone()), &key, true, window, cx);
                        focus::button(button, true, &t)
                            .tooltip(title_tooltip(format!("Remove {}", entry.name), t.mode))
                            .flex()
                            .max_w_full()
                            .items_center()
                            .gap(r(4.))
                            .rounded(r(6.))
                            .bg(hsla(paint.bg))
                            .px(r(8.))
                            .py(r(4.))
                            .text_size(r(10.5))
                            .font_weight(weight(500.))
                            .line_height(relative(1.45))
                            .text_color(hsla(paint.text))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_app(&gid, &app_id, cx)))
                            .child(truncate(entry.name.clone()))
                            .child(icon("x", 12., t.fg_quiet))
                            .child(paint.tracker)
                    },
                )));
            }
            root = root.child(block);
        }
        root.into_any_element()
    }

    fn small_icon_button(
        &self,
        id: impl Into<gpui::ElementId>,
        title: &'static str,
        glyph: &str,
        disabled: bool,
        danger: bool,
        t: &Theme,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Stateful<gpui::Div> {
        // `.press`: color, background-color, opacity, and scale.
        let id: gpui::ElementId = id.into();
        let (text, bg) = if danger { (t.danger, t.danger_soft) } else { (t.fg, t.surface_hover) };
        // The hover classes have no `enabled:` guard, and `:hover` also matches a disabled button.
        let paint = transition::hover_paint(&id, (t.fg_quiet, text), (TRANSPARENT, bg), Timing::PRESS, window, cx);
        let opacity = transition::number(&id, "opacity", if disabled { 0.25 } else { 1.0 }, Timing::PRESS, window, cx);
        let button = transition::press(div().id(id.clone()), &id, !disabled, window, cx);
        focus::button(button, !disabled, &t)
            .tooltip(title_tooltip(title, t.mode))
            .flex()
            .items_center()
            .justify_center()
            .size(r(28.))
            .rounded(r(7.))
            .bg(hsla(paint.bg))
            .opacity(opacity)
            .when(!disabled, |d| d.on_click(on_click))
            .child(icon(glyph, 14., paint.text))
            .child(paint.tracker)
    }

    #[allow(clippy::too_many_arguments)]
    fn app_picker(
        &mut self,
        group: &AppGroup,
        filtered: &[AppEntry],
        apps_loaded: bool,
        all_groups: &[AppGroup],
        t: &Theme,
        zoom: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query_focused = self.app_query.read(cx).focus_handle(cx).is_focused(window);
        let has_query = !self.app_query.read(cx).text().is_empty();
        let gid = group.id.clone();
        let assigned_ids: HashSet<&String> = group.app_ids.iter().collect();
        let full = group.app_ids.len() >= APP_GROUP_APP_LIMIT;
        let list: AnyElement = if !apps_loaded {
            div().px(r(8.)).py(r(20.)).text_center().text_size(r(11.)).text_color(hsla(t.fg_quiet)).child("Loading installed apps...").into_any_element()
        } else if filtered.is_empty() {
            div().px(r(8.)).py(r(20.)).text_center().text_size(r(11.)).text_color(hsla(t.fg_quiet)).child("No matching apps").into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(filtered.iter().enumerate().map(|(index, entry)| {
                    let owner = all_groups.iter().find(|candidate| candidate.app_ids.contains(&entry.app_id));
                    let selected = assigned_ids.contains(&entry.app_id);
                    let disabled = !selected && full;
                    let (gid, app_id) = (gid.clone(), entry.app_id.clone());
                    let icon_value = PaletteIcon::App { name: entry.name.clone(), icon: entry.icon.clone() };
                    // `transition-colors duration-100`.
                    let key = gpui::ElementId::Name(format!("picker-app-{}", entry.app_id).into());
                    let paint = if selected {
                        transition::hover_paint(&key, (t.fg, t.fg), (t.accent_soft, t.accent_soft), Timing::tw(100), window, cx)
                    } else {
                        transition::hover_paint(&key, (t.fg_secondary, t.fg), (TRANSPARENT, t.surface_hover), Timing::tw(100), window, cx)
                    };
                    focus::button(div().id(("picker-app", index)), !disabled, &t)
                        .when(disabled, |d| d.tooltip(title_tooltip(format!("{} is full", group.name), t.mode)))
                        .flex()
                        .min_h(r(40.))
                        .w_full()
                        .items_center()
                        .gap(r(8.))
                        .rounded(r(7.))
                        .px(r(8.))
                        .py(r(4.))
                        .bg(hsla(paint.bg))
                        .text_color(hsla(paint.text))
                        .when(disabled, |d| d.opacity(0.35))
                        .when(!disabled, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                if selected {
                                    this.remove_app(&gid, &app_id, cx)
                                } else {
                                    this.assign_app(&gid, &app_id, cx)
                                }
                            }))
                        })
                        .child(row_icon(&icon_value, 26., t, zoom, window, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(truncate(entry.name.clone()).text_size(r(11.5)).font_weight(weight(500.)).line_height(relative(1.45)))
                                .when_some(owner.filter(|_| !selected), |d, owner| {
                                    d.child(
                                        truncate(format!("In {}", owner.name))
                                            .mt(r(2.))
                                            .text_size(r(10.))
                                            .line_height(relative(1.45))
                                            .text_color(hsla(t.fg_quiet)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .size(r(20.))
                                .rounded(r(6.))
                                .bg(hsla(if selected { t.accent } else { t.surface }))
                                .child(icon_stroke("check", 2.5, 14., if selected { t.accent_fg } else { crate::theme::TRANSPARENT })),
                        )
                        .child(paint.tracker)
                }))
                .into_any_element()
        };
        div()
            .mt(r(6.))
            .overflow_hidden()
            .rounded(r(9.))
            .bg(hsla(t.surface))
            .border_1()
            .border_color(hsla(t.line))
            .child(
                div()
                    .m(r(5.))
                    .flex()
                    .h(r(32.))
                    .items_center()
                    .gap(r(8.))
                    .rounded(r(7.))
                    .bg(hsla(t.field_bg))
                    .px(r(9.))
                    .border_1()
                    .border_color(hsla(if query_focused { t.accent_ring } else { t.field_line }))
                    .child(icon("search", 14., t.fg_quiet))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(r(11.5))
                            .line_height(relative(1.45))
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                if event.keystroke.key == "escape" && !this.app_query.read(cx).is_composing() {
                                    cx.stop_propagation();
                                    this.open_group_id = None;
                                    cx.notify();
                                }
                            }))
                            .child(self.app_query.clone()),
                    )
                    .when(has_query, |d| {
                        d.child(
                            focus::button(div().id("clear-app-filter"), true, &t)
                                .tooltip(title_tooltip("Clear filter", t.mode))
                                .group("clear-filter")
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .size(r(24.))
                                .rounded(r(6.))
                                .hover(|s| s.bg(hsla(t.surface_hover)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.app_query.update(cx, |input, cx| input.set_text("", cx));
                                    this.app_query.read(cx).focus(window);
                                    cx.notify();
                                }))
                                .child(icon("x", 14., t.fg_quiet).group_hover("clear-filter", |s| s.text_color(hsla(t.fg)))),
                        )
                    }),
            )
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("app-picker-list-{}", group.id).into()))
                            .max_h(r(224.))
                            .overflow_y_scroll()
                            .track_scroll(&self.picker_scroll)
                            .pr(self.picker_wheel.borrow().gutter())
                            .child(div().px(r(5.)).pb(r(6.)).child(list)),
                    )
                    .child(crate::gfx::wheel_scroll_layer(self.picker_scroll.clone(), self.picker_wheel.clone(), t.scroll)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(hsla(t.line))
                    // `px-2.5 py-2` less the 1px ring-as-border of the panel (an inset shadow in CSS).
                    .px(r(9.))
                    .pt(r(8.))
                    .pb(r(7.))
                    .child(tabular_nums(
                        div()
                            .text_size(r(10.5))
                            .line_height(relative(1.45))
                            .text_color(hsla(t.fg_quiet))
                            .child(format!("{} of {APP_GROUP_APP_LIMIT} selected", group.app_ids.len())),
                    ))
                    .child(
                        {
                        // `.press`: background-color and scale.
                        let key = gpui::ElementId::Name("app-picker-done".into());
                        let paint = transition::hover_paint(&key, (t.fg, t.fg), (t.accent_soft, t.surface_hover), Timing::PRESS, window, cx);
                        let button = transition::press(div().id(key.clone()), &key, true, window, cx);
                        focus::button(button, true, &t)
                            .rounded(r(7.))
                            .bg(hsla(paint.bg))
                            .px(r(10.))
                            .py(r(4.))
                            .text_size(r(11.))
                            .font_weight(weight(600.))
                            .line_height(relative(1.45))
                            .text_color(hsla(paint.text))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_group_id = None;
                                cx.notify();
                            }))
                            .child("Done")
                            .child(paint.tracker)
                        },
                    ),
            )
            .into_any_element()
    }
}

impl Render for SettingsSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.app.read(cx).open_settings {
            return div().into_any_element();
        }
        focus::reveal_focused(&self.scroll, "SettingsScroll", &mut self.revealed, window, cx);
        let app = self.app.read(cx);
        let (t, zoom) = theme(cx);
        let settings = app.settings.clone();
        let persistence_error = app.persistence_error.clone();

        // settings-backdrop 120 ms / -exit 110 ms; settings-panel 180 ms / -exit 140 ms.
        let reduced = reduced_motion();
        let (backdrop_opacity, panel_opacity, panel_shift, animating) = match self.closing {
            Some(since) => {
                let ms = since.elapsed().as_millis() as f32;
                let b = if reduced { 1.0 } else { (ms / 110.0).min(1.0) };
                let p = if reduced { 1.0 } else { (ms / 140.0).min(1.0) };
                (1.0 - motion::ease_out_soft(b), 1.0 - motion::ease_out_soft(p), 32.0 * motion::ease_out_soft(p), b < 1.0 || p < 1.0)
            }
            None => {
                let ms = self.opened_at.elapsed().as_millis() as f32;
                let b = if reduced { 1.0 } else { (ms / 120.0).min(1.0) };
                let p = if reduced { 1.0 } else { (ms / 180.0).min(1.0) };
                let e = motion::ease_out_soft(p);
                (motion::ease_out_soft(b), e, 32.0 * (1.0 - e), b < 1.0 || p < 1.0)
            }
        };
        if animating {
            window.request_animation_frame();
        }
        let panel_shadow = zoomed(vec![shadow(-24., 0., 64., 0., crate::theme::black(0.35 * panel_opacity))], zoom);
        let panel_radii = gpui::Corners {
            top_left: gpui::px(0.),
            top_right: gpui::px(24. * zoom),
            bottom_right: gpui::px(8. * zoom),
            bottom_left: gpui::px(0.),
        };

        let reset_confirming = self.reset_confirming;
        let reset_busy = self.reset_busy;
        let app_entity = self.app.clone();
        let theme_options: Vec<SegmentOption> = prism_vocab::settings::THEMES
            .iter()
            .zip(THEME_LABELS)
            .map(|(value, label)| SegmentOption { value: (*value).into(), label: label.into() })
            .collect();
        let appearance = segmented("appearance", &settings.theme, theme_options, false, &t, zoom, window, cx, {
            let app = app_entity.clone();
            move |value, _, cx| app.update(cx, |app, cx| app.update_settings(cx, |s| s.theme = value.to_string()))
        });
        let width = segmented(
            "window-width",
            &crate::js::number_to_string(settings.width),
            prism_vocab::settings::WIDTHS
                .iter()
                .zip(["S", "M", "L"])
                .map(|(width, label)| SegmentOption { value: width.to_string().into(), label: label.into() })
                .collect(),
            false,
            &t,
            zoom,
            window,
            cx,
            {
                let app = app_entity.clone();
                move |value: SharedString, _, cx| {
                    let width: f64 = value.parse().unwrap_or(f64::from(prism_vocab::settings::DEFAULT_WIDTH));
                    app.update(cx, |app, cx| app.update_settings(cx, |s| s.width = width))
                }
            },
        );
        let alignment = {
            let this = cx.entity().downgrade();
            let control = segmented(
                "taskbar-alignment",
                &settings.taskbar_alignment,
                vec![
                    SegmentOption { value: "left".into(), label: "Left".into() },
                    SegmentOption { value: "center".into(), label: "Center".into() },
                    SegmentOption { value: "right".into(), label: "Right".into() },
                ],
                self.alignment_busy,
                &t,
                zoom,
                window,
                cx,
                move |value, _, cx| {
                    let _ = this.update(cx, |this, cx| this.apply_alignment(value.to_string(), cx));
                },
            );
            div().flex().flex_col().items_start().child(control).child(
                div()
                    .mt(r(6.))
                    .max_w(r(250.))
                    .text_right()
                    .text_size(r(11.))
                    .line_height(relative(1.625))
                    .text_color(hsla(t.fg_tertiary))
                    .child(crate::components::pretty_text::pretty("Right alignment requires the StartAllBack classic taskbar.")),
            )
        };
        let scroll_volume = toggle("scroll-volume", settings.taskbar_scroll_volume, false, &t, zoom, window, cx, {
            let app = app_entity.clone();
            move |value, _, cx| app.update(cx, |app, cx| app.update_settings(cx, |s| s.taskbar_scroll_volume = value))
        });
        let keep_on_top = toggle("keep-on-top", settings.always_on_top, false, &t, zoom, window, cx, {
            let app = app_entity.clone();
            move |value, _, cx| app.update(cx, |app, cx| app.update_settings(cx, |s| s.always_on_top = value))
        });
        // Accent: the five presets, then the custom swatch.
        let custom_accent = settings.accent_color.as_ref().map(|_| t.accent);
        let mut accents = div().flex().items_center().gap(r(8.));
        for (index, accent) in Accent::ALL.iter().enumerate() {
            let active = settings.accent_color.is_none() && settings.accent == accent.id();
            let id = accent.id().to_owned();
            let (_, _, mark) = accent.colors();
            let label = format!("{}{}", &id[..1].to_uppercase(), &id[1..]);
            accents = accents.child(
                self.swatch(("accent", index).into(), accent.swatch(), mark, active, false, &label, &t, zoom, window, cx).on_click(
                    cx.listener(move |this, _, window, cx| {
                        let id = id.clone();
                        this.choose_preset(ColorTarget::Accent, window, cx, |s| {
                            s.accent = id;
                            s.accent_color = None;
                        })
                    }),
                ),
            );
        }
        let accents = accents.child(self.custom_swatch(ColorTarget::Accent, custom_accent, &t, zoom, window, cx));

        // Background: the default, five tones, then the custom swatch. Each swatch shows
        // the shell it paints in this mode, not the stored color.
        let preset_hexes: Vec<String> = (0..BACKGROUND_PRESETS.len()).map(|index| to_hex(background_preset(index))).collect();
        let stored_background = settings.background_color.clone();
        let mut backgrounds = div().flex().items_center().gap(r(8.)).child(
            self.swatch("background-default".into(), t.default_background(), t.fg, stored_background.is_none(), true, "Default", &t, zoom, window, cx)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.choose_preset(ColorTarget::Background, window, cx, |s| s.background_color = None)
                })),
        );
        for (index, (label, _, _)) in BACKGROUND_PRESETS.iter().enumerate() {
            let hex = preset_hexes[index].clone();
            let active = stored_background.as_deref() == Some(hex.as_str());
            let fill = t.background_tone(background_preset(index));
            backgrounds = backgrounds.child(
                self.swatch(("background", index).into(), fill, t.fg, active, true, label, &t, zoom, window, cx).on_click(
                    cx.listener(move |this, _, window, cx| {
                        let hex = hex.clone();
                        this.choose_preset(ColorTarget::Background, window, cx, |s| s.background_color = Some(hex))
                    }),
                ),
            );
        }
        let custom_background = stored_background
            .as_deref()
            .filter(|hex| !preset_hexes.iter().any(|preset| preset == hex))
            .map(|_| t.shell_solid.from);
        let backgrounds = backgrounds.child(self.custom_swatch(ColorTarget::Background, custom_background, &t, zoom, window, cx));
        let accent_block = self.color_block(
            ColorTarget::Accent,
            settings_row("Accent color", Some("Colors highlights, focus, and the volume pill"), accents, &t),
            &t,
            zoom,
            window,
            cx,
        );
        let background_block = self.color_block(
            ColorTarget::Background,
            settings_row("Background", Some("Match the launcher to your desktop"), backgrounds, &t),
            &t,
            zoom,
            window,
            cx,
        );
        // `.press` (150ms `--ease-out-soft`) for the buttons below.
        let button_paint = |id: &'static str, text: (Rgba, Rgba), bg: (Rgba, Rgba), window: &mut Window, cx: &mut App| {
            transition::hover_paint(&gpui::ElementId::Name(id.into()), text, bg, Timing::PRESS, window, cx)
        };
        let history_paint = button_paint("clear-history", (t.fg_secondary, t.fg), (t.surface, t.surface_hover), window, cx);
        let close_paint = button_paint("close-settings", (t.fg_tertiary, t.fg_secondary), (TRANSPARENT, t.surface_hover), window, cx);
        let reset_paint = if reset_confirming {
            button_paint("reset-settings", (white(1.0), white(1.0)), (t.danger, t.danger), window, cx)
        } else {
            button_paint("reset-settings", (t.danger, white(1.0)), (t.danger_soft, t.danger), window, cx)
        };
        // `disabled:opacity-50` comes after `hover:opacity-90` in Tailwind's variant order.
        let reset_opacity = if reset_busy { 0.5 } else if reset_confirming && reset_paint.hovered { 0.9 } else { 1.0 };
        let reset_opacity = transition::number(&"reset-settings".into(), "opacity", reset_opacity, Timing::PRESS, window, cx);
        let quit_paint = button_paint("quit-prism", (t.danger, t.danger), (t.danger_soft, t.danger_soft), window, cx);
        let quit_opacity = transition::number(&"quit-prism".into(), "opacity", if quit_paint.hovered { 0.9 } else { 1.0 }, Timing::PRESS, window, cx);
        // `.press:active:not(:disabled)`: the scale.
        let pressable = |id: &'static str, enabled: bool, window: &mut Window, cx: &mut App| {
            let key = gpui::ElementId::Name(id.into());
            transition::press(div().id(key.clone()), &key, enabled, window, cx)
        };
        let history_button = pressable("clear-history", true, window, cx);
        let close_button = pressable("close-settings", true, window, cx);
        let reset_button = pressable("reset-settings", !reset_busy, window, cx);
        let quit_button = pressable("quit-prism", true, window, cx);
        // Build these two only while they are drawn: a hover state that outlives its overlay
        // keeps its last value, and the button would come back hovered.
        let cancel = reset_confirming.then(|| {
            let paint = button_paint("reset-cancel", (t.danger, t.danger), (TRANSPARENT, t.surface_hover), window, cx);
            (pressable("reset-cancel", true, window, cx), paint)
        });
        let retry = persistence_error.map(|error| {
            let busy = self.retry_save_busy;
            let paint = button_paint("retry-save", (t.danger, t.danger), (t.surface, t.surface_hover), window, cx);
            let opacity = transition::number(&"retry-save".into(), "opacity", if busy { 0.5 } else { 1.0 }, Timing::PRESS, window, cx);
            (error, pressable("retry-save", !busy, window, cx), paint, opacity)
        });
        let clear_history = {
            let app = app_entity.clone();
            focus::button(history_button, true, &t)
                .flex()
                .items_center()
                .gap(r(6.))
                .rounded(r(10.))
                .bg(hsla(history_paint.bg))
                .px(r(12.))
                .py(r(6.))
                .text_size(r(12.))
                .font_weight(weight(500.))
                .line_height(relative(1.45))
                .text_color(hsla(history_paint.text))
                .on_click(move |_, _, cx| app.update(cx, |app, cx| app.clear_history(cx)))
                .child(icon("history", 14., history_paint.text))
                .child("Clear")
                .child(history_paint.tracker)
        };
        let keybind = self.keybind_picker(&settings, &t, window, cx);
        let zoom_control = self.view_zoom_control(&settings, &t, window, cx);
        let quick = self.quick_access_picker(&settings, &t, window, cx);
        let groups = self.app_groups_picker(&settings, &t, zoom, window, cx);
        let version = self.version.clone();

        let panel = div()
            .id("settings-panel")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.app.update(cx, |app, cx| app.set_open_settings(false, cx));
                    return;
                }
                // Focus trap: Tab and Shift+Tab cycle within the sheet.
                let modifiers = event.keystroke.modifiers;
                if event.keystroke.key == "tab" && !modifiers.control && !modifiers.alt && !modifiers.platform {
                    cx.stop_propagation();
                    for _ in 0..512 {
                        if modifiers.shift {
                            window.focus_prev();
                        } else {
                            window.focus_next();
                        }
                        if this.focus.contains_focused(window, cx) && !this.focus.is_focused(window) {
                            break;
                        }
                    }
                }
            }))
            .relative()
            .left(r(panel_shift))
            .ml_auto()
            .flex()
            .flex_col()
            .h_full()
            .w(r(340.))
            .max_w(gpui::relative(0.88))
            .overflow_hidden()
            .rounded_tr(r(24.))
            .rounded_br(r(8.))
            .border_l_1()
            .border_color(hsla(t.line))
            // `backdrop-blur-2xl` blurred nothing in the original (the animated
            // `.settings-backdrop` is the backdrop root), so the palette showed through the
            // panel unblurred. The port paints `bg-raised` opaque (D4, deviation 2026-10-28).
            .bg(hsla(Rgba { a: 1.0, ..t.bg_raised }))
            .opacity(panel_opacity)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                gpui::canvas(
                    move |bounds, _, _| {
                        // The border box: the absolute child starts inside the 1px left border.
                        let bounds = gpui::Bounds::from_corners(gpui::point(bounds.left() - gpui::px(1.), bounds.top()), bounds.bottom_right());
                        crate::gfx::set_sheet_panel(Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(r(20.))
                    .pb(r(8.))
                    .pt(r(20.))
                    .child(
                        div()
                            .text_size(r(15.))
                            .font_weight(weight(600.))
                            .line_height(relative(1.45))
                            .text_color(hsla(t.fg))
                            .child("Settings"),
                    )
                    .child(
                        focus::button_with(close_button, &self.close_focus, true, &t)
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(r(44.))
                            .rounded(r(6.))
                            .bg(hsla(close_paint.bg))
                            .on_click(cx.listener(|this, _, _, cx| this.request_close(cx)))
                            .child(icon("x", 16., close_paint.text))
                            .child(close_paint.tracker),
                    ),
            )
            .child(
                div()
                    .mx(r(20.))
                    .mb(r(4.))
                    .border_b_1()
                    .border_color(hsla(t.line))
                    .pb(r(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(r(12.))
                            .child(
                                div()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(r(13.))
                                            .font_weight(weight(600.))
                                            .line_height(relative(1.45))
                                            .text_color(hsla(t.fg))
                                            .child("Reset all settings"),
                                    )
                                    .child(
                                        div()
                                            .mt(r(2.))
                                            .text_size(r(11.5))
                                            .line_height(relative(1.625))
                                            .text_color(hsla(t.fg_tertiary))
                                            .child(crate::components::pretty_text::pretty("Restore Prism's appearance, behavior, pins, and collections.")),
                                    ),
                            )
                            .child(
                                focus::button(reset_button, true, &t)
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(r(6.))
                                    .rounded(r(9.))
                                    .px(r(12.))
                                    .py(r(8.))
                                    .text_size(r(12.))
                                    .font_weight(weight(600.))
                                    .line_height(relative(1.45))
                                    .bg(hsla(reset_paint.bg))
                                    .text_color(hsla(reset_paint.text))
                                    .opacity(reset_opacity)
                                    .when(!reset_busy, |d| d.on_click(cx.listener(|this, _, _, cx| this.confirm_reset(cx))))
                                    .child(icon("rotate-ccw", 14., reset_paint.text))
                                    .child(if reset_confirming { "Reset now" } else { "Reset" })
                                    .child(reset_paint.tracker),
                            ),
                    )
                    .when_some(cancel, |d, (cancel_button, cancel_paint)| {
                        d.child(
                            div()
                                .mt(r(8.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(r(8.))
                                .rounded(r(8.))
                                .bg(hsla(t.danger_soft))
                                .px(r(10.))
                                .py(r(8.))
                                .text_size(r(11.))
                                .line_height(relative(1.45))
                                .text_color(hsla(t.danger))
                                .child("This cannot be undone.")
                                .child(
                                    focus::button(cancel_button, true, &t)
                                        .flex_none()
                                        .rounded(r(6.))
                                        .px(r(8.))
                                        .py(r(4.))
                                        .font_weight(weight(600.))
                                        .bg(hsla(cancel_paint.bg))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reset_confirming = false;
                                            cx.notify();
                                        }))
                                        .child("Cancel")
                                        .child(cancel_paint.tracker),
                                ),
                        )
                    }),
            )
            .when_some(retry, |d, (error, retry_button, retry_paint, retry_opacity)| {
                let busy = self.retry_save_busy;
                d.child(
                    div()
                        .mx(r(20.))
                        .mb(r(12.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(r(12.))
                        .rounded(r(10.))
                        .bg(hsla(t.danger_soft))
                        .px(r(12.))
                        .py(r(10.))
                        .text_color(hsla(t.danger))
                        .child(div().min_w_0().text_size(r(11.5)).line_height(relative(1.625)).child(crate::components::pretty_text::pretty(error)))
                        .child(
                            focus::button(retry_button, true, &t)
                                .flex()
                                .flex_none()
                                .items_center()
                                .min_h(r(44.))
                                .rounded(r(8.))
                                .bg(hsla(retry_paint.bg))
                                .px(r(12.))
                                .text_size(r(11.5))
                                .font_weight(weight(600.))
                                .line_height(relative(1.45))
                                .opacity(retry_opacity)
                                .when(!busy, |d| d.on_click(cx.listener(|this, _, _, cx| this.retry_save(cx))))
                                .child(if busy { "Saving..." } else { "Retry save" })
                                .child(retry_paint.tracker),
                        ),
                )
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                div()
                    .id("settings-scroll")
                    .key_context("SettingsScroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .pr(self.wheel.borrow().gutter())
                    // Chromium's wheel line (gfx::CHROMIUM_LINE_PX); the content resets it.
                    .line_height(px(crate::gfx::CHROMIUM_LINE_PX))
                    .child(div().line_height(relative(1.45)).px(r(20.)).pb(r(24.))
                    .child(section_title("Appearance", true, &t))
                    .child(settings_row("Appearance", Some("Follows Windows light/dark mode"), appearance, &t))
                    .child(accent_block)
                    .child(background_block)
                    .child(settings_row("Window width", None, width, &t))
                    .child(settings_row("View zoom", Some("Ctrl + Up/Down or Ctrl + wheel"), zoom_control, &t))
                    .child(section_title("Taskbar", false, &t))
                    .child(settings_row("Alignment", None, alignment, &t))
                    .child(settings_row(
                        "Scroll volume",
                        Some("Scroll over an app to change its volume, over the empty taskbar or tray for master"),
                        scroll_volume,
                        &t,
                    ))
                    .child(self.taskbar.clone())
                    .child(section_title("Behavior", false, &t))
                    .child(settings_row("Global shortcut", Some("Open Prism from anywhere"), keybind, &t))
                    .child(settings_row("Keep on top", Some("Prism floats above other windows"), keep_on_top, &t))
                    .child(settings_row("Quick Access", Some("Choose up to 6 pinned folders"), quick, &t))
                    .child(settings_row("App collections", Some("Group apps like Creative, Developer, or Games"), groups, &t))
                    .child(settings_row("Recent items", Some("Items are kept in order of use"), clear_history, &t))
                    .child(section_title("About", false, &t))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .py(r(12.))
                            .child(
                                div()
                                    .child(
                                        div()
                                            .text_size(r(13.))
                                            .font_weight(weight(500.))
                                            .line_height(relative(1.45))
                                            .text_color(hsla(t.fg))
                                            .child("Prism"),
                                    )
                                    .child(
                                        div()
                                            .mt(r(2.))
                                            .text_size(r(11.5))
                                            .line_height(relative(1.45))
                                            .text_color(hsla(t.fg_tertiary))
                                            .child(if version.is_empty() { "Version".to_owned() } else { format!("Version {version}") }),
                                    ),
                            )
                            .child(
                                focus::button(quit_button, true, &t)
                                    .flex()
                                    .items_center()
                                    .gap(r(6.))
                                    .min_h(r(44.))
                                    .rounded(r(10.))
                                    .bg(hsla(t.danger_soft))
                                    .px(r(12.))
                                    .text_size(r(12.))
                                    .font_weight(weight(500.))
                                    .line_height(relative(1.45))
                                    .text_color(hsla(t.danger))
                                    .opacity(quit_opacity)
                                    .on_click(move |_, _, cx| app_entity.update(cx, |app, cx| app.quit(cx)))
                                    .child(icon("log-out", 14., t.danger))
                                    .child("Quit")
                                    .child(quit_paint.tracker),
                            ),
                    )),
                    )
                    .child(crate::gfx::wheel_scroll_layer(self.scroll.clone(), self.wheel.clone(), t.scroll)),
            );

        div()
            .id("settings-layer")
            .absolute()
            .inset_0()
            .flex()
            // The backdrop button covers the palette: no hover or wheel reaches it.
            .occlude()
            .child(
                // The backdrop: a click outside the panel closes the sheet.
                div()
                    .id("settings-backdrop")
                    .absolute()
                    .inset_0()
                    .rounded_tl(r(24.))
                    .rounded_tr(r(24.))
                    .rounded_bl(r(8.))
                    .rounded_br(r(8.))
                    .bg(hsla(t.backdrop))
                    .opacity(backdrop_opacity)
                    .on_click(cx.listener(|this, _, _, cx| this.request_close(cx))),
            )
            .child(crate::gfx::sheet_panel_shadow(panel_shadow, panel_radii))
            .child(panel)
            .into_any_element()
    }
}
