//! The search field (`src/features/palette/PaletteSearchInput.tsx`).

use gpui::{AnyElement, App, Entity, Focusable, KeyDownEvent, ParentElement, Styled, Window, div, prelude::*, relative};

use crate::components::text_input::{InputStyle, TextInput};
use crate::components::ui::{IconButtonProps, icon_button, kbd, weight};
use crate::gfx::{icon, r, zoomed};
use crate::theme::{Theme, fade, hsla, shadow};

/// What a key press in the search field asks the palette to do.
pub enum SearchKey {
    Move(i32),
    RunSelected,
    OpenSelectedMenu,
    Dismiss,
    /// Ctrl+K: close the palette (`prism:close`).
    Close,
    ClearQuery,
}

/// Maps a key press to a palette command (PaletteSearchInput.tsx:35-82).
pub fn search_key(event: &KeyDownEvent, composing: bool) -> Option<SearchKey> {
    if composing {
        return None;
    }
    let k = &event.keystroke;
    let ctrl = k.modifiers.control;
    if k.key == "menu" || (k.key == "f10" && k.modifiers.shift) {
        return Some(SearchKey::OpenSelectedMenu);
    }
    Some(match k.key.as_str() {
        "down" => SearchKey::Move(1),
        "up" => SearchKey::Move(-1),
        "enter" => SearchKey::RunSelected,
        "escape" => SearchKey::Dismiss,
        "k" if ctrl => SearchKey::Close,
        "w" if ctrl => SearchKey::Dismiss,
        "backspace" if ctrl => SearchKey::ClearQuery,
        _ => return None,
    })
}

pub fn input_style(t: &Theme) -> InputStyle {
    InputStyle { text: t.fg, placeholder: t.fg_quiet, selection: fade(t.accent, 40.0), caret: t.fg }
}

pub struct SearchInputProps<'a> {
    pub input: &'a Entity<TextInput>,
    pub shortcut: String,
    pub settings_open: bool,
}

/// The rendered field. Key handling is the palette's (`search_key`).
pub fn render_search_input(
    props: SearchInputProps,
    t: &Theme,
    zoom: f32,
    window: &mut Window,
    cx: &mut App,
    on_toggle_settings: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let focused = props.input.read(cx).focus_handle(cx).is_focused(window);
    props.input.update(cx, |input, _| input.style = input_style(t));
    let field_shadow = if focused { zoomed(vec![shadow(0., 4., 16., 0., fade(t.accent, 5.0))], zoom) } else { vec![] };
    div().px(r(20.)).pb(r(4.)).pt(r(20.)).child(
        div()
            .relative()
            .flex()
            .items_center()
            .gap(r(12.))
            // The CSS ring is an inset shadow (no layout); a GPUI border takes 1px.
            .px(r(15.))
            .py(r(11.))
            .rounded(r(14.))
            .bg(hsla(if focused { t.field_bg_focus } else { t.field_bg }))
            .border_1()
            .border_color(hsla(if focused { fade(t.accent, 52.0) } else { t.field_line }))
            .shadow(field_shadow)
            .child(crate::components::ui::inset_top_highlight(t.field_hi, 14.))
            // The focus tint (`.search-field:has(input:focus) > svg`) never applies in the
            // original: the `text-fg-tertiary` utility layer outranks the components layer.
            .child(icon("search", 18., t.fg_tertiary))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(r(15.))
                    .font_weight(weight(500.))
                    .line_height(relative(1.45))
                    .child(props.input.clone()),
            )
            .child(kbd(&props.shortcut, t))
            .child(icon_button(
                "settings-trigger",
                IconButtonProps { label: "Settings", active: props.settings_open, disabled: false, focus: Some(crate::components::focus::settings_trigger(cx)) },
                t,
                "settings-2",
                16.,
                on_toggle_settings,
            )),
    )
    .into_any_element()
}

// PORT STATUS: confidence=medium todos=0
