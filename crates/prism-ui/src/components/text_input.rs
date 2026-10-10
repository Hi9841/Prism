//! A single-line text field with the behavior of a WebView2 `<input type="text">`:
//! caret, selection, mouse selection, clipboard, word movement, IME composition,
//! and horizontal scrolling. Adapted from gpui 0.2.2 `examples/input.rs`.
//! Port infrastructure: the original used the browser's input element.

use std::ops::Range;
use std::time::Duration;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding, KeyDownEvent, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Rgba, ShapedLine, SharedString, Style,
    Task, TextRun, Timer, UTF16Selection, UnderlineStyle, Window, actions, div, fill, point, prelude::*, px, relative,
    size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::hsla;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        DeleteWordRight,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Paste,
        Cut,
        Copy,
    ]
);

const CONTEXT: &str = "TextInput";

/// Registers the editing keys once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("shift-backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("ctrl-delete", DeleteWordRight, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("ctrl-left", WordLeft, Some(CONTEXT)),
        KeyBinding::new("ctrl-right", WordRight, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectHome, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectEnd, Some(CONTEXT)),
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        KeyBinding::new("shift-insert", Paste, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", Copy, Some(CONTEXT)),
        KeyBinding::new("ctrl-insert", Copy, Some(CONTEXT)),
        KeyBinding::new("ctrl-x", Cut, Some(CONTEXT)),
        KeyBinding::new("shift-delete", Cut, Some(CONTEXT)),
    ]);
}

/// The text changed through user editing.
pub struct Changed(pub SharedString);

/// How the field paints itself; the owner sets it each render.
#[derive(Clone, Copy, Debug)]
pub struct InputStyle {
    pub text: Rgba,
    pub placeholder: Rgba,
    pub selection: Rgba,
    pub caret: Rgba,
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    max_len: Option<usize>,
    /// Ctrl+Backspace clears the field (the palette search) instead of deleting a word.
    pub ctrl_backspace_clears: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_x: Pixels,
    is_selecting: bool,
    caret_visible: bool,
    blink: Option<Task<()>>,
    pub style: InputStyle,
}

impl EventEmitter<Changed> for TextInput {}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, style: InputStyle, cx: &mut Context<Self>) -> Self {
        // An `<input>` is a tab stop.
        let focus_handle = cx.focus_handle().tab_stop(true);
        Self {
            focus_handle,
            content: SharedString::default(),
            placeholder: placeholder.into(),
            max_len: None,
            ctrl_backspace_clears: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll_x: px(0.),
            is_selecting: false,
            caret_visible: true,
            blink: None,
            style,
        }
    }

    pub fn with_max_len(mut self, max: usize) -> Self {
        self.max_len = Some(max);
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    /// Replaces the text without emitting `Changed` (a controlled `value`).
    /// The caret moves to the end when the text actually changes.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        if text == self.content {
            return;
        }
        self.content = text;
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.restart_blink(cx);
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    /// Caret to the end (`setSelectionRange(len, len)`).
    pub fn move_to_end(&mut self, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn emit_change(&mut self, cx: &mut Context<Self>) {
        self.restart_blink(cx);
        cx.emit(Changed(self.content.clone()));
        cx.notify();
    }

    /// Chromium blinks the caret every 500 ms and shows it solid right after input.
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.caret_visible = true;
        self.blink = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(500)).await;
                let alive = this
                    .update(cx, |this, cx| {
                        this.caret_visible = !this.caret_visible;
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    return;
                }
            }
        }));
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor_offset()), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word(self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word_right(&mut self, _: &DeleteWordRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_word(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    /// Ctrl+Backspace: the owner decides (palette search clears; other fields delete a word).
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.key == "backspace" && keystroke.modifiers.control && !keystroke.modifiers.alt {
            if self.ctrl_backspace_clears {
                return; // the palette's key handler clears the query
            }
            cx.stop_propagation();
            if self.selected_range.is_empty() {
                self.select_to(self.previous_word(self.cursor_offset()), cx);
            }
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let index = self.index_for_mouse_position(event.position);
        if event.click_count >= 3 {
            self.move_to(0, cx);
            self.select_to(self.content.len(), cx);
        } else if event.click_count == 2 {
            let (start, end) = self.word_range_at(index);
            self.move_to(start, cx);
            self.select_to(end, cx);
        } else if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // An <input> strips line breaks from pasted text.
            let single_line: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
            self.replace_text_in_range(None, &single_line, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.restart_blink(cx);
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed { self.selected_range.start } else { self.selected_range.end }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left() + self.scroll_x)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.restart_blink(cx);
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).rev().find_map(|(idx, _)| (idx < offset).then_some(idx)).unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    /// The start of the word before `offset` (Chromium skips whitespace, then the word).
    fn previous_word(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .filter(|(idx, word)| *idx < offset && !word.trim().is_empty())
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    /// The end of the word after `offset`.
    fn next_word(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .find(|(idx, word)| idx + word.len() > offset && !word.trim().is_empty())
            .map(|(idx, word)| idx + word.len())
            .unwrap_or(self.content.len())
    }

    fn word_range_at(&self, offset: usize) -> (usize, usize) {
        self.content
            .split_word_bound_indices()
            .find(|(idx, word)| offset >= *idx && offset <= idx + word.len())
            .map(|(idx, word)| (idx, idx + word.len()))
            .unwrap_or((offset, offset))
    }

    fn clamp_to_max(&self, range: &Range<usize>, new_text: &str) -> String {
        let Some(max) = self.max_len else {
            return new_text.to_owned();
        };
        let kept = self.content.len() - (range.end - range.start);
        let kept_units = self.content[..range.start].encode_utf16().count()
            + self.content[range.end..].encode_utf16().count();
        let _ = kept;
        let mut room = max.saturating_sub(kept_units);
        let mut out = String::new();
        for ch in new_text.chars() {
            let units = ch.len_utf16();
            if units > room {
                break;
            }
            room -= units;
            out.push(ch);
        }
        out
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected_range), reversed: self.selection_reversed })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let new_text = self.clamp_to_max(&range, new_text);
        let next: SharedString =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..]).into();
        let changed = next != self.content;
        self.content = next;
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
        if changed {
            self.emit_change(cx);
        } else {
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content = (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.marked_range = if new_text.is_empty() { None } else { Some(range.start..range.start + new_text.len()) };
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        // Composition text is not a committed value: no `Changed` until commit
        // (the original ignores input while `isComposing`).
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(bounds.left() + last_layout.x_for_index(range.start) - self.scroll_x, bounds.top()),
            point(bounds.left() + last_layout.x_for_index(range.end) - self.scroll_x, bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(line_point.x + self.scroll_x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_x: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let text_style = window.text_style();
        let style = input.style;

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(style.placeholder))
        } else {
            (content, hsla(style.text))
        };

        let run = TextRun {
            len: display_text.len(),
            font: text_style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun { len: marked_range.start, ..run.clone() },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false }),
                    ..run.clone()
                },
                TextRun { len: display_text.len() - marked_range.end, ..run },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(display_text, font_size, &runs, None);

        // Keep the caret inside the field, as an <input> scrolls its text.
        let caret_unit = (window.rem_size() / 16.0).max(px(1.0));
        let cursor_pos = if input.content.is_empty() { px(0.) } else { line.x_for_index(cursor) };
        let width = bounds.size.width;
        let mut scroll_x = input.scroll_x;
        if cursor_pos - scroll_x > width - caret_unit {
            scroll_x = cursor_pos - width + caret_unit;
        }
        if cursor_pos < scroll_x {
            scroll_x = cursor_pos;
        }
        let max_scroll = (line.width - width + caret_unit).max(px(0.));
        scroll_x = scroll_x.clamp(px(0.), max_scroll);

        let origin_x = bounds.left() - scroll_x;
        let (selection, cursor) = if selected_range.is_empty() || input.content.is_empty() {
            let caret_height = font_size * 1.2;
            let caret_top = bounds.top() + (bounds.size.height - caret_height) / 2.0;
            (
                None,
                Some(fill(
                    Bounds::new(point(origin_x + cursor_pos, caret_top), size(caret_unit, caret_height)),
                    hsla(style.caret),
                )),
            )
        } else {
            let selection_height = font_size * 1.2;
            let selection_top = bounds.top() + (bounds.size.height - selection_height) / 2.0;
            (
                Some(fill(
                    Bounds::from_corners(
                        point(origin_x + line.x_for_index(selected_range.start), selection_top),
                        point(origin_x + line.x_for_index(selected_range.end), selection_top + selection_height),
                    ),
                    hsla(style.selection),
                )),
                None,
            )
        };
        PrepaintState { line: Some(line), cursor, selection, scroll_x }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus_handle, caret_visible) = {
            let input = self.input.read(cx);
            (input.focus_handle.clone(), input.caret_visible)
        };
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        let scroll_x = prepaint.scroll_x;
        let line = prepaint.line.take().expect("shaped line");
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection)
            }
            let origin = point(bounds.left() - scroll_x, bounds.top());
            let _ = line.paint(origin, bounds.size.height, window, cx);
            if focus_handle.is_focused(window)
                && caret_visible
                && crate::gfx::parity().is_none()
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            input.scroll_x = scroll_x;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// PORT STATUS: confidence=medium todos=0
