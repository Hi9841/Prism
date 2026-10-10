//! Shared primitives (`src/components/ui.tsx`): settings rows, key chips, row
//! icons, section labels, the toggle, the segmented control, and icon buttons.

use gpui::{
    AnyElement, App, ElementId, FontWeight, Hsla, IntoElement, ObjectFit, ParentElement, Rgba,
    SharedString, StyleRefinement, Styled, TextRun, Window, div, img, prelude::*, px, relative,
};

use crate::gfx::{Tween, data_url_image, icon, r, zoomed};
use crate::lib::emoji::{hue_for_name, name_to_monogram};
use crate::lib::types::PaletteIcon;
use crate::state::app::app_state;
use crate::theme::{Theme, hsla, motion, shadow, shadow_tile, white};

/// The resolved theme and the view zoom factor for this frame.
pub fn theme(cx: &App) -> (Theme, f32) {
    let app = app_state(cx).read(cx);
    (app.theme.clone(), (app.settings.view_zoom / 100.0) as f32)
}

pub fn weight(w: f32) -> FontWeight {
    FontWeight(w)
}

/// A single line of text that truncates with an ellipsis.
pub fn truncate(text: impl Into<SharedString>) -> gpui::Div {
    div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(text.into())
}

/* ---------------- Native tooltip ---------------- */

/// The `title` attribute. WebView2 shows the Windows tooltip control after the
/// hover delay, below the pointer, in system metrics (the view zoom does not apply).
/// Use with `.tooltip(title_tooltip(..))`.
pub fn title_tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let text: SharedString = text.into();
    move |_, cx| {
        let dark = theme(cx).0.mode == crate::theme::Mode::Dark;
        // Headless Chrome does not capture the native tooltip: parity captures skip it.
        let text = if crate::gfx::parity().is_some() { SharedString::default() } else { text.clone() };
        cx.new(|_| TitleTooltip { text, dark }).into()
    }
}

struct TitleTooltip {
    text: SharedString,
    dark: bool,
}

impl gpui::Render for TitleTooltip {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        // Windows 11 tooltip colors (comctl32, small rounded corners).
        let (bg, line, fg) = if self.dark {
            (gpui::rgb(0x2b2b2b), gpui::rgb(0x3d3d3d), gpui::rgb(0xffffff))
        } else {
            (gpui::rgb(0xf9f9f9), gpui::rgb(0xe5e5e5), gpui::rgb(0x1a1a1a))
        };
        if self.text.is_empty() {
            return div();
        }
        // The control sits under the cursor image (about 20 px tall).
        div().pt(px(19.)).child(
            div()
                .max_w(px(500.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(4.))
                .border_1()
                .border_color(line)
                .bg(bg)
                .shadow(vec![shadow(0., 2., 6., 0., crate::theme::black(0.18))])
                .font_family(crate::theme::FONT_SANS_FALLBACK)
                .text_size(px(12.))
                .line_height(px(16.))
                .font_weight(weight(400.))
                .text_color(fg)
                .child(self.text.clone()),
        )
    }
}

/* ---------------- Settings row ---------------- */

pub fn settings_row(title: &str, detail: Option<&str>, control: impl IntoElement, t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_x(r(24.))
        .gap_y(r(8.))
        .py(r(12.))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex_basis(r(160.))
                .child(div().text_size(r(13.)).font_weight(weight(500.)).text_color(hsla(t.fg)).child(title.to_owned()))
                .when_some(detail, |d, detail| {
                    d.child(
                        div()
                            .mt(r(2.))
                            .text_size(r(11.5))
                            .text_color(hsla(t.fg_tertiary))
                            .child(crate::components::pretty_text::pretty(detail.to_owned())),
                    )
                }),
        )
        .child(div().min_w_0().max_w_full().flex_shrink_0().child(control))
}

/* ---------------- Kbd ---------------- */

pub fn kbd(label: &str, t: &Theme) -> gpui::Div {
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(r(20.))
        .h(r(20.))
        .px(r(6.))
        .rounded(r(6.))
        .border_1()
        .border_color(hsla(t.kbd_line))
        .bg(t.kbd.background())
        .text_color(hsla(t.fg_secondary))
        .text_size(r(11.))
        .font_weight(weight(550.))
        .line_height(relative(1.45))
        .child(inset_top_highlight(t.kbd_hi, 6.))
        // Segoe UI has no U+21B5; Chromium falls back to Cambria Math (CSS.getPlatformFontsForNode).
        .when(label.contains('\u{21B5}'), |d| d.font_family("Cambria Math"))
        .child(label.to_owned())
}

/// `box-shadow: inset 0 1px 0 <color>`: a 1px highlight along the top edge.
pub fn inset_top_highlight(color: Rgba, radius: f32) -> gpui::Div {
    div().absolute().top_0().left(r(radius * 0.6)).right(r(radius * 0.6)).h(px(1.)).bg(hsla(color))
}

/* ---------------- Row icon ---------------- */

pub fn row_icon(icon_value: &PaletteIcon, size: f32, t: &Theme, zoom: f32) -> AnyElement {
    match icon_value {
        PaletteIcon::Tile { icon: lucide, tint } => tile(t.tile(tint).background(), size, zoom)
            .child(icon(&lucide_file(lucide.0), size * 19.0 / 40.0, white(0.95)))
            .into_any_element(),
        PaletteIcon::Emoji { char } => div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(r(size))
            .child(div().text_size(r(22.)).line_height(relative(1.0)).child(char.clone()))
            .into_any_element(),
        PaletteIcon::Image { src, .. } => {
            let mut frame = div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .rounded(r(10.))
                .bg(hsla(t.surface))
                .size(r(size));
            if let Some(image) = data_url_image(src) {
                frame = frame.child(img(image).size_full().object_fit(ObjectFit::Cover).rounded(r(10.)));
            }
            frame.into_any_element()
        }
        PaletteIcon::App { name, icon: src } => match src.as_deref().and_then(data_url_image) {
            Some(image) => div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(r(10.))
                .bg(hsla(t.surface))
                .size(r(size))
                .child(img(image).size(r(size - 8.)).object_fit(ObjectFit::Contain))
                .into_any_element(),
            None => monogram(name, size, t, zoom),
        },
    }
}

/// `.tile`: a gradient chip with a top highlight and the tile shadow.
fn tile(background: gpui::Background, size: f32, zoom: f32) -> gpui::Div {
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(r(size))
        .rounded(r(10.))
        .bg(background)
        .shadow(zoomed(shadow_tile(), zoom))
        .child(inset_top_highlight(white(0.14), 10.))
}

fn monogram(name: &str, size: f32, t: &Theme, zoom: f32) -> AnyElement {
    let hue = hue_for_name(name);
    tile(t.mono_tile(hue).background(), size, zoom)
        .child(
            div()
                .font_weight(weight(600.))
                .text_color(hsla(Rgba { a: 0.9, ..white(1.0) }))
                .text_size(r((size * 0.34).max(11.)))
                .line_height(relative(1.45))
                .child(name_to_monogram(name)),
        )
        .into_any_element()
}

/// The asset file of a lucide icon from its React display name.
pub fn lucide_file(display_name: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = display_name.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else if c.is_ascii_digit() {
            if i > 0 && !chars[i - 1].is_ascii_digit() {
                out.push('-');
            }
            out.push(*c);
        } else {
            out.push(*c);
        }
    }
    out
}

/* ---------------- Section label ---------------- */

pub fn section_label(label: &str, t: &Theme) -> gpui::Div {
    div()
        .px(r(14.))
        .pb(r(6.))
        .pt(r(16.))
        .text_size(r(11.))
        .font_weight(weight(600.))
        .text_color(hsla(t.fg_quiet))
        .child(label.to_uppercase())
}

/* ---------------- Toggle ---------------- */

/// A switch. `on_change` receives the next value.
pub fn toggle(
    id: impl Into<ElementId>,
    checked: bool,
    disabled: bool,
    t: &Theme,
    zoom: f32,
    window: &mut Window,
    cx: &mut App,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id: ElementId = id.into();
    let knob = window.use_keyed_state(ElementId::Name(format!("{id}-knob").into()), cx, |_, _| Tween::at(0.0));
    let track = window.use_keyed_state(ElementId::Name(format!("{id}-track").into()), cx, |_, _| Tween::at(0.0));
    let target = if checked { 1.0 } else { 0.0 };
    if knob.read(cx).target() != target {
        // First render jumps; later changes transition (transform 150ms, colors 200ms).
        let first = knob.read(cx).value() == 0.0 && track.read(cx).value() == 0.0 && checked && !knob.read(cx).is_active();
        knob.update(cx, |k, _| if first { k.jump(target) } else { k.set(target, crate::gfx::motion_ms(150), motion::ease_out_soft) });
        track.update(cx, |k, _| if first { k.jump(target) } else { k.set(target, crate::gfx::motion_ms(200), motion::ease) });
    }
    let progress = knob.read(cx).animate(window);
    let color = track.read(cx).animate(window);
    let background = blend(t.track, t.accent, color);
    let on_change = std::rc::Rc::new(on_change);
    let key_change = on_change.clone();
    let track = div()
        .id(id.clone())
        // A `<button role="switch">`: focusable, Enter and Space toggle it.
        .when(!disabled, |d| {
            crate::components::focus::button(d, true, t).on_click(move |_, window, cx| key_change(!checked, window, cx))
        })
        .relative()
        .flex_none()
        .h(r(24.))
        .w(r(40.))
        .rounded_full()
        .bg(background)
        .when(disabled, |d| d.opacity(0.45))
        .child(
            div()
                .absolute()
                .top(r(3.))
                .left(r(3. + 16. * progress))
                .size(r(18.))
                .rounded_full()
                .bg(gpui::white())
                .shadow(zoomed(vec![shadow(0., 1., 2., 0., crate::theme::black(0.05))], zoom)),
        )
        .child(
            // `after:-inset-2.5`: the hit area extends 10px past the track.
            div()
                .id(ElementId::Name(format!("{id}-hit").into()))
                .absolute()
                .top(r(-10.))
                .bottom(r(-10.))
                .left(r(-10.))
                .right(r(-10.))
                .when(!disabled, |d| {
                    d.on_click(move |_, window, cx| {
                        // The track's own click listener must not fire twice.
                        cx.stop_propagation();
                        on_change(!checked, window, cx)
                    })
                }),
        );
    // The switch is an inline-block on the row's text baseline (14px Segoe UI,
    // line-height 1.45): its line box adds the 5.3px font descent below it.
    div().flex_none().h(r(24. + 5.296875)).child(track).into_any_element()
}

/// Linear sRGB interpolation of two colors (CSS color transitions).
pub fn blend(from: Rgba, to: Rgba, t: f32) -> Hsla {
    hsla(Rgba {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    })
}

/* ---------------- Segmented ---------------- */

/// Measures one line of text at the current rem size.
pub fn text_width(text: &str, size_px: f32, font_weight: f32, window: &mut Window) -> f32 {
    let font_size = r(size_px).to_pixels(window.rem_size());
    let mut style = window.text_style();
    style.font_weight = weight(font_weight);
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().layout_line(text, font_size, &[run], None);
    f32::from(line.width) * 16.0 / f32::from(window.rem_size())
}

pub struct SegmentOption {
    pub value: SharedString,
    pub label: SharedString,
}

pub fn segmented(
    id: impl Into<ElementId>,
    value: &str,
    options: Vec<SegmentOption>,
    disabled: bool,
    t: &Theme,
    zoom: f32,
    window: &mut Window,
    cx: &mut App,
    on_change: impl Fn(SharedString, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id: ElementId = id.into();
    let active_index = options.iter().position(|o| o.value.as_ref() == value).unwrap_or(0);
    // `inline-grid` with equal `1fr` columns: every segment is as wide as the widest.
    let segment = options
        .iter()
        .map(|o| text_width(&o.label, 12., 500., window) + 24.)
        .fold(32.0f32, f32::max);
    let pill = window.use_keyed_state(ElementId::Name(format!("{id}-pill").into()), cx, |_, _| {
        Tween::at(active_index as f32)
    });
    if pill.read(cx).target() != active_index as f32 {
        pill.update(cx, |p, _| p.set(active_index as f32, crate::gfx::motion_ms(200), motion::ease_out_soft));
    }
    let position = pill.read(cx).animate(window);
    let on_change = std::rc::Rc::new(on_change);
    // Roving focus: the active option is the tab stop; arrows, Home and End
    // select and focus a neighbor (`selectWithKeyboard`).
    let handles: Vec<gpui::FocusHandle> = (0..options.len())
        .map(|index| crate::components::focus::keyed_handle(ElementId::Name(format!("{id}-focus-{index}").into()), window, cx))
        .collect();
    let values: Vec<SharedString> = options.iter().map(|o| o.value.clone()).collect();
    let key_handles = handles.clone();
    let key_change = on_change.clone();
    div()
        .id(id.clone())
        .when(!disabled, |d| {
            d.on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                let count = values.len();
                let next = match event.keystroke.key.as_str() {
                    "left" => (active_index + count - 1) % count,
                    "right" => (active_index + 1) % count,
                    "home" => 0,
                    "end" => count - 1,
                    _ => return,
                };
                cx.stop_propagation();
                key_change(values[next].clone(), window, cx);
                window.focus(&key_handles[next]);
            })
        })
        .relative()
        .flex()
        .flex_none()
        .rounded(r(10.))
        .bg(hsla(t.surface))
        .p(r(3.))
        .when(disabled, |d| d.opacity(0.55))
        .child(inset_top_highlight(white(0.04), 10.))
        .child(
            div()
                .absolute()
                .top(r(3.))
                .bottom(r(3.))
                .left(r(3. + segment * position))
                .w(r(segment))
                .rounded(r(7.))
                .bg(hsla(t.seg_active))
                .shadow(zoomed(vec![shadow(0., 1., 3., 0., crate::theme::black(0.25))], zoom))
                .child(inset_top_highlight(white(0.08), 7.)),
        )
        .children(options.into_iter().enumerate().map(|(index, option)| {
            let active = index == active_index;
            let on_change = on_change.clone();
            let value = option.value.clone();
            div()
                .id(ElementId::Name(format!("{id}-{index}").into()))
                .when(!disabled, |d| crate::components::focus::button_with(d, &handles[index], active, t))
                .relative()
                .flex()
                .justify_center()
                .w(r(segment))
                .rounded(r(7.))
                .px(r(12.))
                .py(r(5.))
                .text_size(r(12.))
                .font_weight(weight(500.))
                .line_height(relative(1.45))
                .text_color(hsla(if active { t.fg } else { t.fg_tertiary }))
                .when(!active, |d| d.hover(|s| s.text_color(hsla(t.fg_secondary))))
                .when(!disabled, |d| d.on_click(move |_, window, cx| on_change(value.clone(), window, cx)))
                .child(option.label)
        }))
        .into_any_element()
}

/* ---------------- Icon button ---------------- */

pub struct IconButtonProps {
    /// `aria-label` and `title`.
    pub label: &'static str,
    pub active: bool,
    pub disabled: bool,
    /// A handle the caller focuses (`[data-settings-trigger]`).
    pub focus: Option<gpui::FocusHandle>,
}

/// A 32px square icon button (`IconButton`) with a lucide icon.
pub fn icon_button(
    id: impl Into<ElementId>,
    props: IconButtonProps,
    t: &Theme,
    icon_name: &str,
    icon_size: f32,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let IconButtonProps { label, active, disabled, focus } = props;
    let glyph = icon(icon_name, icon_size, if active { t.fg } else { t.fg_tertiary });
    let glyph = if active || disabled {
        glyph
    } else {
        glyph.group_hover("icon-button", |s| s.text_color(hsla(t.fg_secondary)))
    };
    let button = div().id(id.into());
    // `aria-disabled` keeps the button focusable.
    let button = match &focus {
        Some(handle) => crate::components::focus::button_with(button, handle, true, t),
        None => crate::components::focus::button(button, true, t),
    };
    button
        .group("icon-button")
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(r(32.))
        .rounded(r(10.))
        .map(|d| {
            if disabled {
                d.opacity(0.4)
            } else if active {
                d.bg(hsla(t.surface_active))
            } else {
                d.hover(|s| s.bg(hsla(t.surface_hover)))
            }
        })
        .when(!disabled, |d| d.on_click(move |_, window, cx| on_click(window, cx)))
        .tooltip(title_tooltip(label))
        .child(glyph)
}

/// Applies `text-*` color to an icon child that inherits it (`currentColor`).
pub fn current_color(style: &mut StyleRefinement, color: Rgba) {
    style.text.get_or_insert_with(Default::default).color = Some(hsla(color));
}

// PORT STATUS: confidence=medium todos=0
