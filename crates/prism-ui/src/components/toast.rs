//! Toasts (`src/components/Toast.tsx`).

use gpui::{App, Entity, IntoElement, ParentElement, Styled, Window, div, prelude::*, relative};

use crate::components::focus::keyboard_modality;
use crate::components::ui::weight;
use crate::gfx::{icon, icon_stroke, r, reduced_motion, scale_from, zoomed};
use crate::state::app::{AppState, ToastKind};
use crate::theme::{Rgba, TRANSPARENT, Theme, fade, hsla, motion, shadow_raised, white};
use crate::transition::{self, Timing};

/// The toast's `toast-in` / `toast-out` frame.
struct ToastMotion {
    opacity: f32,
    /// `translateY`, in CSS px.
    offset: f32,
    /// `scale`, around `transform-origin: center bottom`.
    scale: f32,
    animating: bool,
}

/// `toast-in` (from opacity 0, `translateY(10px) scale(0.96)`; 160 ms
/// ease-out-soft) and `toast-out` (to opacity 0, `translateY(6px)
/// scale(0.97)`; 140 ms ease-in). Reduced motion shows the end frame.
fn motion_state(toast: &crate::state::app::Toast) -> ToastMotion {
    let progress = |since: std::time::Instant, ms: f32| {
        if reduced_motion() { 1.0 } else { (since.elapsed().as_millis() as f32 / ms).min(1.0) }
    };
    if let Some(start) = toast.closing {
        let t = progress(start, 140.0);
        let e = motion::ease_in(t);
        return ToastMotion { opacity: 1.0 - e, offset: 6.0 * e, scale: 1.0 - 0.03 * e, animating: t < 1.0 };
    }
    let t = progress(toast.born, 160.0);
    let e = motion::ease_out_soft(t);
    ToastMotion { opacity: e, offset: 10.0 * (1.0 - e), scale: 0.96 + 0.04 * e, animating: t < 1.0 }
}

pub fn render_toast_stack(app: &Entity<AppState>, t: &Theme, zoom: f32, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let toasts = app.read(cx).toasts.clone();
    let mut animating = false;
    let stack = div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(r(16.))
        // `max-w-[calc(100%-2rem)]` per toast.
        .px(r(16.))
        .flex()
        .flex_col()
        .items_center()
        .gap(r(8.))
        .children(toasts.into_iter().map(|toast| {
            let motion = motion_state(&toast);
            animating |= motion.animating;
            let is_error = toast.kind == ToastKind::Error;
            let id = toast.id;
            let app = app.clone();
            // `transition-[background-color,color,transform] duration-110 ease-out`.
            let dismiss_key: gpui::ElementId = ("toast-dismiss", id as usize).into();
            // `active:scale-[0.97]`: `scale` is not in the transition list (Tailwind 4 writes the
            // `scale` property, not `transform`), so it jumps.
            let dismiss_button = is_error.then(|| {
                let dismiss = transition::hover_paint(
                    &dismiss_key,
                    (t.fg_tertiary, t.fg),
                    (TRANSPARENT, t.surface_hover),
                    Timing { ms: 110, ease: motion::tailwind_ease_out },
                    window,
                    cx,
                );
                // `focus-visible:outline-2 outline-offset-[-2px] outline-accent`: an inset
                // 2px ring. The button has a fixed size and a centered icon, so a border
                // draws it without moving the icon.
                let button = div()
                    .id(dismiss_key.clone())
                    .tab_index(0)
                    .tab_stop(true)
                    .when(keyboard_modality(), |d| d.focus(|s| s.border_2().border_color(hsla(t.accent))))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(r(44.))
                    .rounded(r(10.))
                    .bg(hsla(dismiss.bg))
                    .on_click(move |_, _, cx| app.update(cx, |app, cx| app.dismiss_toast(id, cx)))
                    .child(icon("x", 14., dismiss.text))
                    .child(dismiss.tracker);
                transition::active_scale(button, &dismiss_key, 0.97, Timing::NONE, true, window, cx)
            });
            let toast_box = div()
                .id(("toast", toast.id as usize))
                .relative()
                .top(r(motion.offset))
                .opacity(motion.opacity)
                .flex()
                .items_center()
                .gap(r(10.))
                .min_h(r(44.))
                .max_w_full()
                .rounded(r(14.))
                .border_1()
                .border_color(hsla(if is_error { fade(t.danger, 35.0) } else { t.line }))
                // D4: the backdrop blur becomes a near-opaque fill.
                .bg(hsla(Rgba { a: 1.0, ..t.toast }))
                .py(r(6.))
                .pl(r(12.))
                .pr(r(if is_error { 4. } else { 16. }))
                .shadow(zoomed(shadow_raised(t), zoom))
                .child(crate::components::ui::inset_top_highlight(white(0.05), 13.))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(r(20.))
                        .rounded_full()
                        .bg(hsla(if is_error { t.danger_soft } else { fade(t.accent, 90.0) }))
                        .child(if is_error {
                            icon_stroke("circle-alert", 2.5, 14., t.danger)
                        } else {
                            icon_stroke("check", 3., 12., t.accent_fg)
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(r(12.5))
                        .font_weight(weight(500.))
                        .line_height(relative(1.45))
                        .text_color(hsla(t.fg))
                        .child(toast.title.clone())
                        .when_some(toast.detail.clone(), |d, detail| {
                            d.child(
                                gpui::div()
                                    .ml(r(6.))
                                    .font_weight(weight(400.))
                                    .text_color(hsla(t.fg_tertiary))
                                    .child(detail),
                            )
                            .flex()
                            .flex_wrap()
                        }),
                )
                .children(dismiss_button);
            scale_from(toast_box, motion.scale, (0.5, 1.0))
        }));
    if animating {
        window.request_animation_frame();
    }
    stack
}

// PORT STATUS: confidence=medium todos=0
