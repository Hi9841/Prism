//! Power options (`src/components/PowerMenu.tsx`).

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseDownEvent, ParentElement, Pixels, Render,
    Styled, Task, Timer, Window, canvas, deferred, div, prelude::*,
};

use crate::components::menu::{self, Tone};
use crate::components::ui::{IconButtonProps, icon_button, theme};
use crate::gfx::{Tween, r, reduced_motion, scale_from};
use crate::lib::bridge::{self, BackendEvent};
use crate::lib::launcher_motion::POPOVER_MOTION_MS;
use crate::state::app::{AppEvent, AppState, ToastKind, app_state, error_string};
use crate::theme::motion;

const ACTIONS: [(&str, &str, &str); 4] = [
    ("lock", "Lock", "lock"),
    ("sleep", "Sleep", "moon"),
    ("shutdown", "Shut down", "power"),
    ("restart", "Restart", "rotate-ccw"),
];

pub struct PowerMenu {
    app: Entity<AppState>,
    open: bool,
    closing: bool,
    /// The open state of the popup: 0 is closed, 1 is open.
    presence: Tween,
    busy: Option<&'static str>,
    focus: FocusHandle,
    /// The trigger button; Escape returns focus to it.
    trigger_focus: FocusHandle,
    focused_index: usize,
    /// Painted bounds of the popup. The popup sits outside the trigger, so a
    /// press on it is "outside" the trigger and must not close the menu.
    menu_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    close_task: Option<Task<()>>,
    _subscriptions: Vec<gpui::Subscription>,
}

/// The `.power-menu` transition: 110 ms ease-out-soft; reduced motion: 100 ms.
fn presence_ms() -> u64 {
    if reduced_motion() { 100 } else { POPOVER_MOTION_MS }
}

impl PowerMenu {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let app = app_state(cx);
        let app_sub = cx.subscribe(&app, |this, app, event: &AppEvent, cx| {
            if matches!(event, AppEvent::OpenSettingsChanged) && app.read(cx).open_settings {
                this.close_menu(false, cx);
            }
        });
        let hub = bridge::event_hub(cx);
        let hub_sub = cx.subscribe(&hub, |this, _, event: &BackendEvent, cx| {
            if matches!(event, BackendEvent::CloseRequested | BackendEvent::DismissTransientUi) {
                this.close_menu(false, cx);
            }
        });
        Self {
            app,
            open: false,
            closing: false,
            presence: Tween::at(0.0),
            busy: None,
            focus: cx.focus_handle(),
            trigger_focus: cx.focus_handle(),
            focused_index: 0,
            menu_bounds: Rc::default(),
            close_task: None,
            _subscriptions: vec![app_sub, hub_sub],
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn open_menu(&mut self, focus_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.close_task = None;
        self.closing = false;
        self.open = true;
        self.presence.toggle(1.0, presence_ms(), motion::ease_out_soft);
        self.focused_index = focus_index;
        self.menu_bounds.set(None);
        window.focus(&self.focus);
        cx.notify();
    }

    /// Deliberate closes animate out first; closes triggered by the palette
    /// hiding snap since the window is gone.
    pub fn close_menu(&mut self, animate: bool, cx: &mut Context<Self>) {
        if !self.open && !self.closing {
            return;
        }
        self.close_task = None;
        if !animate {
            self.closing = false;
            self.open = false;
            self.presence.jump(0.0);
            cx.notify();
            return;
        }
        self.closing = true;
        self.presence.toggle(0.0, presence_ms(), motion::ease_out_soft);
        self.close_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(POPOVER_MOTION_MS)).await;
            let _ = this.update(cx, |this, cx| {
                this.closing = false;
                this.open = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn run_action(&mut self, action: &'static str, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(action);
        cx.notify();
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let result = bridge::perform_power_action(action).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(()) => {
                        this.open = false;
                        bridge::emit_event(BackendEvent::CloseRequested, cx);
                    }
                    Err(error) => {
                        let verb = if action == "shutdown" { "shut down" } else { action };
                        app.update(cx, |app, cx| {
                            app.show_toast(format!("Could not {verb}"), Some(error_string(&error)), ToastKind::Success, cx)
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_menu_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let count = ACTIONS.len();
        match event.keystroke.key.as_str() {
            "down" => self.focused_index = (self.focused_index + 1) % count,
            "up" => self.focused_index = (self.focused_index + count - 1) % count,
            "home" => self.focused_index = 0,
            "end" => self.focused_index = count - 1,
            "escape" => {
                self.close_menu(true, cx);
                window.focus(&self.trigger_focus);
            }
            "enter" | "space" => {
                let (action, _, _) = ACTIONS[self.focused_index];
                self.run_action(action, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Focusable for PowerMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PowerMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, zoom) = theme(cx);
        let open = self.open;
        let expanded = open && !self.closing;
        let trigger = icon_button(
            "power-trigger",
            IconButtonProps {
                label: "Power options",
                active: open,
                disabled: false,
                focus: Some(self.trigger_focus.clone()),
            },
            &t,
            "power",
            14.,
            {
                let this = cx.entity().downgrade();
                move |window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        if this.open && !this.closing {
                            this.close_menu(true, cx);
                        } else {
                            this.open_menu(0, window, cx);
                        }
                    });
                }
            },
            window,
            cx,
        );
        // `.power-menu`: opacity + translateY(4px) + scale(0.98), in from `@starting-style` and
        // out to `.power-menu-exit` (see `presence_ms`). Reduced motion: `transform: none`.
        let reduced = reduced_motion();
        let eased = if open { self.presence.animate(window) } else { 0.0 };
        let lift = if reduced { 0.0 } else { 4.0 * (1.0 - eased) };
        let scale = if reduced { 1.0 } else { 0.98 + 0.02 * eased };
        let focus_visible = self.focus.is_focused(window);
        let busy = self.busy.is_some();
        div()
            .id("power-menu-root")
            .relative()
            .flex()
            .flex_none()
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let on_menu = this.menu_bounds.get().is_some_and(|bounds| bounds.contains(&event.position));
                    if !on_menu {
                        this.close_menu(true, cx);
                    }
                }))
                // Deferred: the menu paints above the result list. Occlude: a
                // press on the menu must not also reach the row below it. The
                // scale wraps the deferred root (`transform-origin: right bottom`).
                .child(deferred(scale_from(
                    menu::surface(&t, zoom)
                        .id("power-menu")
                        .occlude()
                        .child({
                            let bounds = self.menu_bounds.clone();
                            canvas(
                                move |painted, _, _| bounds.set(Some(painted)),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full()
                        })
                        .track_focus(&self.focus)
                        .on_key_down(cx.listener(Self::on_menu_key_down))
                        .absolute()
                        .right_0()
                        .bottom(r(40. - lift))
                        .w(r(160.))
                        .opacity(eased)
                        .children(ACTIONS.iter().enumerate().flat_map(|(index, (action, label, glyph))| {
                            let action: &'static str = action;
                            let tone = if action == "shutdown" { Tone::Danger } else { Tone::Normal };
                            let highlighted = focus_visible && index == self.focused_index;
                            // Lock stands apart from the actions that end the session.
                            let separator = (index == 1).then(|| menu::separator(&t).into_any_element());
                            let row = menu::item(("power-action", index), glyph, label, tone, highlighted, busy, &t, window, cx)
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    if *hovered && this.focused_index != index {
                                        this.focused_index = index;
                                        cx.notify();
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| this.run_action(action, cx)))
                                .into_any_element();
                            separator.into_iter().chain([row])
                        })),
                    scale,
                    (1.0, 1.0),
                )))
            })
            .child(trigger.when(expanded, |d| d))
    }
}

// PORT STATUS: confidence=medium todos=0
