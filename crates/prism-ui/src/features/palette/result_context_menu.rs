//! The result context menu (`src/features/palette/ResultContextMenu.tsx`).

use std::time::Duration;

use gpui::{
    App, Context, DispatchPhase, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Render, Styled, Task, Timer, Window, canvas, div, prelude::*, px,
};

use crate::components::menu::{self, Tone};
use crate::components::ui::theme;
use crate::gfx::{Tween, r, reduced_motion, scale_from};
use crate::lib::bridge;
use crate::lib::launcher_motion::POPOVER_MOTION_MS;
use crate::lib::types::PaletteItem;
use crate::theme::motion;

const MENU_WIDTH: f32 = 196.0;
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

/// The menu height in CSS px; clamping sizes from it. The taskbar pin row
/// counts before its state resolves, so the menu never grows past the edge.
pub fn menu_height(item: &PaletteItem) -> f32 {
    // "Open" is always there: every result has a menu.
    let groups = [
        [true, item.run_as_admin.is_some()],
        [item.open_location.is_some(), item.copy_path.is_some()],
        [item.toggle_taskbar_pin.is_some(), item.show_properties.is_some()],
    ];
    let rows = groups.iter().flatten().filter(|present| **present).count();
    let shown = groups.iter().filter(|group| group.iter().any(|present| *present)).count();
    rows as f32 * menu::ITEM_HEIGHT + shown.saturating_sub(1) as f32 * menu::SEPARATOR_HEIGHT + menu::PADDING
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuChoice {
    Open,
    OpenLocation,
    CopyAsPath,
    RunAsAdmin,
    /// The argument is the pin state the label showed.
    ToggleTaskbarPin(bool),
    ShowProperties,
}

impl MenuChoice {
    /// Separators split the menu between groups (see `menu_height`).
    fn group(self) -> u8 {
        match self {
            Self::Open | Self::RunAsAdmin => 0,
            Self::OpenLocation | Self::CopyAsPath => 1,
            Self::ToggleTaskbarPin(_) | Self::ShowProperties => 2,
        }
    }
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
    closing: bool,
    /// The open state: 0 is closed, 1 is open.
    presence: Tween,
    pinned_to_taskbar: Option<bool>,
    _pin_query: Option<Task<()>>,
    close_task: Option<Task<()>>,
}

impl EventEmitter<MenuEvent> for ResultContextMenu {}

/// The `.context-menu-enter` transition: 110 ms ease-out-soft; reduced motion: 100 ms.
fn presence_ms() -> u64 {
    if reduced_motion() { 100 } else { POPOVER_MOTION_MS }
}

impl ResultContextMenu {
    pub fn new(item: PaletteItem, position: ContextMenuPosition, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // Focus the first menu item, after the right-button press finished
        // dispatching (the row under the cursor is focusable).
        window.focus(&focus);
        let deferred = focus.clone();
        window.defer(cx, move |window, _| window.focus(&deferred));
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
            closing: false,
            presence: {
                let mut presence = Tween::at(0.0);
                presence.toggle(1.0, presence_ms(), motion::ease_out_soft);
                presence
            },
            pinned_to_taskbar: None,
            _pin_query: pin_query,
            close_task: None,
        }
    }

    /// Choices in menu order: launch, then location, then shell actions.
    fn choices(&self) -> Vec<MenuChoice> {
        let mut out = vec![MenuChoice::Open];
        if self.item.run_as_admin.is_some() {
            out.push(MenuChoice::RunAsAdmin);
        }
        if self.item.open_location.is_some() {
            out.push(MenuChoice::OpenLocation);
        }
        if self.item.copy_path.is_some() {
            out.push(MenuChoice::CopyAsPath);
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
        if self.closing {
            return;
        }
        if !animate {
            cx.emit(MenuEvent::Closed { restore_focus });
            return;
        }
        self.closing = true;
        self.presence.toggle(0.0, presence_ms(), motion::ease_out_soft);
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
        // `.context-menu-enter`: opacity and scale(0.98), in from `@starting-style` and out to
        // `.context-menu-exit` (see `presence_ms`). Reduced motion: `transform: none`.
        let reduced = reduced_motion();
        let eased = self.presence.animate(window);
        let scale = if reduced { 1.0 } else { 0.98 + 0.02 * eased };
        let choices = self.choices();
        let focused = self.focused_index.min(choices.len().saturating_sub(1));
        let focus_visible = self.focus.is_focused(window);
        div()
            .id("result-menu-layer")
            .absolute()
            .inset_0()
            .child(
                // Clicks outside close the menu.
                // The original's backdrop button covers the list: a click
                // outside only closes the menu, it never reaches a row.
                div()
                    .id("result-menu-backdrop")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close(true, true, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.close(true, true, cx))),
            )
            // `transform-origin: 0 0`.
            .child(scale_from(
                menu::surface(&t, zoom)
                    .id("result-menu")
                    // A choice must not also press the backdrop or the row below.
                    .occlude()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .child({
                        let weak = cx.entity().downgrade();
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _cx| {
                                let weak = weak.clone();
                                window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                                    if phase != DispatchPhase::Capture || bounds.contains(&event.position) {
                                        return;
                                    }
                                    let _ = weak.update(cx, |this, cx| {
                                        if !this.closing {
                                            this.close(true, true, cx);
                                        }
                                    });
                                });
                            },
                        )
                        .absolute()
                        .size_full()
                    })
                    .absolute()
                    .left(r(self.position.x))
                    .top(r(self.position.y))
                    .w(r(MENU_WIDTH))
                    .opacity(eased)
                    .children(choices.iter().enumerate().flat_map(|(index, &choice)| {
                        let (glyph, label) = match choice {
                            MenuChoice::Open => ("external-link", "Open"),
                            MenuChoice::CopyAsPath => ("clipboard-copy", "Copy as path"),
                            MenuChoice::OpenLocation => ("folder-open", "Open location"),
                            MenuChoice::RunAsAdmin => ("shield-check", "Run as administrator"),
                            MenuChoice::ToggleTaskbarPin(true) => ("pin-off", "Unpin from taskbar"),
                            MenuChoice::ToggleTaskbarPin(false) => ("pin", "Pin to taskbar"),
                            MenuChoice::ShowProperties => ("info", "Properties"),
                        };
                        let tone = if choice == MenuChoice::Open { Tone::Primary } else { Tone::Normal };
                        let highlighted = focus_visible && index == focused;
                        let separator = (index > 0 && choices[index - 1].group() != choice.group())
                            .then(|| menu::separator(&t).into_any_element());
                        let row = menu::item(("result-menu-item", index), glyph, label, tone, highlighted, false, &t, window, cx)
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if *hovered && this.focused_index != index {
                                    this.focused_index = index;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(MenuEvent::Chose(choice))))
                            .into_any_element();
                        separator.into_iter().chain([row])
                    })),
                scale,
                (0.0, 0.0),
            ))
    }
}

/// The menu position for a pointer at `point` (window px).
pub fn menu_position_for(point: Point<Pixels>, item: &PaletteItem, zoom: f32, viewport: (f32, f32)) -> ContextMenuPosition {
    clamp_context_menu_position(f32::from(point.x) / zoom, f32::from(point.y) / zoom, menu_height(item), viewport)
}

pub fn px_of(value: f32) -> Pixels {
    px(value)
}

// PORT STATUS: confidence=medium todos=0
