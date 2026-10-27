# Patches on gpui 0.2.2 (crates.io) for prism-gpui

1. `src/style.rs`, `TextStyle::line_height_in_pixels`: no `.round()`. Chromium keeps
   fractional line heights; the rounding moved stacked text by 1 px per block
   (measured with migration/parity, settings sheet).
2. `src/text_system/line.rs`, `paint_line`: the baseline follows Blink. The font
   ascent and descent round to whole device pixels, and the half-leading floors
   to a whole device pixel (`InlineBoxState::CalculateLeadingSpace`). Measured in
   Chromium: a 13 px line with line-height 18.85 has its baseline at line top +
   14.000, not + 14.92. Settings captures went from 9.53-9.69 to 9.67-9.80 with a
   1/64 px floor; the whole-pixel floor is patch 24.
3. `src/tab_stop.rs` `TabStopMap::is_at_end` and `src/window.rs` `Window::focus_at_tab_end`:
   tell whether Tab would wrap. Chromium moves focus to the document body before
   it wraps, and the port does the same.
4. `src/window.rs` `Window::dispatch_key_up`: the parity runner releases keys
   (GPUI clicks a focused element on Enter or Space key-up).
5. `Frame::focused_bounds` (`src/window.rs`, set in `src/elements/div.rs`) and
   `Window::focused_bounds`: where the focused element painted. Scroll
   containers use it to scroll a newly focused element into view.
6. `src/window.rs`, the `on_request_frame` callback: next-frame callbacks that
   cannot run (the window update fails because a native call re-entered the
   window procedure during an app update) stay queued for the next frame.
   Before, they were dropped, and the second palette open stayed invisible in
   the `Preparing` phase (found in the real app, 2026-10-10).
7. `src/window.rs`, `Window::paint_shadows`: a sharp ring shadow (no offset, no
   blur, spread > 0) paints as a border quad outside the element, with radii
   grown by the spread, as CSS does. Before, the shadow also filled the
   element, so the focus ring filled transparent buttons with the accent. Other
   sharp shadows keep a 0.3 px blur (the shader divides by the blur).
8. `src/scene.rs` `ElementScale`, `src/window.rs` `Window::with_element_scale`
   (all paint calls go through `Window::insert_primitive`, and `paint_layer`
   maps its bounds), `src/style.rs` `Style::scale` (applied in `Style::paint`
   around the center of the bounds) and `src/styled.rs` `Styled::scale`: CSS
   `transform: scale()` for short effects (press feedback, enter and exit,
   `hover:scale-105`). Each primitive is mapped on the CPU when it goes into the
   scene: bounds, content mask, corner radii, border widths, shadow blur,
   underline thickness, path vertices, and the SVG transformation. The mapped
   clip is cut by the clip that was active where the scale started, as the
   ancestors of a CSS transform still clip it. Text, icons and images are
   drawn as scaled sprites (the atlas sampler is linear), so a glyph is not
   rasterized again for each frame of an animation and the glyphs move
   together without jitter; text is a little soft while the scale is not 1.0.
   The atlas has no gutter, so the HLSL sprite shaders clamp each sample to the
   sprite's tile, inset by half a texel (`to_tile_rect`); before, a scaled
   sprite sampled the next tile at its edges.
   Limits: layout, hitboxes and `focused_bounds` stay unscaled; deferred draws
   (`deferred`, `anchored`) paint outside the scale; a cached view
   (`AnyView::cached`) replays the scale of its last paint. A factor of 1.0
   gives the same scene as before.
9. `src/color.rs` `radial_gradient` (`BackgroundTag::RadialGradient = 3`) and
   `src/platform/windows/shaders.hlsl` `gradient_color` `case 3`: CSS
   `radial-gradient(<rx> <ry> at <cx> <cy>, <stop>, <stop>)` with an ellipse,
   center and radii as fractions of the bounds. The four values go in the
   `solid` field, which a gradient does not use, so the `repr(C)` layout does
   not change (`Background::opacity` does not touch `solid` for this tag). Two
   stops; t = length((p - center) / radii); the colors before the first stop
   and after the last stop are the stop colors; premultiplied interpolation in
   sRGB or Oklab, as in CSS (`mix_gradient_stops`, which the linear gradient
   `case 1` also uses: a stop with alpha 0 made a dark band; an unknown color
   space mixes as sRGB). The gradient is relative to the border box (CSS
   uses the padding box: 1 px off for a 1 px border). Only the DirectX shader
   has `case 3`; the Metal and WGSL shaders do not.
10. `src/elements/text.rs`, the text measure: a text box rounds its width up to
    a LayoutUnit (1/64 px), as Blink does, not to a whole pixel. Right-aligned
    and centered text moved left by up to 1 px (file paths in result rows).
    The wrap width also rounds up to 1/64 px: with taffy rounding off, a parent
    can give a width 1 ulp below the measured width, and the last word wrapped.
    Parity: search-files 9.55 -> 9.58, keyboard-deep 9.56 -> 9.61.
11. `src/taffy.rs` (rounding off) and `src/window.rs` `snap` / `snap_mask`:
    layout keeps fractional positions, and painting snaps boxes as Blink's
    `PixelSnappedIntRect` (each edge rounds to a device pixel, .5 up as
    `floor(v + 0.5)`, not away from zero): quads, the base
    box of shadows, SVG icons, images and clip masks. Text keeps its fractional
    x, so glyphs sit where Chromium puts them. Parity mean 9.71 -> 9.74
    (search-files +0.12, keyboard-deep +0.11). A probe without the snapping
    lost up to 0.11 in captures with many 1 px borders.
12. `src/scene.rs` `Shadow::element_bounds` / `element_radii`, `src/window.rs`
    `paint_shadows`, and the HLSL `shadow_fragment`: an outer box shadow is
    cut out under the element that casts it, as CSS paints it only outside the
    border box. Before, the shadow showed through translucent elements (row
    tiles at 90% alpha were 12-20 levels too dark). The two fields are appended
    to the end of the struct. The DirectX and WGSL (`fs_shadow`) shaders have
    the fields and the cut-out; before, the WGSL struct had a 72-byte stride
    against 104 in Rust, so each shadow after the first read wrong data. Metal
    reads the struct that cbindgen makes from `scene.rs` (`build.rs`), so its
    layout is correct, but `shadow_fragment` in `shaders.metal` has no cut-out.
14. (Reverted, then kept in patch 24.) A glyph row y rounded instead of floored: alone, the
    settings captures lost up to 0.16, because the baseline was still too low (patch 2).
15. `src/platform/windows/shaders.hlsl` `gradient_color` case 1: the CSS gradient
    line. It runs through the center in the angle's direction, and its length is
    |w sin a| + |h cos a|, so the corners take the end colors. GPUI measured the
    line over the box height (40 px for a 145deg 40 px tile, not 55.7 px), so the
    end colors came too early. 90deg and 180deg gradients do not change.
16. (Reverted.) Text gamma and contrast overrides for a sweep: 12 settings scored
    within 0.01 of each other.
17. `src/color.rs` `ColorSpace::OklabValues` and the HLSL `prepare_gradient_color`:
    the stops are Oklab values (L, a, b in the Hsla fields), not sRGB colors. CSS
    mixes `oklch()` stops before it clips to sRGB, so a stop outside sRGB (the azure
    tile end has linear red -0.016) stays exact. prism-ui `Gradient::oklch` uses it.
18. `src/platform/windows/shaders.hlsl` `srgb_to_linear` / `linear_to_srgb`: the
    CSS Color 4 transfer functions, sign-extended, with signed cube roots and a
    final clip in `oklab_to_srgb`. The two helpers were swapped (and approximated
    as 2.2), so every Oklab mix ran on gamma-encoded values. They are used only by
    the Oklab conversions.
19. `src/platform/windows/direct_write.rs` `create_glyph_run_analysis`,
    `raster_bounds` and `rasterize_monochrome`: glyph coverage as Skia makes it
    on Windows. DirectWrite gives ClearType 3x1 coverage, and the three values
    average into one alpha byte (Skia's `rgb_to_a8`), not grayscale 1x1. Parity:
    25 captures +0.01 to +0.03, none down. A recommended `ALIASED` mode (a gasp
    table or embedded bitmaps, as in SimSun) becomes `NATURAL_SYMMETRIC`, as
    `OUTLINE` does: an aliased analysis has no 3x1 texture, so the glyph had
    empty bounds. Skia uses aliased rendering only for BW masks.
13. `src/window.rs` `paint_glyph`: a glyph's x goes to the nearest quarter
    pixel, as Skia does; the floor put text 1/8 px left on average. Parity mean
    +0.005 (12 captures up, none down). (Listed here after 19: measured later.)
20. (Reverted.) Unhinted glyph outlines: text captures lost 0.01-0.03.
21. `src/platform/windows/direct_write.rs` `get_font_id_from_font_collection` and
    `wws_font_face`: when the font set has no match, the WWS family model
    (`FindFamilyName`, `GetFirstMatchingFont`) resolves the family, as Chromium
    does. The font set does not match a variable font's named-instance family
    ("Segoe UI Variable Text"), so GPUI drew all text with the "Segoe UI"
    fallback and logged "not found" at each start.
22. `src/elements/text.rs` `TextLayoutInner::truncate_width`: the measure cache
    keys on the truncation width too. A `nowrap` text has no wrap width, so the
    first measure (taffy's untruncated max-content pass) stayed cached and
    `text_ellipsis` never cut: long titles and paths ran into the next column
    with no "...". Now they end in "..." as in Chromium. Each measure truncates
    a copy of the runs, as a later wider pass shapes the full text.
23. `src/platform/windows/alpha_correction.hlsl` `skia_mask_preblend`, the HLSL
    `monochrome_sprite_fragment`, `src/window.rs` `paint_glyph` (`pad: 1`) and
    `src/platform/windows/direct_write.rs` `rasterize_monochrome`: text weight as
    Chromium draws it on Windows. Glyph coverage goes through Skia's mask gamma
    table (`SkTMaskGamma_build_correcting_lut`) for the text color's luminance
    (`SkComputeLuminance`, 3 bits), with sRGB gamma and contrast 1.0 (Chromium's
    Windows build values). The A8 mean truncates, as Skia's `RGBToA8`. Icons
    (`pad: 0`) keep linear coverage, as Chromium draws SVG paths with no mask
    gamma. This replaces the Windows Terminal correction for monochrome sprites
    (color glyphs keep it). Light text was 10% too thin and dark text 12% too
    heavy. Parity (8 text captures): mean 9.726 -> 9.735, none down. The values
    that are constant for a sprite (`skia_mask_preblend_values`: the table
    luminance and its linear values) come from `monochrome_sprite_vertex`.
24. `src/text_system/line.rs` `paint_line` (patch 2, corrected) and `src/window.rs`
    `paint_glyph`: the half-leading floors to a whole device pixel, as Blink does,
    and the glyph row rounds (`floor(y + 0.5)`), as Skia does when there are no y
    variants. Patch 2 floored the half-leading to 1/64 px, so most baselines were
    up to 0.95 px low; the floor of the glyph row (patch 14 reverted) hid part of
    that. Measured with a zero-size baseline probe in Chromium: the port's
    baselines now equal Chromium's. Parity mean 9.767 -> 9.835, 32 of 33
    captures up (settings-scroll-1 +0.21), zoom-120 -0.01. This also closes the
    "weight 500/600 sits 1 px low" item (I-012): it was the baseline, not the font.
25. `src/style.rs` `TextStyle::line_height_in_pixels` and `src/elements/text.rs` (the text
    measure uses it, not its own conversion): the line height floors to 1/64 px, as Blink
    stores it in a LayoutUnit (13 px x 1.45 = 18.85 becomes 18.84375). The 0.006 px per line
    added up down the settings sheet and moved a line by 1 px at some scroll positions.
    Parity: settings-scroll-3 +0.08, none down.
26. `src/elements/div.rs` `Interactivity::clamp_scroll_position`: the maximum scroll offset is
    in whole device pixels, the pixel-snapped contents size minus the pixel-snapped client size,
    as Blink computes it. With the fractional maximum, a list scrolled to its end stopped about
    0.1 px short of Chromium's, and lines near a pixel edge moved 1 px. Parity: settings-bottom
    +0.07, idle-bottom +0.04, none down.
