//! Keyboard focus as WebView2 does it: buttons are tab stops, Tab and
//! Shift+Tab move focus, Enter and Space click the focused element (GPUI), and
//! `.focus-ring:focus-visible` draws a ring only after keyboard input.

use std::cell::Cell;

use gpui::{
    App, DispatchPhase, ElementId, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseDownEvent, Styled,
    Window, canvas,
};

use crate::gfx::zoomed;
use crate::theme::{Theme, fade, shadow};

thread_local! {
    /// The last input was the keyboard (`:focus-visible`).
    static KEYBOARD: Cell<bool> = const { Cell::new(false) };
    /// The view zoom of the current frame (ring widths are CSS px).
    static ZOOM: Cell<f32> = const { Cell::new(1.0) };
}

pub fn keyboard_modality() -> bool {
    KEYBOARD.with(Cell::get)
}

/// The root element is the document body: it takes focus when a press lands
/// on nothing focusable, and it carries the key listeners for the input
/// modality and Tab navigation (key listeners must sit on the focus path).
pub fn root_keys<E: InteractiveElement>(element: E, body: &FocusHandle) -> E {
    let body = body.clone();
    element
        .track_focus(&body)
        .capture_key_down(|event: &KeyDownEvent, window, _| {
            let modifiers = event.keystroke.modifiers;
            // Chromium: a key press without Ctrl, Alt or Meta makes focus visible.
            if !modifiers.control && !modifiers.alt && !modifiers.platform && !KEYBOARD.with(Cell::get) {
                KEYBOARD.with(|cell| cell.set(true));
                window.refresh();
            }
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            let modifiers = event.keystroke.modifiers;
            if event.keystroke.key != "tab" || modifiers.control || modifiers.alt || modifiers.platform {
                return;
            }
            cx.stop_propagation();
            if !body.is_focused(window) && window.focus_at_tab_end(modifiers.shift) {
                // Past the last stop, focus rests on the document body first.
                window.focus(&body);
            } else if modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
        })
}

/// The document body's focus handle.
struct Body(FocusHandle);

impl gpui::Global for Body {}

pub fn body(cx: &mut App) -> FocusHandle {
    if !cx.has_global::<Body>() {
        let handle = cx.focus_handle();
        cx.set_global(Body(handle));
    }
    cx.global::<Body>().0.clone()
}

/// The pointer side of the modality (a window listener) and the frame's zoom.
pub fn root_listeners(zoom: f32) -> impl IntoElement {
    ZOOM.with(|cell| cell.set(zoom));
    canvas(
        |_, _, _| (),
        |_, _, window, _| {
            window.on_mouse_event(|_: &MouseDownEvent, phase, window, _| {
                if phase == DispatchPhase::Capture && KEYBOARD.with(Cell::get) {
                    KEYBOARD.with(|cell| cell.set(false));
                    window.refresh();
                }
            });
        },
    )
    .absolute()
    .size_full()
}

/// `.focus-ring:focus-visible`: 1px accent at 65% and 4px accent at 18%.
fn ring(t: &Theme) -> Vec<gpui::BoxShadow> {
    // GPUI paints the list bottom-up; CSS lists the top shadow first.
    zoomed(
        vec![shadow(0., 0., 0., 4., fade(t.accent, 18.)), shadow(0., 0., 0., 1., fade(t.accent, 65.))],
        ZOOM.with(Cell::get),
    )
}

/// Draws the focus ring on a focusable element while focus is visible.
pub fn focus_ring<E: InteractiveElement>(element: E, t: &Theme) -> E {
    if keyboard_modality() {
        let ring = ring(t);
        element.focus(move |style| style.shadow(ring))
    } else {
        element
    }
}

/// A `<button>`: focusable on click, in the Tab order when `tab_stop`
/// (`tabIndex={selected ? 0 : -1}` passes `false`).
pub fn button<E: InteractiveElement>(element: E, tab_stop: bool, t: &Theme) -> E {
    focus_ring(element.tab_index(0).tab_stop(tab_stop), t)
}

/// A button with a handle the caller can focus (roving focus, restore on close).
pub fn button_with<E: InteractiveElement>(element: E, handle: &FocusHandle, tab_stop: bool, t: &Theme) -> E {
    focus_ring(element.track_focus(&handle.clone().tab_stop(tab_stop)), t)
}

/// A focus handle that lives as long as `key` renders.
pub fn keyed_handle(key: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> FocusHandle {
    window.use_keyed_state(key, cx, |_, cx| cx.focus_handle()).read(cx).clone()
}

/// Scrolls a newly focused element into view, as Blink does on keyboard focus
/// (`ScrollAlignment::CenterIfNeeded`): a hidden element centers, a partly
/// visible one aligns to the nearest edge. `last` is the focus already handled;
/// `context` is the scroll container's key context (the focus must be inside it).
pub fn reveal_focused(
    handle: &gpui::ScrollHandle,
    context: &str,
    last: &mut Option<FocusHandle>,
    window: &mut Window,
    cx: &App,
) {
    let focused = window.focused(cx);
    if focused == *last {
        return;
    }
    if !keyboard_modality() {
        *last = focused;
        return;
    }
    // Focus outside this scroll container (the search input, for example)
    // needs no reveal; never wait for its bounds.
    if !window.context_stack().iter().any(|key_context| key_context.contains(context)) {
        *last = focused;
        return;
    }
    let Some(target) = window.focused_bounds() else {
        // The new focus has not painted yet.
        window.request_animation_frame();
        return;
    };
    *last = focused;
    let view = handle.bounds();
    let offset = handle.offset();
    let max = handle.max_offset();
    if target.top() >= view.top() && target.bottom() <= view.bottom() {
        return;
    }
    let hidden = target.bottom() <= view.top() || target.top() >= view.bottom();
    let delta = if hidden {
        target.center().y - view.center().y
    } else if target.top() < view.top() {
        target.top() - view.top()
    } else {
        target.bottom() - view.bottom()
    };
    // Chromium keeps scroll offsets on whole device pixels.
    let y = (offset.y - delta).round().clamp(-max.height, gpui::px(0.));
    handle.set_offset(gpui::point(offset.x, y));
    window.refresh();
}

/// The palette's settings button (`[data-settings-trigger]`): the sheet
/// returns focus to it on close.
struct SettingsTrigger(FocusHandle);

impl gpui::Global for SettingsTrigger {}

pub fn settings_trigger(cx: &mut App) -> FocusHandle {
    if !cx.has_global::<SettingsTrigger>() {
        let handle = cx.focus_handle();
        cx.set_global(SettingsTrigger(handle));
    }
    cx.global::<SettingsTrigger>().0.clone()
}

// PORT STATUS: confidence=medium todos=0
