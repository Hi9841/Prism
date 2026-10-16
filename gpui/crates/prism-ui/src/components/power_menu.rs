//! Power options (`src/components/PowerMenu.tsx`).

use std::time::{Duration, Instant};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyDownEvent, ParentElement, Render, Styled, Task, Timer, Window,
    deferred, div, prelude::*,
};

use crate::components::menu::{self, Tone};
use crate::components::ui::{IconButtonProps, icon_button, theme};
use crate::gfx::{r, reduced_motion};
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
    changed_at: Instant,
    busy: Option<&'static str>,
    focus: FocusHandle,
    focused_index: usize,
    /// The pointer is over the open menu. The menu sits outside the root's
    /// bounds, so a press on it must not count as a press outside.
    menu_hovered: bool,
    close_task: Option<Task<()>>,
    _subscriptions: Vec<gpui::Subscription>,
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
            changed_at: Instant::now(),
            busy: None,
            focus: cx.focus_handle(),
            focused_index: 0,
            menu_hovered: false,
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
        self.changed_at = Instant::now();
        self.focused_index = focus_index;
        self.menu_hovered = false;
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
            cx.notify();
            return;
        }
        self.closing = true;
        self.changed_at = Instant::now();
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
                window.blur();
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
            IconButtonProps { label: "Power options", active: open, disabled: false, focus: None },
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
        );
        // `.power-menu`: opacity + translateY(4px) + scale(0.98) over 110 ms.
        let duration = if reduced_motion() { 100.0 } else { POPOVER_MOTION_MS as f32 };
        let elapsed = self.changed_at.elapsed().as_millis() as f32 / duration;
        let progress = if self.closing { 1.0 - elapsed.min(1.0) } else { elapsed.min(1.0) };
        if open && elapsed < 1.0 {
            window.request_animation_frame();
        }
        let eased = motion::ease_out_soft(progress);
        let lift = if reduced_motion() { 0.0 } else { 4.0 * (1.0 - eased) };
        let focus_visible = self.focus.is_focused(window);
        let busy = self.busy.is_some();
        div()
            .id("power-menu-root")
            .relative()
            .flex()
            .flex_none()
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if !this.menu_hovered {
                        this.close_menu(true, cx);
                    }
                }))
                // Deferred: the menu paints above the result list. Occlude: a
                // press on the menu must not also reach the row below it.
                .child(deferred(
                    menu::surface(&t, zoom)
                        .id("power-menu")
                        .occlude()
                        .on_hover(cx.listener(|this, hovered: &bool, _, _| this.menu_hovered = *hovered))
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
                            let row = menu::item(("power-action", index), glyph, label, tone, highlighted, busy, &t)
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
                ))
            })
            .child(trigger.when(expanded, |d| d))
    }
}

// PORT STATUS: confidence=medium todos=0
