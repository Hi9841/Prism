# Patches on gpui 0.2.2 (crates.io) for prism-gpui

1. `src/style.rs`, `TextStyle::line_height_in_pixels`: no `.round()`. Chromium keeps
   fractional line heights; the rounding moved stacked text by 1 px per block
   (measured with migration/parity, settings sheet).
2. `src/text_system/line.rs`, `paint_line`: the baseline follows Blink. The font
   ascent and descent round to whole pixels; the half-leading floors to 1/64 px.
   Settings captures went from 9.53-9.69 to 9.67-9.80. (Rounding the glyph row
   instead of flooring it made every capture worse; GPUI's floor stays.)
3. `src/tab_stop.rs` `TabStopMap::is_at_end` and `src/window.rs` `Window::focus_at_tab_end`:
   tell whether Tab would wrap. Chromium moves focus to the document body before
   it wraps, and the port does the same.
4. `src/window.rs` `Window::dispatch_key_up`: the parity runner releases keys
   (GPUI clicks a focused element on Enter or Space key-up).
5. `Frame::focused_bounds` (`src/window.rs`, set in `src/elements/div.rs`) and
   `Window::focused_bounds`: where the focused element painted. Scroll
   containers use it to scroll a newly focused element into view.
