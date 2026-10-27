//! CSS transitions of colors, opacity, and scale (`transition-colors`, `transition-opacity`,
//! `transition-[...]`, `.press`): one keyed tween per element property, and
//! `:hover` and `:active` as data so a call site can pick the target value.
//!
//! The first frame of an element shows its target with no animation (a new
//! element has no "before" style). Reduced motion jumps (`0.01ms !important`).

use gpui::{
    App, DispatchPhase, Div, ElementId, Entity, HitboxBehavior, InteractiveElement, KeyDownEvent, KeyUpEvent, Keystroke,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Rgba, Stateful, Styled, Window, canvas, div,
};

use crate::gfx::{Tween, linear, motion_ms};
use crate::theme::{mix, motion};

/// A `transition-duration` and `transition-timing-function` pair.
#[derive(Clone, Copy)]
pub struct Timing {
    pub ms: u64,
    pub ease: fn(f32) -> f32,
}

impl Timing {
    /// A Tailwind `transition-*` class with `duration-<ms>` and no `ease-*` class.
    pub const fn tw(ms: u64) -> Self {
        Self { ms, ease: motion::tailwind_default }
    }

    /// `.press`: color, background-color, border-color, opacity, and scale, 150ms `--ease-out-soft`.
    pub const PRESS: Self = Self { ms: 150, ease: motion::ease_out_soft };

    /// A property that `transition-property` leaves out: it jumps.
    pub const NONE: Self = Self { ms: 0, ease: linear };
}

/// `:hover` as data: the flag and an overlay child that records it. Add the
/// overlay with `.child(..)`. It has no click listener, so a press keeps the
/// hover (CSS `:hover` stays on while the button is down).
///
/// The overlay reads its hitbox at each paint, as GPUI's own `.hover()` style
/// does, so a scroll or a layout change under a still pointer moves the hover.
/// A mouse move updates it between paints. The flag is off while the pointer
/// is outside the window (GPUI keeps the last pointer position).
pub fn hover(id: &ElementId, window: &mut Window, cx: &mut App) -> (bool, Stateful<Div>) {
    let state = window.use_keyed_state((id.clone(), "hover"), cx, |_, _| false);
    let hovered = *state.read(cx) && window.is_window_hovered();
    let probe = canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, cx| {
            // GPUI's `on_hover` also reports no hover during a drag.
            let sync = move |window: &Window, cx: &mut App| {
                set_flag(&state, hitbox.is_hovered(window) && !cx.has_active_drag(), cx)
            };
            sync(window, cx);
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    sync(window, cx);
                }
            });
        },
    )
    .size_full();
    (hovered, div().id((id.clone(), "hover-hit")).absolute().inset_0().child(probe))
}

fn set_flag(state: &Entity<bool>, on: bool, cx: &mut App) {
    state.update(cx, |flag, cx| {
        if *flag != on {
            *flag = on;
            cx.notify();
        }
    })
}

fn is_space(keystroke: &Keystroke) -> bool {
    keystroke.key == "space" && !keystroke.modifiers.modified()
}

/// `:active` as data, with the listeners that record it. A left mouse down on
/// the element starts it; a left mouse up anywhere ends it. Space held while
/// the element has focus also activates it, as Chromium does for a `<button>`
/// (GPUI clicks on the key up).
///
/// The mouse up and the Space key up end it in the capture phase. Thus a
/// bubble-phase listener that stops propagation (for example a click listener,
/// which GPUI runs before the element's key listeners) cannot keep the element
/// active. A capture-phase listener that stops propagation before it can.
pub fn active<E: InteractiveElement>(element: E, id: &ElementId, window: &mut Window, cx: &mut App) -> (bool, E) {
    let state = window.use_keyed_state((id.clone(), "active"), cx, |_, _| false);
    let active = *state.read(cx);
    let (down, up, up_out, key_down, key_up) = (state.clone(), state.clone(), state.clone(), state.clone(), state);
    let element = element
        .capture_any_mouse_down(move |event: &MouseDownEvent, _, cx| {
            if event.button == MouseButton::Left {
                set_flag(&down, true, cx);
            }
        })
        .capture_any_mouse_up(move |event: &MouseUpEvent, _, cx| {
            if event.button == MouseButton::Left {
                set_flag(&up, false, cx);
            }
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| set_flag(&up_out, false, cx))
        .on_key_down(move |event: &KeyDownEvent, _, cx| {
            if is_space(&event.keystroke) {
                set_flag(&key_down, true, cx);
            }
        })
        .capture_key_up(move |event: &KeyUpEvent, _, cx| {
            if is_space(&event.keystroke) {
                set_flag(&key_up, false, cx);
            }
        });
    (active, element)
}

/// `active:scale-*`: the element paints at `factor` around its center while
/// it is active and `enabled` (`:not(:disabled)`). `timing` is the `scale`
/// transition ([`Timing::NONE`] when `transition-property` leaves it out).
pub fn active_scale<E: InteractiveElement + Styled>(
    element: E,
    id: &ElementId,
    factor: f32,
    timing: Timing,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> E {
    let (active, element) = active(element, id, window, cx);
    let scale = number(id, "scale", if active && enabled { factor } else { 1.0 }, timing, window, cx);
    element.scale(scale)
}

/// `.press:active:not(:disabled) { scale: 0.96 }` with the `.press` transition.
pub fn press<E: InteractiveElement + Styled>(element: E, id: &ElementId, enabled: bool, window: &mut Window, cx: &mut App) -> E {
    active_scale(element, id, 0.96, Timing::PRESS, enabled, window, cx)
}

/// The common button case: a text color and a background, each `(rest, hover)`.
pub struct HoverPaint {
    pub hovered: bool,
    pub text: Rgba,
    pub bg: Rgba,
    /// The overlay from [`hover`]; add it as a child.
    pub tracker: Stateful<Div>,
}

pub fn hover_paint(
    id: &ElementId,
    text: (Rgba, Rgba),
    bg: (Rgba, Rgba),
    timing: Timing,
    window: &mut Window,
    cx: &mut App,
) -> HoverPaint {
    let (hovered, tracker) = hover(id, window, cx);
    let pick = |(rest, on): (Rgba, Rgba)| if hovered { on } else { rest };
    let text = color(id, "text", pick(text), timing, window, cx);
    let bg = color(id, "bg", pick(bg), timing, window, cx);
    HoverPaint { hovered, text, bg, tracker }
}

#[derive(Clone, Copy)]
struct ColorTween {
    from: Rgba,
    to: Rgba,
    progress: Tween,
}

impl ColorTween {
    /// Chromium interpolates colors premultiplied, in Oklab when an end is not
    /// a legacy sRGB color (the `oklch()` tokens). Every legacy token pair here
    /// has one hue or a transparent end, where both spaces give the same result.
    fn value(&self) -> Rgba {
        let p = self.progress.value();
        if p >= 1.0 { self.to } else { mix(self.to, p * 100.0, self.from) }
    }
}

/// A transition of one color property (`prop` names it within the element).
pub fn color(id: &ElementId, prop: &'static str, target: Rgba, timing: Timing, window: &mut Window, cx: &mut App) -> Rgba {
    let state = window.use_keyed_state((id.clone(), prop), cx, |_, _| ColorTween {
        from: target,
        to: target,
        progress: Tween::at(1.0),
    });
    if state.read(cx).to != target {
        state.update(cx, |c, _| {
            c.from = c.value();
            c.to = target;
            c.progress.play(0.0, 1.0, motion_ms(timing.ms), timing.ease);
        });
    }
    let tween = *state.read(cx);
    tween.progress.animate(window);
    tween.value()
}

/// A transition of one number property, for example `opacity`.
pub fn number(id: &ElementId, prop: &'static str, target: f32, timing: Timing, window: &mut Window, cx: &mut App) -> f32 {
    let state = window.use_keyed_state((id.clone(), prop), cx, |_, _| Tween::at(target));
    if state.read(cx).target() != target {
        state.update(cx, |t, _| t.set(target, motion_ms(timing.ms), timing.ease));
    }
    state.read(cx).animate(window)
}

// PORT STATUS: confidence=medium todos=0
