//! The root view (`src/App.tsx`): the launcher phase machine, the open and
//! close motion, the global zoom keys, and the shell surface.

use std::time::Duration;

use gpui::{
    AnyElement, Context, DispatchPhase, Entity, Keystroke, ParentElement, Render, ScrollDelta, ScrollWheelEvent,
    Styled, Subscription, Task, Timer, Window, canvas, div, prelude::*, px, relative,
};

use crate::components::settings_sheet::SettingsSheet;
use crate::components::toast::render_toast_stack;
use crate::components::ui::theme;
use crate::features::palette::palette::Palette;
use crate::gfx::{Tween, r, reduced_motion};
use crate::lib::bridge::{self, BackendEvent};
use crate::lib::launcher_motion::{
    HideInvocation, HideOrigin, LAUNCHER_MOTION_MS, LauncherEvent, LauncherPhase, hide_palette_invocation,
    reduce_launcher_phase,
};
use crate::state::app::{AppState, app_state};
use crate::state::palette::{PaletteState, palette_state};
use crate::theme::{Theme, fade, hsla, motion, white};

/// Chromium on Windows: one wheel line is 100/3 CSS px (Step 2, M3).
const CHROMIUM_PX_PER_LINE: f32 = 100.0 / 3.0;

pub struct Launcher {
    app: Entity<AppState>,
    palette_state: Entity<PaletteState>,
    palette: Entity<Palette>,
    settings: Entity<SettingsSheet>,
    phase: LauncherPhase,
    stage: Tween,
    close_timer: Option<Task<()>>,
    blur_check: Option<Task<()>>,
    present_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Launcher {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = app_state(cx);
        let palette_state = palette_state(cx);
        let palette = cx.new(|cx| Palette::new(window, cx));
        let settings = cx.new(|cx| SettingsSheet::new(window, cx));
        let hub = bridge::event_hub(cx);
        let hub_sub = cx.subscribe_in(&hub, window, |this, _, event: &BackendEvent, window, cx| match event {
            BackendEvent::Toggle { open } => this.on_toggle(*open, window, cx),
            BackendEvent::WindowFocused(focused) => this.on_window_focused(*focused, window, cx),
            BackendEvent::CloseRequested => this.hide(HideOrigin::Web, cx),
            _ => {}
        });
        let app_obs = cx.observe(&app, |_, _, cx| cx.notify());
        // The native focus events: the shell runs its click-away logic, then the
        // UI hears `onFocusChanged`.
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            let active = window.is_window_active();
            bridge::shell(cx).window_activation_changed(active, cx);
            bridge::emit_event(BackendEvent::WindowFocused(active), cx);
        });
        // Keep zoom shortcuts global so they work from both search and settings (A5).
        let weak = cx.weak_entity();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            let handled = weak.update(cx, |this, cx| this.zoom_key(&event.keystroke, window, cx)).unwrap_or(false);
            if handled {
                cx.stop_propagation();
            }
        });
        let mut this = Self {
            app,
            palette_state,
            palette,
            settings,
            phase: LauncherPhase::Hidden,
            stage: Tween::at(0.0),
            close_timer: None,
            blur_check: None,
            present_task: None,
            _subscriptions: vec![hub_sub, app_obs, activation, intercept],
        };
        // Cold-start sync: reflect a window that is already shown.
        if bridge::shell(cx).is_window_visible(cx) {
            this.set_phase(LauncherPhase::Preparing, window, cx);
        }
        this
    }

    fn zoom_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.app.read(cx).ready {
            return false;
        }
        let palette_input = self.palette.read(cx).input.clone();
        if palette_input.read(cx).is_composing() {
            return false;
        }
        let m = keystroke.modifiers;
        if !m.control || m.alt || m.platform {
            return false;
        }
        // GPUI reports shifted OEM keys shifted (`+`, `_`) and numpad keys as `+`, `-`, `0`.
        let direction = match keystroke.key.as_str() {
            "up" | "+" | "=" => Some(1),
            "down" | "-" | "_" => Some(-1),
            _ => None,
        };
        let _ = window;
        if keystroke.key == "0" {
            self.app.update(cx, |app, cx| app.reset_view_zoom(cx));
            true
        } else if let Some(direction) = direction {
            self.app.update(cx, |app, cx| app.change_view_zoom(direction, cx));
            true
        } else {
            false
        }
    }

    fn set_phase(&mut self, next: LauncherPhase, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase == next {
            return;
        }
        self.phase = next;
        // `.launcher-stage`: opacity and scale over 110 ms ease-out-soft;
        // reduced motion keeps opacity at 100 ms.
        let duration = if reduced_motion() { 100 } else { LAUNCHER_MOTION_MS };
        let target = if next == LauncherPhase::Visible { 1.0 } else { 0.0 };
        if next == LauncherPhase::Hidden {
            self.stage.jump(0.0);
        } else {
            self.stage.set(target, duration, motion::ease_out_soft);
        }
        if next == LauncherPhase::Preparing {
            self.present(window, cx);
        }
        cx.notify();
    }

    fn clear_close_timer(&mut self) {
        self.close_timer = None;
    }

    fn hide(&mut self, origin: HideOrigin, cx: &mut Context<Self>) {
        bridge::emit_event(BackendEvent::DismissTransientUi, cx);
        let event = if origin == HideOrigin::Native { LauncherEvent::NativeClose } else { LauncherEvent::WebClose };
        let next = reduce_launcher_phase(self.phase, event);
        if next == self.phase {
            return;
        }
        self.clear_close_timer();
        self.phase = next;
        let duration = if reduced_motion() { 100 } else { LAUNCHER_MOTION_MS };
        if next == LauncherPhase::Hidden {
            self.stage.jump(0.0);
        } else {
            self.stage.set(0.0, duration, motion::ease_out_soft);
        }
        let defer = match hide_palette_invocation(origin, next) {
            HideInvocation::Instant => Some(false),
            HideInvocation::Deferred => Some(true),
            HideInvocation::None => None,
        };
        if let Some(defer) = defer
            && let Err(error) = bridge::shell(cx).hide_palette(defer, cx)
        {
            eprintln!("Prism could not hide the palette window: {error}");
        }
        if next == LauncherPhase::Closing {
            self.close_timer = Some(cx.spawn(async move |this, cx| {
                Timer::after(Duration::from_millis(LAUNCHER_MOTION_MS)).await;
                let _ = this.update(cx, |this, cx| {
                    this.close_timer = None;
                    let next = reduce_launcher_phase(this.phase, LauncherEvent::ExitFinished);
                    if next != this.phase {
                        this.phase = next;
                        this.stage.jump(0.0);
                        cx.notify();
                    }
                });
            }));
        }
        cx.notify();
    }

    fn on_toggle(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !open {
            self.hide(HideOrigin::Native, cx);
            return;
        }
        bridge::emit_event(BackendEvent::DismissTransientUi, cx);
        // Fresh state on every open: no leftover query, selection or settings
        // overlay - the palette starts from zero.
        self.palette_state.update(cx, |p, cx| p.reset(cx));
        self.app.update(cx, |app, cx| app.set_open_settings(false, cx));
        self.clear_close_timer();
        let next = reduce_launcher_phase(self.phase, LauncherEvent::Open);
        if next == LauncherPhase::Visible {
            self.phase = LauncherPhase::Visible;
            let duration = if reduced_motion() { 100 } else { LAUNCHER_MOTION_MS };
            self.stage.set(1.0, duration, motion::ease_out_soft);
            match bridge::shell(cx).present_palette(cx) {
                Ok(true) => self.focus_search(window, cx),
                _ => {
                    let next = reduce_launcher_phase(self.phase, LauncherEvent::PresentFailed);
                    self.set_phase(next, window, cx);
                }
            }
            cx.notify();
        } else {
            self.set_phase(next, window, cx);
        }
    }

    /// Commit the prepared view while the window is still hidden, then present
    /// it and begin the entrance on the next frame.
    fn present(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let presented = bridge::shell(cx).present_palette(cx);
        match presented {
            Ok(true) => {
                let weak = cx.weak_entity();
                window.on_next_frame(move |window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        let next = reduce_launcher_phase(this.phase, LauncherEvent::Presented);
                        if next != this.phase {
                            this.phase = next;
                            let duration = if reduced_motion() { 100 } else { LAUNCHER_MOTION_MS };
                            this.stage.set(1.0, duration, motion::ease_out_soft);
                        }
                        this.focus_search(window, cx);
                        cx.notify();
                    });
                });
            }
            _ => {
                let next = reduce_launcher_phase(self.phase, LauncherEvent::PresentFailed);
                self.phase = next;
                self.stage.jump(0.0);
            }
        }
        self.present_task = None;
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette.update(cx, |palette, cx| palette.focus_search(window, cx));
    }

    fn on_window_focused(&mut self, focused: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if focused {
            self.blur_check = None;
            return;
        }
        self.blur_check = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(60)).await;
            let _ = this.update(cx, |this, cx| {
                this.blur_check = None;
                if !bridge::shell(cx).is_window_visible(cx) {
                    // Instant native hide (item launch) already took the window.
                    bridge::emit_event(BackendEvent::DismissTransientUi, cx);
                    this.clear_close_timer();
                    this.phase = LauncherPhase::Hidden;
                    this.stage.jump(0.0);
                    cx.notify();
                    return;
                }
                // A dialog Prism opened (the custom icon picker) holds the
                // focus: the native side keeps the window, so the view stays.
                if bridge::shell(cx).owns_foreground() {
                    return;
                }
                // The native side decides click-away (it emits the close). If
                // it kept the palette open, closing only the view would leave
                // a visible, empty window that the next Win press would close.
                if bridge::shell(cx).palette_held_open() {
                    return;
                }
                // Click-away: the window is still up for the shared hide delay.
                if matches!(this.phase, LauncherPhase::Visible | LauncherPhase::Preparing) {
                    this.hide(HideOrigin::Native, cx);
                }
            });
        }));
    }
}

/// The `.shell` surface: rounded 24/24/8/8, a 1px line, the accent glow over
/// the solid gradient, the inset top highlight, and the inset 1px ring.
fn shell(t: &Theme, content: AnyElement) -> gpui::Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .rounded_tl(r(24.))
        .rounded_tr(r(24.))
        .rounded_bl(r(8.))
        .rounded_br(r(8.))
        .border_1()
        .border_color(hsla(t.shell_line))
        .bg(t.shell_solid.background())
        // `radial-gradient(120% 90% at 50% -20%, color-mix(in oklab, accent 8%, transparent) 0%,
        // transparent 55%)` over the solid gradient. A background is placed in the padding box;
        // the `color-mix()` stop is not a legacy color, so the gradient mixes in Oklab.
        .child(
            shell_inset().bg(gpui::radial_gradient(
                gpui::point(0.5, -0.2),
                gpui::size(1.2, 0.9),
                gpui::linear_color_stop(hsla(fade(t.accent, 8.0)), 0.0),
                gpui::linear_color_stop(hsla(fade(t.accent, 0.0)), 0.55),
            )
            .color_space(gpui::ColorSpace::Oklab)),
        )
        // `box-shadow: inset 0 1px 0 var(--t-shell-hi), inset 0 0 0 1px rgb(255 255 255 / .02)`.
        // Inset shadows follow the padding box (the radii shrink by the 1px border) and
        // the first one paints on top, so the ring goes first.
        .child(shell_inset().border_1().border_color(hsla(white(0.02))))
        .child(shell_inset().border_t(px(1.)).border_color(hsla(t.shell_hi)))
        .child(content)
}

/// An overlay on the padding box of `.shell`: radii 24/24/8/8 minus the 1px border.
fn shell_inset() -> gpui::Div {
    div().absolute().inset_0().rounded_tl(r(23.)).rounded_tr(r(23.)).rounded_bl(r(7.)).rounded_br(r(7.))
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, zoom) = theme(cx);
        // View zoom through the rem size only (A11).
        window.set_rem_size(px(16.0 * zoom));
        // A new frame: the settings panel records itself again when it renders.
        crate::gfx::set_sheet_panel(None);
        let opacity = self.stage.animate(window);
        let interactive = self.phase == LauncherPhase::Visible;
        let body = crate::components::focus::body(cx);
        // A focused element that left the tree hands focus to the body.
        if window.focused(cx).is_none() && interactive {
            window.focus(&body);
        }

        // Ctrl + wheel zoom (capture phase, before the lists scroll).
        let weak = cx.weak_entity();
        let wheel = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let weak = weak.clone();
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !event.modifiers.control {
                        return;
                    }
                    let zoom = window.rem_size() / px(16.0);
                    // Chromium's deltaY: opposite sign, 100/3 px per line, divided by the zoom.
                    let delta_y = match event.delta {
                        ScrollDelta::Lines(lines) => -lines.y * CHROMIUM_PX_PER_LINE / zoom,
                        ScrollDelta::Pixels(pixels) => -f32::from(pixels.y) / zoom,
                    };
                    if delta_y == 0.0 {
                        return;
                    }
                    cx.stop_propagation();
                    let _ = weak.update(cx, |this, cx| this.app.update(cx, |app, cx| app.zoom_wheel(delta_y, cx)));
                });
            },
        )
        .absolute()
        .size_full();

        let toasts = render_toast_stack(&self.app, &t, zoom, window, cx);
        let backdrop = crate::gfx::parity().map(|p| div().absolute().inset_0().bg(hsla(p.backdrop)));
        crate::components::focus::root_keys(div().id("launcher-root"), &body)
            .relative()
            .size_full()
            .children(backdrop)
            .child(self.render_stage(opacity, interactive, toasts, wheel, &t))
            .child(crate::components::focus::root_listeners(zoom))
    }
}

impl Launcher {
    fn render_stage(&self, opacity: f32, interactive: bool, toasts: impl IntoElement, wheel: impl IntoElement, t: &Theme) -> impl IntoElement {
        let t = t.clone();
        div()
            .id("launcher-stage")
            .relative()
            .size_full()
            .font_family(crate::theme::FONT_SANS)
            .text_size(r(14.))
            .line_height(relative(1.45))
            .text_color(hsla(t.fg))
            .opacity(opacity)
            // `scale(0.98)` to `scale(1)` with the opacity (same duration and easing);
            // reduced motion drops the transform.
            .when(!reduced_motion(), |d| d.scale(0.98 + 0.02 * opacity))
            .child(shell(&t, self.palette.clone().into_any_element()))
            .child(self.settings.clone())
            .child(toasts)
            .child(wheel)
            .when(!interactive, |d| {
                // `pointer-events: none` outside the visible phase.
                d.child(div().id("stage-blocker").absolute().inset_0().occlude())
            })
    }
}

// PORT STATUS: confidence=medium todos=0
