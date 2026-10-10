//! The palette (`src/features/palette/Palette.tsx`): search field, sections
//! and rows, reorder and category drags, file drag-out, the result context
//! menu, empty and error states, and the footer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, Context, DispatchPhase, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollHandle,
    ScrollWheelEvent, Styled, Task, Timer, Window, canvas, div, point, prelude::*, px, relative,
};

use crate::components::power_menu::PowerMenu;
use crate::components::settings_sheet::display_shortcut;
use crate::components::text_input::{Changed, TextInput};
use crate::components::ui::{IconButtonProps, icon_button, kbd, row_icon, section_label, theme, truncate, weight};
use crate::features::palette::palette_search_input::{
    SearchInputProps, SearchKey, input_style, render_search_input, search_key,
};
use crate::features::palette::result_context_menu::{
    MenuChoice, MenuEvent, ResultContextMenu, menu_position_for, result_action_count,
};
use crate::features::palette::sections::is_clipboard_kind;
use crate::features::updater::update_control::UpdateControl;
use crate::gfx::{Tween, icon, motion_ms, r, rotated, zoomed};
use crate::js;
use crate::lib::bridge::{self, BackendEvent};
use crate::lib::types::{
    FileSearchError, PINNED_APP_LIMIT, PaletteItem, reorder_app_groups, reorder_pinned_apps, reorder_quick_access,
    reorder_sections,
};
use crate::state::app::{AppEvent, AppState, ToastKind, app_state, error_string};
use crate::state::palette::{PaletteEvent, PaletteState, palette_state, run_action};
use crate::theme::{Rgba, Theme, fade, hsla, motion, shadow};

/// Palette.tsx: `Math.hypot(dx, dy) >= 4`.
const DRAG_THRESHOLD: f32 = 4.0;
/// animatePreviewOut.
const PREVIEW_LEAVE_MS: u64 = 110;

#[derive(Clone, Debug)]
struct ReorderDragState {
    item: PaletteItem,
    start: Point<Pixels>,
    pos: Point<Pixels>,
    active: bool,
    target_item_id: Option<String>,
}

#[derive(Clone, Debug)]
struct FileDragState {
    item: PaletteItem,
    start: Point<Pixels>,
    active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CategoryKind {
    Section,
    Group,
}

#[derive(Clone, Debug)]
struct CategoryDragState {
    kind: CategoryKind,
    id: String,
    label: String,
    start: Point<Pixels>,
    pos: Point<Pixels>,
    active: bool,
    target_id: Option<String>,
}

/// Which drag a pointer-down on a row icon started (the pointer captured by the row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveDrag {
    Reorder,
    File,
    Category,
}

fn reorder_item_id(item: &PaletteItem) -> Option<String> {
    if let Some(app_id) = &item.app_id {
        return Some(format!("app:{app_id}"));
    }
    item.quick_access_kind.as_ref().map(|kind| format!("quick:{kind}"))
}

/// Any action that justifies opening the result context menu.
fn has_result_actions(item: &PaletteItem) -> bool {
    result_action_count(item) > 0
}

/// Hit-test targets recorded during prepaint (A6), in window coordinates.
#[derive(Default)]
struct HitTargets {
    rows: HashMap<usize, Bounds<Pixels>>,
    reorder: Vec<(String, Bounds<Pixels>)>,
    sections: Vec<(String, Bounds<Pixels>)>,
    groups: Vec<(String, Bounds<Pixels>)>,
    viewport: Option<Bounds<Pixels>>,
}

pub struct Palette {
    palette: Entity<PaletteState>,
    app: Entity<AppState>,
    pub input: Entity<TextInput>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    wheel: Rc<RefCell<crate::gfx::WheelScroll>>,
    targets: Rc<RefCell<HitTargets>>,
    reorder_drag: Option<ReorderDragState>,
    category_drag: Option<CategoryDragState>,
    file_drag: Option<FileDragState>,
    active_drag: Option<ActiveDrag>,
    pending_point: Option<Point<Pixels>>,
    frame_scheduled: bool,
    suppress_click: bool,
    leaving_preview: Option<(ReorderDragState, Instant)>,
    leave_task: Option<Task<()>>,
    file_drag_task: Option<Task<()>>,
    result_menu: Option<Entity<ResultContextMenu>>,
    power_menu: Entity<PowerMenu>,
    update_control: Entity<UpdateControl>,
    announce_loading: bool,
    announce_task: Option<Task<()>>,
    reveal_pending: bool,
    hovered_row: Option<usize>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl Palette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let palette = palette_state(cx);
        let app = app_state(cx);
        let (t, _) = theme(cx);
        let input = cx.new(|cx| {
            let mut input = TextInput::new("Search files, folders, and apps\u{2026}", input_style(&t), cx);
            input.ctrl_backspace_clears = true;
            input
        });
        input.read(cx).focus(window);
        let input_sub = cx.subscribe(&input, |this, _, event: &Changed, cx| {
            let text = event.0.to_string();
            this.palette.update(cx, |palette, cx| palette.set_query(&text, cx));
        });
        let palette_sub = cx.subscribe(&palette, |this, palette, event: &PaletteEvent, cx| match event {
            PaletteEvent::SelectionChanged => {
                this.reveal_pending = true;
                let _ = palette;
                cx.notify();
            }
        });
        let observe_palette = cx.observe(&palette, |this, palette, cx| {
            // A controlled `value`: reset() and query changes from code reach the field.
            let query = palette.read(cx).query.clone();
            this.input.update(cx, |input, cx| input.set_text(query, cx));
            this.sync_loading_announcement(cx);
            cx.notify();
        });
        let observe_app = cx.observe(&app, |_, _, cx| cx.notify());
        let app_sub = cx.subscribe(&app, |this, _, event: &AppEvent, cx| {
            if matches!(event, AppEvent::SettingsChanged { .. }) {
                cx.notify();
            }
            let _ = this;
        });
        let hub = bridge::event_hub(cx);
        let hub_sub = cx.subscribe(&hub, |this, _, event: &BackendEvent, cx| {
            if matches!(event, BackendEvent::CloseRequested | BackendEvent::DismissTransientUi)
                && let Some(menu) = this.result_menu.clone()
            {
                menu.update(cx, |menu, cx| menu.close(false, false, cx));
            }
        });
        let power_menu = cx.new(PowerMenu::new);
        let update_control = cx.new(UpdateControl::new);
        Self {
            palette,
            app,
            input,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            wheel: Rc::default(),
            targets: Rc::default(),
            reorder_drag: None,
            category_drag: None,
            file_drag: None,
            active_drag: None,
            pending_point: None,
            frame_scheduled: false,
            suppress_click: false,
            leaving_preview: None,
            leave_task: None,
            file_drag_task: None,
            result_menu: None,
            power_menu,
            update_control,
            announce_loading: false,
            announce_task: None,
            reveal_pending: false,
            hovered_row: None,
            _subscriptions: vec![input_sub, palette_sub, observe_palette, observe_app, app_sub, hub_sub],
        }
    }

    /// Focuses the search field with the caret at the end.
    pub fn focus_search(&self, window: &mut Window, cx: &mut App) {
        self.input.read(cx).focus(window);
        self.input.update(cx, |input, cx| input.move_to_end(cx));
    }

    /// EmptyState: the loading copy appears only after 200 ms.
    fn sync_loading_announcement(&mut self, cx: &mut Context<Self>) {
        let palette = self.palette.read(cx);
        let loading = if palette.query.is_empty() { !palette.apps_loaded } else { palette.files_busy() };
        if !loading {
            self.announce_task = None;
            self.announce_loading = false;
            return;
        }
        if self.announce_task.is_none() && !self.announce_loading {
            self.announce_task = Some(cx.spawn(async move |this, cx| {
                Timer::after(Duration::from_millis(200)).await;
                let _ = this.update(cx, |this, cx| {
                    this.announce_task = None;
                    this.announce_loading = true;
                    cx.notify();
                });
            }));
        }
    }

    /* ---------------- commands ---------------- */

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (open_settings, query_empty) =
            (self.app.read(cx).open_settings, self.palette.read(cx).query.is_empty());
        if open_settings {
            self.app.update(cx, |app, cx| app.set_open_settings(false, cx));
        } else if !query_empty {
            self.palette.update(cx, |palette, cx| palette.set_query("", cx));
        } else {
            bridge::emit_event(BackendEvent::CloseRequested, cx);
        }
        let _ = window;
    }

    fn on_search_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let composing = self.input.read(cx).is_composing();
        // Escape also cancels a drag in progress (a window keydown listener).
        if event.keystroke.key == "escape" && (self.reorder_drag.is_some() || self.category_drag.is_some() || self.file_drag.is_some()) {
            self.cancel_drags(cx);
        }
        let Some(command) = search_key(event, composing) else { return };
        cx.stop_propagation();
        match command {
            SearchKey::Move(delta) => self.palette.update(cx, |p, cx| p.move_selection(delta, cx)),
            SearchKey::RunSelected => self.palette.update(cx, |p, cx| p.run_selected(cx)),
            SearchKey::OpenSelectedMenu => self.open_selected_menu(window, cx),
            SearchKey::Dismiss => self.dismiss(window, cx),
            SearchKey::Close => bridge::emit_event(BackendEvent::CloseRequested, cx),
            SearchKey::ClearQuery => self.palette.update(cx, |p, cx| p.set_query("", cx)),
        }
    }

    fn open_selected_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (selected, item) = {
            let palette = self.palette.read(cx);
            (palette.selected, palette.flat_items.get(palette.selected).cloned())
        };
        let Some(item) = item.filter(has_result_actions) else { return };
        let bounds = self.targets.borrow().rows.get(&selected).copied();
        let viewport = window.viewport_size();
        let point = match bounds {
            Some(b) => point(b.right() - px(12. * self.zoom(cx)), b.top() + px(12. * self.zoom(cx))),
            None => point(viewport.width / 2., viewport.height / 2.),
        };
        self.open_result_menu(item, selected, point, window, cx);
    }

    fn zoom(&self, cx: &App) -> f32 {
        (self.app.read(cx).settings.view_zoom / 100.0) as f32
    }

    fn open_result_menu(
        &mut self,
        item: PaletteItem,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !has_result_actions(&item) {
            return;
        }
        self.palette.update(cx, |p, cx| p.select(index, cx));
        let zoom = self.zoom(cx);
        let size = window.viewport_size();
        let viewport = (f32::from(size.width) / zoom, f32::from(size.height) / zoom);
        let position = menu_position_for(position, &item, zoom, viewport);
        let menu = cx.new(|cx| ResultContextMenu::new(item, position, window, cx));
        let sub = cx.subscribe_in(&menu, window, |this, menu, event: &MenuEvent, window, cx| match event {
            MenuEvent::Chose(choice) => {
                let item = menu.read(cx).item.clone();
                this.result_menu = None;
                this.choose_menu_action(*choice, item, window, cx);
                this.focus_search_later(window, cx);
            }
            MenuEvent::Closed { restore_focus } => {
                this.result_menu = None;
                if *restore_focus {
                    this.focus_search_later(window, cx);
                }
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.result_menu = Some(menu);
        cx.notify();
    }

    fn focus_search_later(&self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.input.clone();
        window.on_next_frame(move |window, cx| input.read(cx).focus(window));
        cx.notify();
    }

    fn choose_menu_action(&mut self, choice: MenuChoice, item: PaletteItem, _window: &mut Window, cx: &mut Context<Self>) {
        let app = self.app.clone();
        match choice {
            MenuChoice::OpenLocation => {
                if let Some(action) = item.open_location.clone() {
                    run_action(action, cx).detach();
                }
            }
            MenuChoice::RunAsAdmin => self.palette.update(cx, |p, cx| p.run_item_as_admin(item, cx)),
            MenuChoice::ToggleTaskbarPin(was_pinned) => {
                let Some(action) = item.toggle_taskbar_pin.clone() else { return };
                let task = run_action(action, cx);
                cx.spawn(async move |_, cx| {
                    let result = task.await;
                    let _ = app.update(cx, |app, cx| match result {
                        Ok(()) => app.show_toast(
                            if was_pinned { "Unpinned from taskbar" } else { "Pinned to taskbar" },
                            Some(item.title.clone()),
                            ToastKind::Success,
                            cx,
                        ),
                        Err(error) => {
                            let message = error_string(&error);
                            if !was_pinned && message.contains("prism-manual-pin-required") {
                                app.show_toast(
                                    "Windows blocks automatic pinning",
                                    Some(format!(
                                        "Open {}, then right-click its taskbar icon and choose \"Pin to taskbar\".",
                                        item.title
                                    )),
                                    ToastKind::Success,
                                    cx,
                                );
                            } else {
                                app.show_toast("Could not update taskbar pin", Some(message), ToastKind::Success, cx);
                            }
                        }
                    });
                })
                .detach();
            }
            MenuChoice::ShowProperties => {
                let Some(action) = item.show_properties.clone() else { return };
                let task = run_action(action, cx);
                cx.spawn(async move |_, cx| {
                    if let Err(error) = task.await {
                        let _ = app.update(cx, |app, cx| {
                            app.show_toast("Could not open properties", Some(error_string(&error)), ToastKind::Success, cx)
                        });
                    }
                })
                .detach();
            }
        }
    }

    fn toggle_pin(&mut self, item: &PaletteItem, window: &mut Window, cx: &mut Context<Self>) {
        let Some(app_id) = item.app_id.clone() else { return };
        let pinned_apps = self.app.read(cx).settings.pinned_apps.clone();
        let title = item.title.clone();
        if pinned_apps.contains(&app_id) {
            self.app.update(cx, |app, cx| {
                app.update_settings(cx, |s| s.pinned_apps.retain(|candidate| candidate != &app_id));
                app.show_toast("Unpinned", Some(title), ToastKind::Success, cx);
            });
            self.focus_search_later(window, cx);
            return;
        }
        if pinned_apps.len() >= PINNED_APP_LIMIT {
            self.app.update(cx, |app, cx| {
                app.show_toast("Pin limit reached", Some(format!("Unpin an app before adding {title}")), ToastKind::Success, cx)
            });
            return;
        }
        self.app.update(cx, |app, cx| {
            app.update_settings(cx, |s| s.pinned_apps.push(app_id));
            app.show_toast("Pinned", Some(title), ToastKind::Success, cx);
        });
        self.focus_search_later(window, cx);
    }

    fn toggle_app_group(&mut self, group_id: &str, cx: &mut Context<Self>) {
        let group_id = group_id.to_owned();
        self.app.update(cx, |app, cx| {
            app.update_settings(cx, |s| {
                for group in &mut s.app_groups {
                    if group.id == group_id {
                        group.collapsed = !group.collapsed;
                    }
                }
            })
        });
    }

    fn reorder_item(&mut self, source: &PaletteItem, target_item_id: &str, cx: &mut Context<Self>) {
        let settings = self.app.read(cx).settings.clone();
        if let (Some(app_id), Some(target)) = (&source.app_id, target_item_id.strip_prefix("app:")) {
            let pinned = reorder_pinned_apps(&settings.pinned_apps, app_id, target);
            if pinned != settings.pinned_apps {
                self.app.update(cx, |app, cx| app.update_settings(cx, |s| s.pinned_apps = pinned));
            }
            return;
        }
        if let (Some(kind), Some(target)) = (&source.quick_access_kind, target_item_id.strip_prefix("quick:")) {
            let quick = reorder_quick_access(&settings.quick_access, kind, target);
            if quick != settings.quick_access {
                self.app.update(cx, |app, cx| app.update_settings(cx, |s| s.quick_access = quick));
            }
        }
    }

    /* ---------------- pointer drags (A6) ---------------- */

    fn start_reorder_drag(&mut self, item: PaletteItem, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.reorder_drag = Some(ReorderDragState { item, start: position, pos: position, active: false, target_item_id: None });
        self.active_drag = Some(ActiveDrag::Reorder);
        cx.notify();
    }

    fn start_file_drag(&mut self, item: PaletteItem, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.file_drag = Some(FileDragState { item, start: position, active: false });
        self.active_drag = Some(ActiveDrag::File);
        cx.notify();
    }

    fn start_category_drag(&mut self, kind: CategoryKind, id: String, label: String, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.category_drag =
            Some(CategoryDragState { kind, id, label, start: position, pos: position, active: false, target_id: None });
        self.active_drag = Some(ActiveDrag::Category);
        cx.notify();
    }

    /// One apply per frame, like the original's requestAnimationFrame coalescing (A9).
    fn pointer_move(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_drag.is_none() {
            return;
        }
        self.pending_point = Some(position);
        if self.frame_scheduled {
            return;
        }
        self.frame_scheduled = true;
        let weak = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.frame_scheduled = false;
                if let Some(position) = this.pending_point.take() {
                    this.apply_drag(position, window, cx);
                }
            });
        });
        cx.notify();
    }

    fn crossed(start: Point<Pixels>, position: Point<Pixels>, zoom: f32) -> bool {
        let dx = f32::from(position.x - start.x) / zoom;
        let dy = f32::from(position.y - start.y) / zoom;
        (dx * dx + dy * dy).sqrt() >= DRAG_THRESHOLD
    }

    fn target_at<'a>(targets: &'a [(String, Bounds<Pixels>)], viewport: Option<Bounds<Pixels>>, position: Point<Pixels>) -> Option<&'a str> {
        if viewport.is_some_and(|v| !v.contains(&position)) {
            return None;
        }
        targets.iter().find(|(_, bounds)| bounds.contains(&position)).map(|(id, _)| id.as_str())
    }

    fn apply_drag(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let zoom = self.zoom(cx);
        match self.active_drag {
            Some(ActiveDrag::Reorder) => {
                let targets = self.targets.borrow();
                let Some(drag) = self.reorder_drag.as_mut() else { return };
                let active = drag.active || Self::crossed(drag.start, position, zoom);
                let hovered = if active { Self::target_at(&targets.reorder, targets.viewport, position) } else { None };
                let source = reorder_item_id(&drag.item);
                let same_group = hovered.map(|h| h.split(':').next()) == source.as_deref().map(|s| s.split(':').next());
                drag.target_item_id =
                    hovered.filter(|h| Some(*h) != source.as_deref() && same_group).map(str::to_owned);
                drag.pos = position;
                drag.active = active;
            }
            Some(ActiveDrag::Category) => {
                let targets = self.targets.borrow();
                let Some(drag) = self.category_drag.as_mut() else { return };
                let active = drag.active || Self::crossed(drag.start, position, zoom);
                let list = if drag.kind == CategoryKind::Section { &targets.sections } else { &targets.groups };
                let hovered = if active { Self::target_at(list, targets.viewport, position) } else { None };
                drag.target_id = hovered.filter(|h| *h != drag.id).map(str::to_owned);
                drag.pos = position;
                drag.active = active;
            }
            Some(ActiveDrag::File) => {
                let Some(drag) = self.file_drag.as_mut() else { return };
                if !drag.active && Self::crossed(drag.start, position, zoom) {
                    drag.active = true;
                    self.start_native_file_drag(window, cx);
                }
            }
            None => {}
        }
        cx.notify();
    }

    /// The OLE drag runs from a foreground task outside every GPUI callback (A7).
    fn start_native_file_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.file_drag.as_ref().map(|d| d.item.clone()) else { return };
        let paths: Vec<String> = item.shell_path.clone().into_iter().collect();
        self.file_drag_task = Some(cx.spawn_in(window, async move |this, cx| {
            let drag = cx.update(|_, cx| bridge::shell(cx).start_file_drag(paths, cx));
            if let Ok(task) = drag {
                let _ = task.await;
            }
            let _ = this.update(cx, |this, cx| {
                this.file_drag = None;
                if this.active_drag == Some(ActiveDrag::File) {
                    this.active_drag = None;
                }
                cx.notify();
            });
        }));
    }

    /// Ends the captured pointer. Returns true when a drag happened (the release
    /// is then not an activation).
    fn pointer_up(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(position) = self.pending_point.take() {
            self.apply_drag(position, window, cx);
        }
        let dragged = match self.active_drag.take() {
            Some(ActiveDrag::Reorder) => {
                let Some(drag) = self.reorder_drag.take() else { return false };
                if drag.active
                    && let Some(target) = drag.target_item_id.clone()
                {
                    self.reorder_item(&drag.item, &target, cx);
                }
                if drag.active {
                    self.animate_preview_out(drag.clone(), cx);
                }
                drag.active
            }
            Some(ActiveDrag::Category) => {
                let Some(drag) = self.category_drag.take() else { return false };
                if drag.active
                    && let Some(target) = drag.target_id.clone()
                {
                    let settings = self.app.read(cx).settings.clone();
                    match drag.kind {
                        CategoryKind::Section => {
                            let order = reorder_sections(&settings.section_order, &drag.id, &target);
                            self.app.update(cx, |app, cx| app.update_settings(cx, |s| s.section_order = order));
                        }
                        CategoryKind::Group => {
                            let groups = reorder_app_groups(&settings.app_groups, &drag.id, &target);
                            self.app.update(cx, |app, cx| app.update_settings(cx, |s| s.app_groups = groups));
                        }
                    }
                }
                drag.active
            }
            Some(ActiveDrag::File) => {
                let active = self.file_drag.as_ref().is_some_and(|d| d.active);
                if !active {
                    self.file_drag = None;
                }
                active
            }
            None => false,
        };
        cx.notify();
        dragged
    }

    fn animate_preview_out(&mut self, drag: ReorderDragState, cx: &mut Context<Self>) {
        self.leaving_preview = Some((drag, Instant::now()));
        self.leave_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(PREVIEW_LEAVE_MS)).await;
            let _ = this.update(cx, |this, cx| {
                this.leaving_preview = None;
                cx.notify();
            });
        }));
    }

    fn cancel_drags(&mut self, cx: &mut Context<Self>) {
        self.pending_point = None;
        self.reorder_drag = None;
        self.category_drag = None;
        self.file_drag = None;
        self.active_drag = None;
        cx.notify();
    }

    /* ---------------- reveal ---------------- */

    /// `scrollIntoView({block: "nearest"})` for the selected row (A10).
    fn reveal_selected(&mut self, cx: &mut Context<Self>) {
        let selected = self.palette.read(cx).selected;
        let targets = self.targets.borrow();
        let (Some(row), Some(viewport)) = (targets.rows.get(&selected).copied(), targets.viewport) else { return };
        let mut offset = self.scroll.offset();
        if row.top() < viewport.top() {
            offset.y += viewport.top() - row.top();
        } else if row.bottom() > viewport.bottom() {
            offset.y -= row.bottom() - viewport.bottom();
        } else {
            return;
        }
        drop(targets);
        self.scroll.set_offset(offset);
        cx.notify();
    }
}

impl Focusable for Palette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/* ---------------- rendering ---------------- */

struct RowFlags {
    index: usize,
    selected: bool,
    hovered: bool,
    pinned: bool,
    reorderable: bool,
    removable: bool,
    dragged: bool,
    drop_target: bool,
}

impl Palette {
    fn render_row(&self, item: &PaletteItem, flags: RowFlags, t: &Theme, zoom: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let RowFlags { index, selected, hovered, pinned, reorderable, removable, dragged, drop_target } = flags;
        let item_reorder_id = reorder_item_id(item);
        let can_reorder = reorderable && item_reorder_id.is_some();
        let can_drag_file = !can_reorder && item.shell_path.is_some() && item.drag_file.is_some();
        let targets = self.targets.clone();
        let reorder_target = can_reorder.then(|| item_reorder_id.clone()).flatten();

        // `.row` background stack.
        let mut row = div()
            .id(("row", index))
            .group("row")
            .relative()
            .flex()
            .items_center()
            .gap(r(12.))
            .w_full()
            .py(r(7.))
            .px(r(10.))
            .rounded(r(14.))
            .map(|d| {
                if drop_target {
                    d.bg(hsla(t.drop_target_fill()))
                        .border_1()
                        .border_color(hsla(fade(t.accent, 52.0)))
                        .shadow(zoomed(vec![shadow(0., 3., 10., 0., fade(t.accent, 9.0))], zoom))
                } else if selected {
                    d.bg(gpui::linear_gradient(
                        90.,
                        gpui::linear_color_stop(hsla(crate::theme::mix(t.row_selected_wash(), 100.0, t.surface_active)), 0.0),
                        gpui::linear_color_stop(hsla(t.surface_active), 0.58),
                    ))
                    .border_1()
                    .border_color(hsla(fade(t.accent, 13.0)))
                } else if hovered {
                    d.bg(hsla(t.surface_hover))
                } else {
                    d
                }
            })
            .when(dragged, |d| d.opacity(0.42))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                // A drag holds the pointer (pointer capture): other rows get no hover.
                if this.active_drag.is_some() {
                    return;
                }
                if *hovered {
                    this.hovered_row = Some(index);
                    // `onMouseEnter`: hovering selects.
                    this.palette.update(cx, |p, cx| p.select(index, cx));
                } else if this.hovered_row == Some(index) {
                    this.hovered_row = None;
                }
                cx.notify();
            }));
        if selected || drop_target {
            // The selected/drop ring is an inset shadow in CSS; compensate the 1px border.
            row = row.py(r(6.)).px(r(9.));
        }
        let run_item = item.clone();
        row = row
            .on_click(cx.listener(move |this, _, _, cx| {
                if std::mem::take(&mut this.suppress_click) {
                    return;
                }
                let item = run_item.clone();
                this.palette.update(cx, |p, cx| p.run_item(item, cx));
            }))
            .on_mouse_down(MouseButton::Right, {
                let item = item.clone();
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if has_result_actions(&item) {
                        this.open_result_menu(item.clone(), index, event.position, window, cx);
                    }
                })
            });

        // Record hit targets for drags and reveal (A6).
        row = row.child(
            canvas(
                move |bounds, _, _| {
                    let mut targets = targets.borrow_mut();
                    targets.rows.insert(index, bounds);
                    if let Some(id) = &reorder_target {
                        targets.reorder.push((id.clone(), bounds));
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );

        // Icon cell: a drag handle for reorderable and draggable rows.
        let icon_cell = {
            let mut cell = div().flex_none().child(row_icon(&item.icon, 40., t, zoom));
            if can_reorder || can_drag_file {
                let drag_item = item.clone();
                cell = cell.cursor_grab().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        if can_reorder {
                            this.start_reorder_drag(drag_item.clone(), event.position, cx);
                        } else {
                            this.start_file_drag(drag_item.clone(), event.position, cx);
                        }
                    }),
                );
            }
            cell
        };

        let title_color = if selected { hsla(t.fg) } else { crate::gfx::alpha(t.fg, 0.9) };
        let text_cell = div()
            .flex_1()
            .min_w_0()
            .child(
                truncate(item.title.clone())
                    .text_size(r(13.5))
                    .line_height(relative(1.25))
                    .font_weight(weight(500.))
                    .text_color(title_color),
            )
            .when_some(item.subtitle.clone(), |d, subtitle| {
                d.child(
                    truncate(subtitle)
                        .mt(r(3.))
                        .text_size(r(11.5))
                        .line_height(relative(1.25))
                        .text_color(hsla(t.fg_tertiary)),
                )
            });

        let trailing = self.render_trailing(item, index, selected, hovered, pinned, can_reorder, can_drag_file, removable, dragged, t, window, cx);
        row.child(icon_cell).child(text_cell).child(trailing).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_trailing(
        &self,
        item: &PaletteItem,
        index: usize,
        selected: bool,
        row_hovered: bool,
        pinned: bool,
        can_reorder: bool,
        can_drag_file: bool,
        removable: bool,
        dragged: bool,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut trailing = div().relative().flex().flex_none().items_center().gap(r(2.));
        let square = |id: (&'static str, usize)| {
            div().id(id).flex().flex_none().items_center().justify_center().size(r(32.)).rounded(r(8.))
        };
        if can_reorder {
            let drag_item = item.clone();
            let grabbing = dragged;
            trailing = trailing.child(
                square(("reorder-grip", index))
                    .group("grip")
                    .map(|d| if grabbing { d.bg(hsla(t.accent_soft)).cursor_grabbing() } else { d.cursor_grab().hover(|s| s.bg(hsla(t.surface_hover))) })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.start_reorder_drag(drag_item.clone(), event.position, cx);
                        }),
                    )
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        icon("grip-vertical", 16., if grabbing { t.accent } else { t.fg_quiet })
                            .group_hover("grip", |s| s.text_color(hsla(t.fg))),
                    ),
            );
        }
        let remove_button = |this: &Self, cx: &mut Context<Self>| {
            let id = item.id.clone();
            let app = this.app.clone();
            square(("remove-recent", index))
                .group("remove")
                .opacity(if row_hovered { 1.0 } else { 0.6 })
                .hover(|s| s.bg(hsla(t.danger_soft)))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.stop_propagation();
                    let id = id.clone();
                    app.update(cx, |app, cx| app.remove_history(&id, cx));
                }))
                .child(icon("x", 16., t.fg_tertiary).group_hover("remove", |s| s.text_color(hsla(t.danger))))
        };
        if item.app_id.is_some() {
            // Pin toggle: visible on row hover; the two icons cross-fade (`.icon-swap`).
            let swap = window.use_keyed_state(gpui::ElementId::Name(format!("pin-swap-{}", item.id).into()), cx, |_, _| {
                Tween::at(if pinned { 1.0 } else { 0.0 })
            });
            let target = if pinned { 1.0 } else { 0.0 };
            if swap.read(cx).target() != target {
                swap.update(cx, |s, _| s.set(target, motion_ms(300), motion::icon_swap));
            }
            let p = swap.read(cx).animate(window);
            let pin_item = item.clone();
            let base_color = if pinned { t.accent } else { t.fg_tertiary };
            let scaled = |name: &str, visible: f32| {
                let scale = 0.25 + 0.75 * visible;
                icon(name, 16., base_color)
                    .with_transformation(gpui::Transformation::scale(gpui::size(scale, scale)))
                    .when(!pinned, |s| s.group_hover("pin", |s| s.text_color(hsla(t.fg))))
            };
            trailing = trailing.child(
                square(("pin-toggle", index))
                    .group("pin")
                    .opacity(if row_hovered { 1.0 } else { 0.0 })
                    .map(|d| if pinned { d.bg(hsla(t.accent_soft)) } else { d.hover(|s| s.bg(hsla(t.surface_hover))) })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.toggle_pin(&pin_item, window, cx);
                    }))
                    .child(
                        div()
                            .relative()
                            .size(r(16.))
                            .child(div().absolute().inset_0().opacity(p).child(scaled("pin-off", p)))
                            .child(div().absolute().inset_0().opacity(1.0 - p).child(scaled("pin", 1.0 - p))),
                    ),
            );
            if removable {
                trailing = trailing.child(remove_button(self, cx));
            }
        } else if removable {
            trailing = trailing.child(remove_button(self, cx));
        } else if is_clipboard_kind(&item.id) && selected {
            trailing = trailing.child(
                div()
                    .text_size(r(12.))
                    .font_weight(weight(600.))
                    .line_height(relative(1.45))
                    .text_color(hsla(t.accent))
                    .child("Enter to copy"),
            );
        } else if can_drag_file {
            let drag_item = item.clone();
            trailing = trailing.child(
                div()
                    .flex()
                    .items_center()
                    .gap(r(6.))
                    .when(selected, |d| {
                        d.child(
                            div()
                                .text_size(r(11.))
                                .font_weight(weight(500.))
                                .line_height(relative(1.45))
                                .text_color(hsla(t.fg_tertiary))
                                .child("Drag to copy"),
                        )
                    })
                    .child(
                        square(("file-grip", index))
                            .group("file-grip")
                            .cursor_grab()
                            .hover(|s| s.bg(hsla(t.surface_hover)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.start_file_drag(drag_item.clone(), event.position, cx);
                                }),
                            )
                            .child(icon("grip-vertical", 16., t.fg_quiet).group_hover("file-grip", |s| s.text_color(hsla(t.accent)))),
                    ),
            );
        } else if selected && item.id.starts_with("file::") {
            trailing = trailing.child(
                div()
                    .text_size(r(11.5))
                    .font_weight(weight(500.))
                    .line_height(relative(1.45))
                    .text_color(hsla(t.fg_tertiary))
                    .child("Enter to open"),
            );
        }
        trailing.into_any_element()
    }

    /// A category drag handle (sections and app collections; idle view only).
    fn category_grip(
        &self,
        kind: CategoryKind,
        id: &str,
        label: &str,
        margin_right: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let drag_id = id.to_owned();
        let drag_label = label.to_owned();
        let prefix = if kind == CategoryKind::Section { "section-grip" } else { "group-grip" };
        div()
            .id(gpui::ElementId::Name(format!("{prefix}-{id}").into()))
            .group("category-grip")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(r(28.))
            .rounded(r(7.))
            .when(margin_right, |d| d.mr(r(14.)))
            .cursor_grab()
            .hover(|s| s.bg(hsla(t.surface_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if !js::trim(&this.palette.read(cx).query).is_empty() {
                        return;
                    }
                    this.start_category_drag(kind, drag_id.clone(), drag_label.clone(), event.position, cx);
                }),
            )
            .child(icon("grip-vertical", 14., t.fg_quiet).group_hover("category-grip", |s| s.text_color(hsla(t.fg))))
            .into_any_element()
    }

    /// A collapsible header (Quick Access and app collections) with a rotating chevron.
    fn collapsible_header(
        &self,
        id: &str,
        label: &str,
        collapsed: bool,
        label_color: Rgba,
        hover_color: Rgba,
        window: &mut Window,
        cx: &mut App,
        on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let chevron = window.use_keyed_state(gpui::ElementId::Name(format!("chevron-{id}").into()), cx, |_, _| {
            Tween::at(if collapsed { 1.0 } else { 0.0 })
        });
        let target = if collapsed { 1.0 } else { 0.0 };
        if chevron.read(cx).target() != target {
            chevron.update(cx, |c, _| c.set(target, motion_ms(150), motion::ease));
        }
        let turn = chevron.read(cx).animate(window);
        let group = format!("collapse-{id}");
        div()
            .id(gpui::ElementId::Name(format!("collapse-{id}").into()))
            .group(group.clone())
            .flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(r(4.))
            .rounded(r(6.))
            .text_size(r(11.))
            .font_weight(weight(600.))
            .line_height(relative(1.45))
            .text_color(hsla(label_color))
            .hover(|s| s.text_color(hsla(hover_color)))
            .on_click(move |_, window, cx| on_toggle(window, cx))
            .child(
                rotated(icon("chevron-down", 14., label_color), -std::f32::consts::FRAC_PI_2 * turn)
                    .group_hover(group, |s| s.text_color(hsla(hover_color))),
            )
            .child(truncate(label.to_uppercase()))
            .into_any_element()
    }

    fn render_results(&mut self, t: &Theme, zoom: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette.read(cx);
        let sections = palette.sections.clone();
        let selected = palette.selected;
        let query = palette.query.clone();
        let file_error = palette.file_error.clone();
        let sections_empty = sections.is_empty();
        let pinned_apps = self.app.read(cx).settings.pinned_apps.clone();
        let quick_collapsed_setting = self.app.read(cx).settings.quick_access_collapsed;
        let section_reorderable = js::trim(&query).is_empty();
        let dragged_id = match (&self.reorder_drag, &self.file_drag) {
            (Some(d), _) if d.active => reorder_item_id(&d.item),
            (_, Some(d)) if d.active => Some(d.item.id.clone()),
            _ => None,
        };
        let drop_target = self.reorder_drag.as_ref().filter(|d| d.active).and_then(|d| d.target_item_id.clone());
        let category = self.category_drag.clone();
        let hovered_row = self.hovered_row;

        {
            let mut targets = self.targets.borrow_mut();
            targets.rows.clear();
            targets.reorder.clear();
            targets.sections.clear();
            targets.groups.clear();
        }
        let targets = self.targets.clone();
        // The scroll container carries Chromium's wheel line (100/3 px); the
        // content resets the line height (gfx::CHROMIUM_LINE_PX).
        let viewport = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                canvas(move |bounds, _, _| targets.borrow_mut().viewport = Some(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            );
        let scroll = div()
            .id("prism-results")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .line_height(px(crate::gfx::CHROMIUM_LINE_PX));
        let mut list = div().flex().flex_col().line_height(relative(1.45)).px(r(10.)).pb(r(8.));

        if let Some(error) = file_error.clone().filter(|_| !sections_empty) {
            list = list.child(self.file_error_notice(&error, t, cx));
        }
        if sections_empty {
            list = list.size_full().child(self.empty_state(t, cx));
            return viewport
                .child(scroll.child(list))
                .child(crate::gfx::wheel_scroll_layer(self.scroll.clone(), self.wheel.clone()))
                .into_any_element();
        }

        let mut flat = 0usize;
        for section in sections {
            let section_drop = category
                .as_ref()
                .is_some_and(|c| c.active && c.kind == CategoryKind::Section && c.target_id.as_deref() == Some(&section.id));
            let section_dragging = category
                .as_ref()
                .is_some_and(|c| c.active && c.kind == CategoryKind::Section && c.id == section.id);
            let targets = self.targets.clone();
            let section_target = section_reorderable.then(|| section.id.clone());
            let mut block = div()
                .id(gpui::ElementId::Name(format!("section-{}", section.id).into()))
                .relative()
                .flex()
                .flex_col()
                .when(section_drop, |d| {
                    d.rounded(r(14.)).bg(hsla(fade(t.accent, 8.0))).border_1().border_color(hsla(fade(t.accent, 42.0)))
                })
                .when(section_dragging, |d| d.opacity(0.45))
                .child(
                    canvas(
                        move |bounds, _, _| {
                            if let Some(id) = &section_target {
                                targets.borrow_mut().sections.push((id.clone(), bounds));
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                );

            // Header.
            if section.collapsible == Some(true) {
                let app = self.app.clone();
                let header = self.collapsible_header(
                    &section.id,
                    &section.label,
                    section.collapsed == Some(true),
                    t.fg_quiet,
                    t.fg_secondary,
                    window,
                    cx,
                    move |_, cx| {
                        app.update(cx, |app, cx| {
                            app.update_settings(cx, |s| s.quick_access_collapsed = !quick_collapsed_setting)
                        })
                    },
                );
                block = block.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(r(4.))
                        .px(r(14.))
                        .pb(r(6.))
                        .pt(r(16.))
                        .child(div().flex_1().min_w_0().child(header))
                        .when(section_reorderable, |d| {
                            d.child(self.category_grip(CategoryKind::Section, &section.id, &section.label, false, t, cx))
                        }),
                );
            } else {
                block = block.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(r(4.))
                        .child(div().flex_1().min_w_0().child(section_label(&section.label, t)))
                        .when(section_reorderable, |d| {
                            d.child(self.category_grip(CategoryKind::Section, &section.id, &section.label, true, t, cx))
                        }),
                );
            }

            // App collections inside the Apps section.
            let groups = section.groups.clone().unwrap_or_default();
            let groups_present = !groups.is_empty();
            for (group_index, group) in groups.into_iter().enumerate() {
                let group_drop = category.as_ref().is_some_and(|c| {
                    c.active && c.kind == CategoryKind::Group && c.target_id.as_deref() == Some(&group.group_id)
                });
                let group_dragging = category
                    .as_ref()
                    .is_some_and(|c| c.active && c.kind == CategoryKind::Group && c.id == group.group_id);
                let targets = self.targets.clone();
                let group_target = section_reorderable.then(|| group.group_id.clone());
                let group_id = group.group_id.clone();
                let this = cx.entity().downgrade();
                let header = self.collapsible_header(
                    &group.id,
                    &group.label,
                    group.collapsed,
                    t.fg_secondary,
                    t.fg,
                    window,
                    cx,
                    move |_, cx| {
                        let _ = this.update(cx, |this, cx| this.toggle_app_group(&group_id, cx));
                    },
                );
                let mut group_block = div()
                    .relative()
                    .flex()
                    .flex_col()
                    .when(group_index > 0, |d| d.mt(r(4.)).border_t_1().border_color(hsla(t.line)).pt(r(4.)))
                    .when(group_drop, |d| {
                        d.rounded(r(14.)).bg(hsla(fade(t.accent, 8.0))).border_1().border_color(hsla(fade(t.accent, 42.0)))
                    })
                    .when(group_dragging, |d| d.opacity(0.45))
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                if let Some(id) = &group_target {
                                    targets.borrow_mut().groups.push((id.clone(), bounds));
                                }
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
                            .gap(r(4.))
                            .px(r(14.))
                            .pb(r(6.))
                            .pt(r(8.))
                            .child(div().flex_1().min_w_0().child(header))
                            .when(section_reorderable, |d| {
                                d.child(self.category_grip(CategoryKind::Group, &group.group_id, &group.label, false, t, cx))
                            }),
                    );
                let mut rows = div().flex().flex_col().gap(r(2.));
                for item in &group.items {
                    let index = flat;
                    flat += 1;
                    let flags = RowFlags {
                        index,
                        selected: selected == index,
                        hovered: hovered_row == Some(index),
                        pinned: item.app_id.as_ref().is_some_and(|id| pinned_apps.contains(id)),
                        reorderable: false,
                        removable: false,
                        dragged: dragged_id.as_deref() == Some(item.id.as_str()),
                        drop_target: false,
                    };
                    rows = rows.child(self.render_row(item, flags, t, zoom, window, cx));
                }
                group_block = group_block.child(rows);
                block = block.child(group_block);
            }
            if groups_present && !section.items.is_empty() {
                block = block.child(
                    div()
                        .mt(r(8.))
                        .flex()
                        .items_center()
                        .gap(r(8.))
                        .px(r(14.))
                        .pb(r(4.))
                        .pt(r(8.))
                        .text_size(r(10.))
                        .font_weight(weight(600.))
                        .line_height(relative(1.45))
                        .text_color(hsla(t.fg_quiet))
                        .child(div().h(px(1.)).flex_1().bg(hsla(t.line)))
                        .child("OTHER APPS")
                        .child(div().h(px(1.)).flex_1().bg(hsla(t.line))),
                );
            }
            let reorderable = section.id == "pinned" || section.id == "quick";
            let removable = section.id == "recent";
            let mut rows = div().flex().flex_col().gap(r(2.));
            for item in &section.items {
                let index = flat;
                flat += 1;
                let item_reorder_id = reorder_item_id(item);
                let can_reorder = reorderable && item_reorder_id.is_some();
                let can_drag_file = !can_reorder && item.shell_path.is_some() && item.drag_file.is_some();
                let flags = RowFlags {
                    index,
                    selected: selected == index,
                    hovered: hovered_row == Some(index),
                    pinned: item.app_id.as_ref().is_some_and(|id| pinned_apps.contains(id)),
                    reorderable,
                    removable,
                    dragged: (can_reorder && dragged_id.is_some() && dragged_id == item_reorder_id)
                        || (can_drag_file && dragged_id.as_deref() == Some(item.id.as_str())),
                    drop_target: can_reorder && drop_target.is_some() && drop_target == item_reorder_id,
                };
                rows = rows.child(self.render_row(item, flags, t, zoom, window, cx));
            }
            block = block.child(rows);
            list = list.child(block);
        }
        viewport
            .child(scroll.child(list))
            .child(crate::gfx::wheel_scroll_layer(self.scroll.clone(), self.wheel.clone()))
            .into_any_element()
    }

    fn file_error_notice(&self, error: &FileSearchError, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let index_failed = error.kind == "indexQuery";
        let palette = self.palette.clone();
        div()
            .mx(r(10.))
            .mt(r(8.))
            .flex()
            .items_center()
            .justify_between()
            .gap(r(12.))
            .min_h(r(44.))
            .rounded(r(10.))
            .bg(hsla(t.danger_soft))
            .px(r(12.))
            .text_color(hsla(t.danger))
            .child(
                truncate(if index_failed { "File index search failed." } else { "Prism cannot read this folder." })
                    .text_size(r(11.5))
                    .line_height(relative(1.45)),
            )
            .child(
                div()
                    .id("file-error-action")
                    .flex()
                    .flex_none()
                    .items_center()
                    .min_h(r(44.))
                    .px(r(8.))
                    .text_size(r(11.5))
                    .font_weight(weight(600.))
                    .line_height(relative(1.45))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        palette.update(cx, |p, cx| if index_failed { p.rebuild_index(cx) } else { p.retry_file_search(cx) })
                    }))
                    .child(if index_failed { "Rebuild index" } else { "Retry" }),
            )
            .into_any_element()
    }

    fn empty_state(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette.read(cx);
        let query = palette.query.clone();
        let loading = if query.is_empty() { !palette.apps_loaded } else { palette.files_busy() };
        let show_loading = loading && self.announce_loading;
        let path_browsing = palette.path_browsing();
        // `query ? fileError : appError ? {kind: "apps"} : null`
        let error_kind: Option<String> = if !query.is_empty() {
            palette.file_error.as_ref().map(|e| e.kind.clone())
        } else if palette.apps_error {
            Some("apps".into())
        } else {
            None
        };
        let title_style = |d: gpui::Div| {
            d.text_size(r(13.5)).font_weight(weight(500.)).line_height(relative(1.45)).text_color(hsla(t.fg_secondary))
        };
        let body_style = |d: gpui::Div| {
            d.max_w(r(260.)).text_size(r(12.)).line_height(relative(1.625)).text_color(hsla(t.fg_tertiary)).text_center()
        };
        let mut cell = div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap(r(10.))
            .text_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(r(56.))
                    .rounded(r(20.))
                    .bg(hsla(t.surface))
                    .child(crate::gfx::icon_stroke("search", 1.75, 24., t.fg_quiet)),
            );
        if let Some(kind) = error_kind {
            let (title, body) = match kind.as_str() {
                "directoryAccess" => (
                    "This folder cannot be read",
                    "Check that the folder still exists and that Prism has permission to open it.",
                ),
                "indexQuery" => (
                    "File search is unavailable",
                    "Prism couldn't query the local file catalog. Rebuild the index, then search again.",
                ),
                _ => ("Apps couldn't be loaded", "The app index failed to scan. Retry now, or keep searching local files."),
            };
            let palette_entity = self.palette.clone();
            let action_kind = kind.clone();
            cell = cell
                .child(title_style(div()).child(crate::components::pretty_text::pretty(title)))
                .child(body_style(div()).child(crate::components::pretty_text::pretty(body)))
                .child(
                    div()
                        .id("empty-state-action")
                        .group("empty-action")
                        .mt(r(4.))
                        .flex()
                        .items_center()
                        .gap(r(6.))
                        .min_h(r(44.))
                        .rounded(r(10.))
                        .bg(hsla(t.surface))
                        .px(r(12.))
                        .text_size(r(12.))
                        .font_weight(weight(500.))
                        .line_height(relative(1.45))
                        .text_color(hsla(t.fg_secondary))
                        .hover(|s| s.bg(hsla(t.surface_hover)).text_color(hsla(t.fg)))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            palette_entity.update(cx, |p, cx| match action_kind.as_str() {
                                "directoryAccess" => p.retry_file_search(cx),
                                "indexQuery" => p.rebuild_index(cx),
                                _ => p.refresh_apps(cx),
                            })
                        }))
                        .child(
                            icon(if kind == "indexQuery" { "folder-sync" } else { "refresh-cw" }, 14., t.fg_secondary)
                                .group_hover("empty-action", |s| s.text_color(hsla(t.fg))),
                        )
                        .child(if kind == "indexQuery" { "Rebuild index" } else { "Retry" }),
                );
        } else {
            let (title, body) = if show_loading {
                if query.is_empty() {
                    ("Preparing Prism\u{2026}", "Loading quick access and installed applications")
                } else {
                    ("Searching local files\u{2026}", "The first file index finishes in the background")
                }
            } else if path_browsing {
                ("No matching items", "This folder is empty, or no item matches the partial path")
            } else {
                ("Start typing to search", "Find local files, folders, apps, or calculate 12 \u{00D7} 8")
            };
            cell = cell
                .child(title_style(div()).child(crate::components::pretty_text::pretty(title)))
                .child(body_style(div()).child(crate::components::pretty_text::pretty(body)));
        }
        div().flex_1().size_full().child(cell).into_any_element()
    }

    fn render_previews(&self, t: &Theme, zoom: f32, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let viewport = window.viewport_size();
        let clamp = |value: f32, min: f32, max: f32| value.max(min).min(max);
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        // ReorderDragPreview: left of the pointer, clamped 8px inside the window.
        let reorder = match (&self.reorder_drag, &self.leaving_preview) {
            (Some(drag), _) if drag.active => Some((drag.clone(), None)),
            (_, Some((drag, since))) => Some((drag.clone(), Some(*since))),
            _ => None,
        };
        if let Some((drag, leaving)) = reorder {
            let (w, h) = (232.0 * zoom, 54.0 * zoom);
            let x = clamp(f32::from(drag.pos.x) - w - 14.0 * zoom, 8.0 * zoom, vw - w - 8.0 * zoom);
            let y = clamp(f32::from(drag.pos.y) - h / 2.0, 8.0 * zoom, vh - h - 8.0 * zoom);
            // reorder-drag-pickup (120 ms) and reorder-drag-out (110 ms): opacity .72 <-> 1.
            let opacity = match leaving {
                Some(since) => {
                    window.request_animation_frame();
                    1.0 - 0.28 * motion::ease_in((since.elapsed().as_millis() as f32 / 110.0).min(1.0))
                }
                None => 1.0,
            };
            let item = drag.item.clone();
            return Some(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(r(232.))
                    .flex()
                    .items_center()
                    .gap(r(10.))
                    .py(r(7.))
                    .px(r(9.))
                    .rounded(r(14.))
                    .bg(hsla(t.bg_raised))
                    .shadow(zoomed(vec![shadow(0., 8., 22., 0., crate::theme::black(0.34))], zoom))
                    .opacity(opacity)
                    .child(crate::components::ui::inset_top_highlight(t.preview_hi(), 14.))
                    .child(row_icon(&item.icon, 40., t, zoom))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                truncate(item.title.clone())
                                    .text_size(r(13.5))
                                    .line_height(relative(1.25))
                                    .font_weight(weight(600.))
                                    .text_color(hsla(t.fg)),
                            )
                            .child(
                                truncate(item.subtitle.clone().unwrap_or_else(|| "Application".into()))
                                    .mt(r(3.))
                                    .text_size(r(11.5))
                                    .line_height(relative(1.25))
                                    .text_color(hsla(t.fg_tertiary)),
                            ),
                    )
                    .child(icon("grip-vertical", 16., t.accent))
                    .into_any_element(),
            );
        }
        let drag = self.category_drag.as_ref().filter(|d| d.active)?;
        let (w, h) = (208.0 * zoom, 48.0 * zoom);
        let x = clamp(f32::from(drag.pos.x) + 14.0 * zoom, 8.0 * zoom, vw - w - 8.0 * zoom);
        let y = clamp(f32::from(drag.pos.y) - h / 2.0, 8.0 * zoom, vh - h - 8.0 * zoom);
        let _ = cx;
        Some(
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(r(208.))
                .min_h(r(48.))
                .flex()
                .items_center()
                .gap(r(10.))
                .py(r(7.))
                .px(r(9.))
                .rounded(r(10.))
                .bg(hsla(t.bg_raised))
                .border_1()
                .border_color(hsla(fade(t.accent, 22.0)))
                .shadow(zoomed(vec![shadow(0., 8., 22., 0., crate::theme::black(0.34))], zoom))
                .child(crate::components::ui::inset_top_highlight(t.preview_hi(), 10.))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(r(32.))
                        .rounded(r(8.))
                        .bg(hsla(t.accent_soft))
                        .child(icon("grip-vertical", 16., t.accent)),
                )
                .child(
                    div()
                        .min_w_0()
                        .child(
                            truncate(drag.label.clone())
                                .text_size(r(12.))
                                .font_weight(weight(600.))
                                .line_height(relative(1.45))
                                .text_color(hsla(t.fg)),
                        )
                        .child(
                            div()
                                .mt(r(2.))
                                .text_size(r(10.5))
                                .line_height(relative(1.45))
                                .text_color(hsla(t.fg_tertiary))
                                .child(if drag.kind == CategoryKind::Section { "Section" } else { "App collection" }),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_footer(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette.read(cx);
        let apps_loaded = palette.apps_loaded;
        let apps_error = palette.apps_error;
        let file_indexing = palette.file_indexing;
        let query_empty = palette.query.is_empty();
        let hint = |label: &str| {
            div().px(r(4.)).child(label.to_owned())
        };
        let dot = || div().text_color(hsla(t.divider)).child("\u{00B7}");
        let spinner = |label: &'static str| {
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(r(6.))
                .text_size(r(11.))
                .line_height(relative(1.45))
                .text_color(hsla(t.fg_quiet))
                .child(spinning_icon("refresh-cw", 12., t.fg_quiet))
                .child(truncate(label))
        };
        let palette_entity = self.palette.clone();
        let palette_entity2 = self.palette.clone();
        div()
            .relative()
            .flex()
            .flex_none()
            .h(r(48.))
            .items_center()
            .justify_between()
            .gap_x(r(12.))
            .px(r(20.))
            .border_t_1()
            .border_color(hsla(t.footer_line()))
            .bg(gpui::linear_gradient(
                180.,
                gpui::linear_color_stop(hsla(crate::theme::TRANSPARENT), 0.0),
                gpui::linear_color_stop(hsla(crate::theme::black(0.06)), 1.0),
            ))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(r(6.))
                    .text_size(r(11.))
                    .line_height(relative(1.45))
                    .text_color(hsla(t.fg_quiet))
                    .child(kbd("\u{2191}", t))
                    .child(kbd("\u{2193}", t))
                    .child(hint("navigate"))
                    .child(dot())
                    .child(kbd("\u{21B5}", t))
                    .child(hint("open"))
                    .child(dot())
                    .child(kbd("esc", t))
                    .child(hint("dismiss")),
            )
            .child(
                // `ms-auto` is redundant beside `justify-between`, and taffy
                // ends an auto-margin item one gap short of the edge.
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .justify_end()
                    .gap(r(6.))
                    .child(self.update_control.clone())
                    .when(!apps_loaded && query_empty, |d| d.child(spinner("indexing apps")))
                    .when(file_indexing && apps_loaded, |d| d.child(spinner("indexing files")))
                    .when(apps_error && query_empty, |d| {
                        d.child(
                            div()
                                .text_size(r(11.))
                                .line_height(relative(1.45))
                                .text_color(hsla(t.danger))
                                .child("apps failed to load"),
                        )
                    })
                    .child(icon_button(
                        "rebuild-index",
                        IconButtonProps { active: false, disabled: file_indexing },
                        t,
                        "folder-sync",
                        14.,
                        move |_, cx| palette_entity.update(cx, |p, cx| p.rebuild_index(cx)),
                    ))
                    .child(icon_button(
                        "refresh-apps",
                        IconButtonProps { active: false, disabled: !apps_loaded },
                        t,
                        "refresh-cw",
                        14.,
                        move |_, cx| palette_entity2.update(cx, |p, cx| p.refresh_apps(cx)),
                    ))
                    .child(self.power_menu.clone()),
            )
            .into_any_element()
    }
}

/// `animate-spin`: one turn per second, linear.
pub fn spinning_icon(name: &str, size: f32, color: Rgba) -> gpui::AnimationElement<gpui::Svg> {
    use gpui::{Animation, AnimationExt, Transformation, percentage};
    icon(name, size, color).with_animation(
        gpui::ElementId::Name(format!("spin-{name}").into()),
        Animation::new(Duration::from_secs(1)).repeat(),
        |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
    )
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, zoom) = theme(cx);
        if std::mem::take(&mut self.reveal_pending) {
            self.reveal_selected(cx);
        }
        let shortcut = display_shortcut(&self.app.read(cx).settings.shortcut);
        let settings_open = self.app.read(cx).open_settings;
        let app = self.app.clone();
        let search = render_search_input(
            SearchInputProps { input: &self.input, shortcut, settings_open },
            &t,
            zoom,
            window,
            cx,
            move |_, cx| {
                app.update(cx, |app, cx| {
                    let open = !app.open_settings;
                    app.set_open_settings(open, cx)
                })
            },
        );
        let results = self.render_results(&t, zoom, window, cx);
        let footer = self.render_footer(&t, cx);
        let previews = self.render_previews(&t, zoom, window, cx);

        // Window-level pointer listeners while a drag owns the pointer (A6).
        let listeners = self.active_drag.map(|_| {
            let weak = cx.weak_entity();
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let w = weak.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = w.update(cx, |this, cx| this.pointer_move(e.position, window, cx));
                        }
                    });
                    let w = weak.clone();
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture && e.button == MouseButton::Left {
                            let _ = w.update(cx, |this, cx| {
                                let release_item = this.reorder_drag.as_ref().map(|d| d.item.clone())
                                    .or_else(|| this.file_drag.as_ref().map(|d| d.item.clone()));
                                let from_icon = matches!(this.active_drag, Some(ActiveDrag::Reorder | ActiveDrag::File));
                                let dragged = this.pointer_up(window, cx);
                                // The release that ends an active drag is not an activation.
                                this.suppress_click = dragged;
                                let _ = (release_item, from_icon);
                            });
                        }
                    });
                },
            )
            .absolute()
            .size_full()
        });

        let menu = self.result_menu.clone();
        div()
            .id("palette-shell")
            .key_context("Palette")
            .on_key_down(cx.listener(Self::on_search_key))
            .on_scroll_wheel(|_: &ScrollWheelEvent, _, _| {})
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .child(search)
            .child(results)
            .children(menu)
            .children(previews)
            .child(footer)
            .children(listeners)
    }
}

// PORT STATUS: confidence=medium todos=0
