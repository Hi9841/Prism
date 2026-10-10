//! Toasts (`src/components/Toast.tsx`).

use gpui::{App, Entity, IntoElement, ParentElement, Styled, Window, div, prelude::*, relative};

use crate::components::ui::weight;
use crate::gfx::{icon, icon_stroke, r, reduced_motion, zoomed};
use crate::state::app::{AppState, ToastKind};
use crate::theme::{Rgba, Theme, fade, hsla, motion, shadow_raised, white};

/// `toast-in` (160 ms ease-out-soft) and `toast-out` (140 ms ease-in) as
/// (opacity, offset_y px). Scale is not reproduced (no transforms in GPUI).
fn motion_state(toast: &crate::state::app::Toast) -> (f32, f32, bool) {
    if reduced_motion() {
        return (if toast.closing.is_some() { 0.0 } else { 1.0 }, 0.0, false);
    }
    if let Some(start) = toast.closing {
        let t = (start.elapsed().as_millis() as f32 / 140.0).min(1.0);
        let e = motion::ease_in(t);
        return (1.0 - e, 6.0 * e, t < 1.0);
    }
    let t = (toast.born.elapsed().as_millis() as f32 / 160.0).min(1.0);
    let e = motion::ease_out_soft(t);
    (e, 10.0 * (1.0 - e), t < 1.0)
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
            let (opacity, offset, active) = motion_state(&toast);
            animating |= active;
            let is_error = toast.kind == ToastKind::Error;
            let id = toast.id;
            let app = app.clone();
            div()
                .id(("toast", toast.id as usize))
                .relative()
                .top(r(offset))
                .opacity(opacity)
                .flex()
                .items_center()
                .gap(r(10.))
                .min_h(r(44.))
                .max_w_full()
                .rounded(r(14.))
                .border_1()
                .border_color(hsla(if is_error { fade(t.danger, 35.0) } else { t.line }))
                // D4: the backdrop blur becomes a near-opaque fill.
                .bg(hsla(Rgba { a: 0.96, ..t.toast }))
                .py(r(5.))
                .pl(r(11.))
                .pr(r(if is_error { 3. } else { 15. }))
                .shadow(zoomed(shadow_raised(), zoom))
                .child(crate::components::ui::inset_top_highlight(white(0.05), 14.))
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
                .when(is_error, |d| {
                    d.child(
                        div()
                            .id(("toast-dismiss", id as usize))
                            .group("toast-dismiss")
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(r(44.))
                            .rounded(r(10.))
                            .hover(|s| s.bg(hsla(t.surface_hover)))
                            .on_click(move |_, _, cx| app.update(cx, |app, cx| app.dismiss_toast(id, cx)))
                            .child(icon("x", 14., t.fg_tertiary).group_hover("toast-dismiss", |s| s.text_color(hsla(t.fg)))),
                    )
                })
        }));
    if animating {
        window.request_animation_frame();
    }
    stack
}

// PORT STATUS: confidence=medium todos=0
