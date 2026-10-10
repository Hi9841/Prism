//! The result context menu (`src/features/palette/ResultContextMenu.tsx`).

use std::time::{Duration, Instant};

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, ParentElement, Pixels, Point,
    Render, Styled, Task, Timer, Window, div, prelude::*, px, relative,
};

use crate::components::ui::{theme, weight};
use crate::gfx::{icon, motion_ms, r, reduced_motion, zoomed};
use crate::lib::bridge;
use crate::lib::launcher_motion::POPOVER_MOTION_MS;
use crate::lib::types::PaletteItem;
use crate::theme::{hsla, motion, shadow_pop};

const MENU_WIDTH: f32 = 196.0;
const ITEM_HEIGHT: f32 = 36.0;
const PADDING: f32 = 8.0;
const VIEWPORT_MARGIN: f32 = 8.0;

/// A menu position in CSS px (window coordinates divided by the zoom).
#[derive(Clone, Copy, Debug)]
pub struct ContextMenuPosition {
    pub x: f32,
    pub y: f32,
}

/// Keeps the menu inside the viewport (`clampContextMenuPosition`).
pub fn clamp_context_menu_position(x: f32, y: f32, menu_height: f32, viewport: (f32, f32)) -> ContextMenuPosition {
    ContextMenuPosition {
        x: x.max(VIEWPORT_MARGIN).min(viewport.0 - MENU_WIDTH - VIEWPORT_MARGIN),
        y: y.max(VIEWPORT_MARGIN).min(viewport.1 - menu_height - VIEWPORT_MARGIN),
    }
}

/// Rows the menu will render; clamping sizes from it.
pub fn result_action_count(item: &PaletteItem) -> usize {
    [item.open_location.is_some(), item.run_as_admin.is_some(), item.toggle_taskbar_pin.is_some(), item.show_properties.is_some()]
        .into_iter()
        .filter(|present| *present)
        .count()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuChoice {
    OpenLocation,
    RunAsAdmin,
    /// The argument is the pin state the label showed.
    ToggleTaskbarPin(bool),
    ShowProperties,
}

pub enum MenuEvent {
    Chose(MenuChoice),
    /// The menu is gone; `restore_focus` returns focus to the search field.
    Closed { restore_focus: bool },
}

pub struct ResultContextMenu {
    pub item: PaletteItem,
    position: ContextMenuPosition,
    focus: FocusHandle,
    focused_index: usize,
    closing: Option<Instant>,
    opened: Instant,
    pinned_to_taskbar: Option<bool>,
    _pin_query: Option<Task<()>>,
    close_task: Option<Task<()>>,
}

impl EventEmitter<MenuEvent> for ResultContextMenu {}

impl ResultContextMenu {
    pub fn new(item: PaletteItem, position: ContextMenuPosition, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // Focus the first menu item.
        window.focus(&focus);
        // The shell owns the pin state; resolve it lazily so the menu reads
        // "Unpin from taskbar" exactly when the target is already pinned.
        let pin_query = match (&item.toggle_taskbar_pin, &item.shell_path) {
            (Some(_), Some(path)) => {
                let path = path.clone();
                Some(cx.spawn(async move |this, cx| {
                    let pinned = bridge::is_pinned_to_taskbar(path).await.ok();
                    let _ = this.update(cx, |this, cx| {
                        this.pinned_to_taskbar = pinned;
                        cx.notify();
                    });
                }))
            }
            _ => None,
        };
        Self {
            item,
            position,
            focus,
            focused_index: 0,
            closing: None,
            opened: Instant::now(),
            pinned_to_taskbar: None,
            _pin_query: pin_query,
            close_task: None,
        }
    }

    fn choices(&self) -> Vec<MenuChoice> {
        let mut out = Vec::new();
        if self.item.open_location.is_some() {
            out.push(MenuChoice::OpenLocation);
        }
        if self.item.run_as_admin.is_some() {
            out.push(MenuChoice::RunAsAdmin);
        }
        if self.item.toggle_taskbar_pin.is_some()
            && let Some(pinned) = self.pinned_to_taskbar
        {
            out.push(MenuChoice::ToggleTaskbarPin(pinned));
        }
        if self.item.show_properties.is_some() {
            out.push(MenuChoice::ShowProperties);
        }
        out
    }

    /// Deliberate closes animate out first; closes triggered by the palette
    /// hiding snap since the window is gone.
    pub fn close(&mut self, restore_focus: bool, animate: bool, cx: &mut Context<Self>) {
        if self.closing.is_some() {
            return;
        }
        if !animate {
            cx.emit(MenuEvent::Closed { restore_focus });
            return;
        }
        self.closing = Some(Instant::now());
        self.close_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(POPOVER_MOTION_MS)).await;
            let _ = this.update(cx, |_, cx| cx.emit(MenuEvent::Closed { restore_focus }));
        }));
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.choices().len();
        if count == 0 {
            return;
        }
        match event.keystroke.key.as_str() {
            "down" => self.focused_index = (self.focused_index + 1) % count,
            "up" => self.focused_index = (self.focused_index + count - 1) % count,
            "home" => self.focused_index = 0,
            "end" => self.focused_index = count - 1,
            "escape" | "tab" => self.close(true, true, cx),
            "enter" | "space" => {
                let choice = self.choices()[self.focused_index.min(count - 1)];
                cx.emit(MenuEvent::Chose(choice));
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Focusable for ResultContextMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ResultContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, zoom) = theme(cx);
        // `.context-menu-enter`: opacity and scale(0.98) over 110 ms; exit back.
        // Reduced motion keeps the opacity transition only (100 ms).
        let (duration, reduced) = if reduced_motion() { (100.0, true) } else { (POPOVER_MOTION_MS as f32, false) };
        let progress = match self.closing {
            Some(start) => 1.0 - (start.elapsed().as_millis() as f32 / duration).min(1.0),
            None => (self.opened.elapsed().as_millis() as f32 / duration).min(1.0),
        };
        if progress > 0.0 && progress < 1.0 || self.closing.is_some() && progress > 0.0 {
            window.request_animation_frame();
        }
        let eased = motion::ease_out_soft(progress);
        let _ = (reduced, motion_ms(0));
        let choices = self.choices();
        let focused = self.focused_index.min(choices.len().saturating_sub(1));
        let focus_visible = self.focus.is_focused(window);
        div()
            .id("result-menu-layer")
            .absolute()
            .inset_0()
            .child(
                // Clicks outside close the menu.
                div()
                    .id("result-menu-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close(true, true, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.close(true, true, cx))),
            )
            .child(
                div()
                    .id("result-menu")
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .absolute()
                    .left(r(self.position.x))
                    .top(r(self.position.y))
                    .w(r(MENU_WIDTH))
                    .overflow_hidden()
                    .rounded(r(8.))
                    .border_1()
                    .border_color(hsla(t.line))
                    // D4: the backdrop blur becomes a near-opaque fill.
                    .bg(hsla(crate::theme::Rgba { a: 0.97, ..t.bg_raised }))
                    .p(r(3.))
                    .shadow(zoomed(shadow_pop(), zoom))
                    .opacity(eased)
                    .children(choices.into_iter().enumerate().map(|(index, choice)| {
                        let (glyph, label) = match choice {
                            MenuChoice::OpenLocation => ("folder-open", "Open location"),
                            MenuChoice::RunAsAdmin => ("shield-check", "Run as administrator"),
                            MenuChoice::ToggleTaskbarPin(true) => ("pin-off", "Unpin from taskbar"),
                            MenuChoice::ToggleTaskbarPin(false) => ("pin", "Pin to taskbar"),
                            MenuChoice::ShowProperties => ("info", "Properties"),
                        };
                        let is_focused = focus_visible && index == focused;
                        div()
                            .id(("result-menu-item", index))
                            .flex()
                            .items_center()
                            .gap(r(8.))
                            .h(r(ITEM_HEIGHT))
                            .w_full()
                            .px(r(10.))
                            .rounded(r(4.))
                            .text_size(r(12.5))
                            .font_weight(weight(500.))
                            .line_height(relative(1.45))
                            .text_color(hsla(t.fg_secondary))
                            .hover(|s| s.bg(hsla(t.surface_hover)).text_color(hsla(t.fg)))
                            .when(is_focused, |d| {
                                d.border_1().border_color(hsla(crate::theme::fade(t.accent, 65.0)))
                            })
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(MenuEvent::Chose(choice))))
                            .child(icon(glyph, 16., t.accent))
                            .child(label)
                    })),
            )
    }
}

/// The menu position for a pointer at `point` (window px).
pub fn menu_position_for(point: Point<Pixels>, item: &PaletteItem, zoom: f32, viewport: (f32, f32)) -> ContextMenuPosition {
    let menu_height = result_action_count(item) as f32 * ITEM_HEIGHT + PADDING;
    clamp_context_menu_position(f32::from(point.x) / zoom, f32::from(point.y) / zoom, menu_height, viewport)
}

pub fn px_of(value: f32) -> Pixels {
    px(value)
}

// PORT STATUS: confidence=medium todos=0
