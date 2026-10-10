# Patches on gpui 0.2.2 (crates.io) for prism-gpui

1. `src/style.rs`, `TextStyle::line_height_in_pixels`: no `.round()`. Chromium keeps
   fractional line heights; the rounding moved stacked text by 1 px per block
   (measured with migration/parity, settings sheet).
