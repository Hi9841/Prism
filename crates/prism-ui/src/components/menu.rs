//! The shared look of the popover menus (power options, result actions).

use gpui::{App, Div, ElementId, ParentElement, Stateful, Styled, Window, div, prelude::*, relative};

use crate::components::ui::weight;
use crate::gfx::{icon, r, zoomed};
use crate::theme::{Rgba, Theme, hsla, shadow_pop};
use crate::transition::{self, Timing};

/// One item row, in CSS px.
pub const ITEM_HEIGHT: f32 = 32.0;
/// The surface padding, top plus bottom.
pub const PADDING: f32 = 8.0;
/// A separator line with its vertical margins.
pub const SEPARATOR_HEIGHT: f32 = 9.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// The default action: semibold, like the bold verb in Explorer menus.
    Primary,
    /// A destructive action: red icon and a red highlight.
    Danger,
}

/// The menu surface. The caller adds the id, position and children.
pub fn surface(t: &Theme, zoom: f32) -> Div {
    div()
        .overflow_hidden()
        .rounded(r(10.))
        .border_1()
        .border_color(hsla(t.line))
        // D4: the backdrop blur becomes an opaque fill.
        .bg(hsla(Rgba { a: 1.0, ..t.bg_raised }))
        .p(r(PADDING / 2.))
        .shadow(zoomed(shadow_pop(t), zoom))
}

/// One item row. Hover and keyboard focus share the one `highlighted`
/// state, so only one row lights up at a time. Colors, opacity, and the
/// press scale move like `.press` (150ms `--ease-out-soft`).
pub fn item(
    id: impl Into<ElementId>,
    glyph: &str,
    label: &'static str,
    tone: Tone,
    highlighted: bool,
    disabled: bool,
    t: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let (icon_color, text_color, fill) = match (tone, highlighted && !disabled) {
        (Tone::Danger, true) => (t.danger, t.danger, t.danger_soft),
        (Tone::Danger, false) => (t.danger, t.fg_secondary, Rgba { a: 0.0, ..t.danger_soft }),
        (_, true) => (t.fg_secondary, t.fg, t.surface_hover),
        (Tone::Primary, false) => (t.fg_tertiary, t.fg, Rgba { a: 0.0, ..t.surface_hover }),
        (Tone::Normal, false) => (t.fg_tertiary, t.fg_secondary, Rgba { a: 0.0, ..t.surface_hover }),
    };
    let id: ElementId = id.into();
    let icon_color = transition::color(&id, "icon", icon_color, Timing::PRESS, window, cx);
    let text_color = transition::color(&id, "text", text_color, Timing::PRESS, window, cx);
    let fill = transition::color(&id, "bg", fill, Timing::PRESS, window, cx);
    let opacity = transition::number(&id, "opacity", if disabled { 0.5 } else { 1.0 }, Timing::PRESS, window, cx);
    transition::press(div().id(id.clone()), &id, !disabled, window, cx)
        .flex()
        .items_center()
        .gap(r(10.))
        .h(r(ITEM_HEIGHT))
        .w_full()
        .px(r(8.))
        .rounded(r(6.))
        .text_size(r(12.5))
        .font_weight(weight(if tone == Tone::Primary { 600. } else { 500. }))
        .line_height(relative(1.45))
        .text_color(hsla(text_color))
        .bg(hsla(fill))
        .opacity(opacity)
        .child(icon(glyph, 15., icon_color))
        .child(label)
}

/// A thin line between item groups.
pub fn separator(t: &Theme) -> Div {
    div().h_px().my(r(4.)).mx(r(6.)).bg(hsla(t.divider))
}
