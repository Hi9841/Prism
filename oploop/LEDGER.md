# OpLoop ledger: Prism GPUI port

## Workspace

- Baseline: `C:\Users\hi\Prism-parity-baseline` (git worktree, detached) at `11dc8f9` (release 1.0.10).
- Work: `C:\Users\hi\Prism`, uncommitted on `main` at loop start (the 2026-10-27 parity re-check, the
  `prism-core` move into `gpui/`, and the design fixes). They go to a branch as the loop's first commit.
- Machine: AMD Ryzen 9 7950X3D (32 logical), 31.6 GB RAM, NVIDIA RTX 4090, Windows 11 Pro 26200.
- Runtime: rustc 1.98.1, Bun 1.3.14, gpui 0.2.2 vendored (`vendor/gpui`, patches 1-9).
- The spec: the original React/Tauri app at the repo root. It stays unchanged.

## Correctness floor

| Check | Command (from the repo root, with `TMP`/`TEMP` set to `target/tmp`) | Baseline result | Current result |
|---|---|---|---|
| Judge vs original | `bun migration/judge/judge.mjs` | 244/277 (33 stale goldens, see RULEBOOK 2026-10-27) | 277/277 |
| Judge vs port | `PRISM_ADAPTER_CMD=target/debug/prism-judge.exe bun migration/judge/judge.mjs` | 244/277 (same 33, identical output) | 277/277 |
| Judge sensitivity | `bun migration/judge/mutate.mjs` | 24/26 (2 patterns did not match CRLF) | 26/26 |
| Typecheck | `cargo check --workspace --all-targets` | pass | pass |
| Backend tests | removed 2026-10-27 (owner: no unit tests) | 246 tests (HANDOFF) | n/a |
| Lint (JS) | `bunx oxlint <changed files>` | n/a | 0 problems |

## Journeys

| ID | Journey | Metric | Baseline (median, range, n) | Current | Change |
|---|---|---|---|---|---|
| J1a | UI parity, 33 captures (`bun migration/parity/run.mjs`) | mean / min score of 10 | 9.71 / 9.37 (context-menu); excl. menus 9.73 | 9.902 / 9.67 (context-menu); excl. menus 9.913 / 9.74 (zoom-80) | +0.192 (+0.183 excl. menus); the zoom captures also changed with the harness fix I-024 |
| J1b | UX parity, 37 scenarios (`--ux`) | passed, UX score | 36/37 (ux-context-keys, logged), 9.73 | 37/37, 10.0 | +1 scenario |
| J2 | Open latency (warm) | ms, activation to first painted frame | original 41 (p95 61, n=16) | GPUI 14 (p95 21) | -66%, p 1.5e-6 |
| J2b | First open after start | ms, first painted frame | original 204 (200-215, n=4) | GPUI 20 (19-22) | -90% |
| J3 | Type latency, 13 queries | ms, query shown / rows complete | original 20 / 69 (n=260) | GPUI 5 / 54 | 11 of 13 queries faster beyond noise; 2 equal |
| J4 | Resources | private MiB after start / after 20 cycles; CPU s first 60 s | original 498 / 601; 20.3 s; 7 processes | GPUI 401 / 433; 11.6 s; 1 process | -20% / -28% memory, -43% CPU |
| J5 | Search rows on the real catalog, 13 queries | differences | | 0 in sections, rows, order, subtitles | presentation only: no ellipsis on long text (I-013) |

Parity scorer noise: the same build gives the same score on a rerun (deterministic fixture, parked
window, PrintWindow capture); a +-0.01 change is at the noise floor of one capture.

## Iterations

### I-001 Design fixes from the static audit (V1-V5, S2, S5-S8, highlights, kbd radius, tnum, Oklab)

- Journey: J1a
- Profile: static CSS-vs-port audit (no pixel profile).
- Hypothesis: border-as-padding errors, missing highlights, and wrong easing cost points in settings,
  toast and zoom captures. Fixing them should raise those captures.
- Result: zoom-120 9.52 -> 9.57, zoom-80 9.65 -> 9.68, settings 9.78 -> 9.80; light-search 9.57 -> 9.54,
  keyboard-deep, search-files, search-settings -0.01.
- Pixel profile of the loss: the 40 px row icon tiles. With Oklab mixing, the low channel of the tile
  gradient drops far below the original (red 51 vs 72); the port's tile end colors were already off by
  about 13 levels in sRGB mode.
- Decision: KEEP the layout fixes; the tile Oklab change goes to I-004.

Parity profile after I-001 (`python -I oploop/parity/hotspots.py target/parity/run`): mean loss per
capture 0.287 = SSIM on edges 0.152 (53%), not-close on edges 0.083 (29%), SSIM on flat fills 0.049
(17%), not-close on flat 0.003. Text and icon edges are 82% of the gap. Top repeated region: the
right-aligned path text of file rows (x 448-576), about 0.025 per row in 8 captures; its ink ends 2 px
left of the original's (568 vs 570, centroid -1.9 px).

### I-002 Text width rounds up to 1/64 px, not 1 px

- Journey: J1a
- Profile: `vendor/gpui/src/elements/text.rs:419` `size.width.max(line_size.width).ceil()`.
- Hypothesis: GPUI rounds every text box up to a whole pixel; Blink rounds text width up to a
  LayoutUnit (1/64 px). Right-aligned and centered text moves left by up to 1 px. Rounding to 1/64 px
  should move the path text about 0.5-1 px right and raise the file-row captures by about 0.01 each.
- Test: judge 277/277; parity subset search-files, light-search, idle-scrolled, keyboard-deep,
  settings, zoom-120 against the I-001 run.
- Result: search-files 9.55 -> 9.58, light-search 9.54 -> 9.56, idle-scrolled 9.62 -> 9.66,
  keyboard-deep 9.56 -> 9.61, settings 9.80 -> 9.82, zoom-120 9.57 -> 9.57 (n=1 each; the scorer is
  deterministic). The hover-transition agent edited prism-ui during the run, so the next full run
  confirms it.
- Decision: KEEP (vendored patch 10).

### I-003 Fractional layout (taffy rounding off), probe

- Journey: J1a
- Profile: `vendor/gpui/src/taffy.rs:41` `taffy.enable_rounding()`: every box, and so every text origin,
  snaps to whole pixels. Blink keeps LayoutUnit (1/64 px) positions, snaps only box painting, and draws
  text at fractional x (4 sub-pixel positions, the same as GPUI's `SUBPIXEL_VARIANTS_X`).
- Hypothesis: whole-pixel text origins cost a large part of the 0.152 SSIM edge loss. A probe with
  rounding off (no paint snapping yet) shows the size: text should gain; 1 px borders and fills can
  lose sharpness. If the net is positive, patch 11 adds Blink-style paint snapping for quads.

### I-004 Outer shadows do not paint under their element (patch 12)

- Journey: J1a
- Profile: the 40 px row tiles in light-search (rows 330-369, x 21-60): port G/B 12-20 levels darker
  than the original at every sample. A model of the CSS gradient (145deg, Oklab, alpha 0.9 over the
  measured row color) matches the original within 1 level, so the interpolation is right (Oklab) and
  the darkening comes from somewhere else: `--shadow-tile` (black 40% + 30%) paints under the 90%
  opaque tile in GPUI, while CSS paints an outer box shadow only outside the border box.
- Hypothesis: cutting the element's rounded box out of its own shadow (shader `quad_sdf`) removes the
  darkening of every translucent element with an outer shadow (tiles, kbd, toast, menus) and should
  raise the light captures with tiles by about 0.02-0.04.
- Test: judge; full UI parity run.

### I-005 Glyph x to the nearest quarter pixel (patch 13)

- Journey: J1a
- Profile: `window.rs` `paint_glyph`: `floor(fract * 4)` picks the quarter below; Skia picks the
  nearest quarter. Text sits 1/8 px left of Chromium's on average.
- Hypothesis: rounding to the nearest quarter removes the bias and should raise text-heavy captures
  (settings-scroll-*, search-*) by about 0.005-0.01 each.

### I-003 result (probe, then patch 11)

- Probe (rounding off, no snapping): mean 9.70. Fractional text gained (light-search, search-files
  +0.10, keyboard-deep +0.08) and blurry box edges lost (settings-scroll-1 -0.11, settings -0.07).
- Patch 11 (`window.rs` `snap` / `snap_mask`: quads, shadow base boxes, SVG icons, images and clip
  masks snap each edge to a device pixel, as Blink's `PixelSnappedIntRect`; text keeps fractional x):
  mean 9.71 -> 9.74 (excluding the logged menus 9.73 -> 9.75), min 9.51 -> 9.52. Up: search-files +0.12,
  light-search +0.11, keyboard-deep +0.11, idle-scrolled +0.10, search-power +0.09,
  settings-scroll-3 +0.08. Down: settings-scroll-1 -0.05, idle-bottom -0.03, settings -0.02.
- Decision: KEEP (vendored patches 10 + 11). The settings losses go to I-006 (glyph row y).
- Result: mean 9.74 -> 9.74; settings, light-search, keyboard-deep +0.01; zoom-80 -0.04 (its loss is
  the scrollbar thumb and a 1 px row offset at zoom 80, not shadows; queued as its own item). Tile G/B
  now match the original at the start and middle (G 178/150 vs 171/151); the darkening is gone.
- Decision: KEEP (vendored patch 12; CSS rule, measured neutral-to-positive).

### I-007 CSS gradient line for angled gradients (patch 15)

- Journey: J1a
- Profile: after I-004 the tile is too light at the start (G 178 vs 171) and too dark at the end (124 vs
  131). `shaders.hlsl` `gradient_color` case 1 measures t over the box height for 145deg (40 px); CSS
  uses the gradient line |w sin a| + |h cos a| = 55.7 px. 90deg and 180deg gradients are the same in
  both, so only the angled tile and monogram gradients change.
- Hypothesis: the CSS gradient line makes the tile colors match within a few levels and should raise
  every capture with tiles by about 0.01-0.03.
- Result: mean 9.74 -> 9.74 (zoom-80 +0.04 back, four captures -0.01). Tile G/B now match within 1 level
  (170/238 vs 171/238 at the start, 131/205 vs 132/205 at the end).
- Decision: KEEP (vendored patch 15; the CSS rule, neutral on the mean, removes a visible color error).

### I-008 Unclipped Oklab gradient stops (queued)

- Profile: tile red is still low (60 vs 77 at the start, 34 vs 49 in the middle). Two tile ends are
  outside sRGB (azure end linear red -0.016, amber end blue -0.017). The port clips them in
  `theme::oklch` before the Oklab mix; Chromium mixes the real Oklab values and clips each pixel.
- Plan: a gradient color-space mode that takes Oklab stops as given (no sRGB round trip).

### I-009 Text gamma and contrast against Chromium (sweep)

- Journey: J1a
- Profile: 82% of the loss is on edges; the light theme port text has 12% more ink than Chromium's
  (dark theme +2%). GPUI applies Windows Terminal's alpha correction with the system DirectWrite gamma
  and grayscale enhanced contrast; Chromium applies Skia's mask gamma.
- Hypothesis: a gamma/contrast pair exists that matches Chromium's text weight in both themes and
  raises every text-heavy capture. Patch 16 adds `PRISM_TEXT_GAMMA` / `PRISM_TEXT_CONTRAST` overrides
  for the sweep (`oploop/parity/text_sweep.sh`, removed with patch 16).

### I-018 The shader's sRGB transfer helpers were swapped (patch 18)

- Profile: `shaders.hlsl` `srgb_to_linear` was `pow(c, 1/2.2)` and `linear_to_srgb` `pow(c, 2.2)`, but
  `hsla_to_rgba` returns sRGB-encoded values and the target is plain UNORM. So `srgb_to_oklab` encoded
  instead of decoding, and every "Oklab" mix ran on gamma-encoded values (an upstream gpui bug; the
  helpers are used only by the Oklab conversions).
- Change: the CSS Color 4 piecewise transfer functions, sign-extended; signed cube roots; a final clip.
- Result: mean 9.74 -> 9.74 (six captures +0.01, five -0.01: noise level). Tile red moved from too low
  to too high (82/64/42 vs 77/49/22), as predicted while the stops are still clipped (I-008).
- Decision: KEEP (vendored patch 18: a correctness fix; I-008 completes it).
- Result (sweep, 12 settings x 6 text-heavy captures, one build): every setting scores 9.662-9.673
  (gamma 1.0-2.2, contrast 0-1.0). The alpha correction is not the lever.
- Decision: REVERT (patch 16 removed; no gain for its code).

### I-006 Glyph row y rounds, not floors (patch 14), with I-008 (exact Oklab stops, patch 17)

- Profile: settings captures: the port's text sits about 1 px below the original's. With fractional
  layout (patch 11) a line can start between pixels; Skia rounds a horizontal glyph's y, GPUI floors it.
  The tile red channel is too high after patch 18 (82/64/42 vs 77/49/22) while the stops are clipped.
- Hypothesis: rounding y raises the settings captures; exact Oklab stops put the tile red within a few
  levels. The two touch different regions (text rows, 40 px tiles), so one run measures both.
- Result: mean 9.74 -> 9.70. Rounding y: search-settings +0.06, search-command +0.04, but the settings
  group -0.12 to -0.16 (settings, light-settings, settings-scroll-*), search-files/light-search -0.09.
  The settings text was already low, and rounding moves it lower. Tiles with exact Oklab stops now match
  within 1-2 levels in every channel (78/171/239 vs 77/171/238; 51/151/222 vs 49/151/221).
- Decision: REVERT patch 14 (floor stays, as patch 2 found). KEEP patch 17 (measured alone next).
- Patch 17 alone (after reverting 14): mean 9.740 -> 9.742 (excl. menus 9.752); search-power,
  search-files, light-search, keyboard-deep, idle-scrolled +0.01, accent-amber-settings -0.01.
- Decision: KEEP patch 17 (exact Oklab stops; tiles within 1-2 levels).
- Open: some settings lines sit 0.5-1 px low in the port (band cross-correlation: settings y40, y168,
  y264, y392 +1 px; other bands 0). Not floor vs round (I-006). Suspects: the half-leading split or the
  line-height LayoutUnit rounding on fractional line boxes.

### I-010 Glyph coverage as Skia makes it (patch 19)

- Journey: J1a
- Profile: text is 82% of the loss; position fixes and gamma do not close it. GPUI rasterizes with
  `DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE` into `DWRITE_TEXTURE_ALIASED_1x1`; Skia (Chromium) asks for
  ClearType coverage (`DWRITE_TEXTURE_CLEARTYPE_3x1`) and averages the three values into its A8 mask.
- Hypothesis: the averaged ClearType coverage is horizontally smoother, as Chromium's is, and should
  raise every text-heavy capture.
- Result: mean 9.742 -> 9.752 (excl. menus 9.752 -> 9.762), min 9.52 -> 9.54. 25 captures +0.01 to +0.03
  (settings +0.03, accent-amber-settings +0.03), none down.
- Decision: KEEP (vendored patch 19).

### I-005 measured: glyph x to the nearest quarter pixel (patch 13)
- Result: mean 9.752 -> 9.757 (excl. menus 9.762 -> 9.767), min 9.54 -> 9.56.
- Decision: KEEP (vendored patch 13).

### I-011 Unhinted glyph outlines (patch 20, probe)

- Profile: GPUI uses DirectWrite's recommended grid-fit mode (hinted at UI sizes). Chromium places
  glyphs at quarter pixels and Skia draws them from unhinted outlines.
- Hypothesis: grid fitting off makes stem widths and positions match Chromium's; text-heavy captures
  should rise.
- Result: mean 9.757 -> 9.708; 13 text captures -0.01 to -0.03 and settings-scroll-3 -1.40 (probably a
  broken capture; the direction is clear without it). Hinted outlines match Chromium better.
- Decision: REVERT (patch 20 removed).

### I-012 Weight 500/600 text sits 1 px low (investigation) and the font family (patch 21)

- Evidence (probe scenario with Chromium DOM rects, CDP `CSS.getPlatformFontsForNode`, and a temporary
  port trace of line origins): the port's line tops and baselines match Chromium within 0.02 px (title
  line 179.175 vs 179.156, baseline 194.097 vs 194.078; header 31.125 / 48.0 in both). Chromium draws
  weight 500 and 600 with `Segoe-UI-Variable-Text-Semibold` and 400 with `Segoe-UI-Variable-Text`.
  Still, the port's 500/600 ink is 1.0 px lower and about 10% lighter; 400 text matches in position.
- Ruled out by measurement: optical size (Chromium output identical with `font-optical-sizing: none`),
  vertical metrics (hhea/OS2 identical; MVAR varies only x-height), rendering mode (3, 4, 5 all 1 px
  low), hinting of the instances (unhinted 600: still 1.1 px low), gamma/contrast (I-009).
- Found: GPUI never resolved "Segoe UI Variable Text" (its font-set query does not match a named-instance
  family) and fell back to "Segoe UI" for all text. Patch 21 resolves it through the WWS family model,
  as Chromium does. Pixels: 6 changed by 1 level (the two fonts render alike at this optical size).
- Decision: KEEP patch 21 (correct family, no "font not found" fallback; neutral in pixels). The 500/600
  offset stays open: what is left is Skia's rasterization and preblend against DirectWrite + GPUI's
  shader correction, which depends on the text color.

### Note: zoom-80 / zoom-120 scrollbar (harness limit, not a port defect; fixed in the harness by I-024)

- At 80% Chromium in the harness draws a 9 px gutter, a 5 px thumb and 2 px inset (11/6/2 x 0.8,
  rounded); the port keeps 11/6/2. The harness emulates view zoom with `deviceScaleFactor = zoom`,
  which scales scrollbars. The real app zooms with `getCurrentWebview().setZoom()` (page zoom), and
  the port's scrollbar was measured on the real Tauri app ("follows the device scale, not CSS zoom").
  Kept as is; the two zoom captures carry this harness difference.

### E2E baseline (2026-10-27, `oploop/results/2026-10-27-baseline.md`, `oploop/bench/`)

- GPUI is as fast or faster in every journey (table above). Search rows are equal for all 13 queries.
- I-013 (defect): long titles and paths are cut with no "..." and a long title can reach the path
  column with no gap. The original truncates with an ellipsis (`truncate`).
- I-014 (defect, fixed, not yet verified on the desktop): opened by the single-instance signal, the GPUI
  palette took the foreground in 0 of 20 opens (Tauri 20 of 20). The second instance now calls
  `AllowSetForegroundWindow(ASFW_ANY)` before it signals (`prism-app/src/main.rs`); the running
  instance cannot grant the right to itself. Verify with a real second launch in the next E2E run.

### I-013 Text truncation never applied (patch 22)

- Profile: E2E J5: long titles and paths cut at the edge with no "...", a long title reaching the path
  column. `vendor/gpui/src/elements/text.rs` caches the measured size when the wrap width matches; a
  `nowrap` text has none, so taffy's first (max-content, untruncated) measure stayed cached.
- Change: the cache also keys on the truncation width.
- Test: a fixture with two long file names (`target/tmp/probe/make_long.py`): the original and the port
  now show the same "..." cuts in title and path, side by side.
- Decision: KEEP (vendored patch 22; full parity run follows with the motion fixes).
- I-014 follow-up: Tauri took the foreground on the bench path because tao's `set_focus` sends a
  synthetic Alt key (`SendInput`) to pass the foreground lock. The port does not copy that (global input
  injection). With the fix, a user launch (Start menu, shortcut; it holds the foreground right) passes
  the right on through `AllowSetForegroundWindow(ASFW_ANY)`. The bench signals the event from a process
  without that right, so it cannot measure this path; checked by reasoning, not measured.

### I-015 Skia mask-gamma preblend for glyphs (patch 23)

- Profile: text weight depended on the text color (dark-theme near-white text -10% ink, mid-grey +11%,
  light-theme dark text +12%). GPUI used Windows Terminal's alpha correction.
- Change: glyph coverage goes through Skia's preblend (`SkTMaskGamma<3,3,3>`, sRGB gamma, contrast 1.0:
  Chromium's Windows build defaults, used when the Avalon.Graphics key has no display subkey, as here);
  A8 mean truncates as `RGBToA8`; SVG icons get linear coverage (Chromium has no mask gamma for paths).
- Result (8 text-heavy captures, same original captures): mean 9.7259 -> 9.7346, none down; dark-theme
  ink ratio 0.978 -> 0.991; mean absolute pixel error -3% to -11% in every capture.
- Decision: KEEP (vendored patch 23). Full run: see the next entry.

### Combined state after I-015 (patches 8-13, 15, 17-19, 21-23; motion layer and its review fixes)

- UI parity: mean 9.757 -> 9.766 (excl. menus 9.767 -> 9.776), min 9.56; 25 captures +0.01/+0.02, none down.
- UX parity: 37/37 passed, UX score 10.0 (the baseline had 36/37: ux-context-keys rose above 9.4).
- Judge 277/277. All 14 HLSL entry points compile with fxc 10.0.28000 (/O3).
- I-012 follow-up (after patch 23): the semibold weight now matches (ink 47186 vs 46839, 55571 vs
  55627); the offset is a pure 1-row shift (identical row profile). Blink on Windows only rounds ascent
  and descent (`AscentDescentWithHacks`; the borrow-1px hack is Linux-family only), and Chromium's canvas
  metrics are the same for 400/500/600 (13 px: 14/3, 15 px: 16/4), so the baseline is the same. Chromium
  places the Semibold instance's glyph images 1 row higher at 12, 13 and 15 px but not at 11 px; hinting
  does not change it (I-012). Open; worth about +0.005 on the mean (settings captures). No unexplained
  1 px correction added.

### E2E final (2026-10-27, `results/2026-10-27-final.md`, release build at `7d79a97`)

| Journey | Baseline (first E2E run) | Final | Original (Tauri), final run |
|---|---|---|---|
| J2 first painted frame, warm | 14 ms | 14 ms (p95 20, n=16) | 41 ms (p95 42) |
| J2 first open after start | 20 ms | 21 ms (n=4) | 208 ms |
| J3 query shown, all queries | 5 ms | 5 ms (p95 7, n=260) | 20 ms (p95 75) |
| J3 rows done, all queries | 54 ms | 54 ms (p95 110) | 68 ms (p95 130) |
| J4 private MiB 60 s after start | 401 | 385 (n=3) | 428 |
| J4 private MiB after 20 cycles | 433 | 385 (growth +0.4/-30.8/+14.0) | 498 (growth +70 to +75) |
| J4 handles / threads after start | 1662 / 255 | 895 / 103 | 3889 / 311 |
| J4 CPU s, start to 60 s | 11.6 | 9.36 | 15.41 |
| J5 rows, 13 queries | 0 differences | 0 differences | |

- No journey regressed with the rendering patches (ClearType 3x1, preblend, snapping). The memory,
  handle and thread drops against the first run come mostly from a quieter machine (the first run had
  other agents' builds), so they are not claimed as wins of this loop.
- I-016 (open, minor): GPUI's `truncate_line` cuts some long titles 1-2 characters before Chromium's
  ellipsis point.

### I-019 Half-leading floors to a whole pixel, glyph row rounds (patch 24)

- Journey: J1a
- Profile: `oploop/parity/line_shift.py` (best (dx, dy) per text line): in every settings capture,
  some lines of every weight sit exactly 1.00 px low (error 0.02-0.05 at the shift, so the same
  pixels), others 0. A zero-size inline-block probe in Chromium gives the real baselines: "Appearance"
  (13 px, line-height 18.85) has its baseline at line top + 14.000. Patch 2 put it at + 14.92: it
  floored the half-leading (0.925) to 1/64 px, but Blink's `(leading / 2).Floor()` gives an int. The
  I-012 probe compared against a baseline it computed with the same 1/64 px rule, so it did not see it.
- Hypothesis: with Blink's whole-pixel floor, the port's baselines equal Chromium's; then Skia's
  row rounding (patch 14, reverted because the baseline was low) is right too. Every text capture
  should rise; settings most.
- Result: mean 9.767 -> 9.835 (excl. menus 9.776 -> 9.845), min 9.54 -> 9.58. 32 of 33 captures up
  (settings-scroll-1 +0.21, settings-scroll-2 +0.13, light-settings +0.13, settings +0.12, idle +0.08);
  zoom-120 -0.01.
- Decision: KEEP (vendored patch 24; patch 2 corrected). Closes I-012: the "semibold 1 px low" offset
  was this baseline error (13 px and 15 px lines lose 0.9 px; 11 px lines only 0.48, so they matched).

### I-020 Scrollbar gutter is 10 px, not 11 (measured in WebView2)

- Profile: in settings-scroll-2 the right-aligned shortcut chips and their note sit exactly 1 px left
  (error 0.02); the section dividers end at x 608 in the port, 609 in the original. Headless Chrome
  155: `offsetWidth - clientWidth` of `.scroll-thin` is 10. The real Tauri build (WebView2, Edge 155,
  scale 1.0, `oploop/bench/webview_probe.py`, read-only CDP, hidden window): thin 10, normal 15. The
  port reserves 11 (`gfx.rs` `SCROLLBAR_GUTTER`, from an earlier WebView2 measurement).
- Hypothesis: a 10 px gutter puts right-aligned content and divider ends where both engines put them;
  the thumb stays at 2 px from the right edge (now 2 px on both sides, as Chromium centers it).
- Result: mean 9.835 -> 9.861 (excl. menus 9.845 -> 9.870), min 9.58. settings-scroll-2 +0.18, light-search
  +0.12, search-files +0.11, keyboard-deep +0.10, idle-scrolled +0.07 (the right-aligned file paths: the
  last 1 px of the I-001 path offset was this gutter); zoom-120 -0.01.
- Decision: KEEP (`gfx.rs` `SCROLLBAR_GUTTER` 10; the real WebView2 value, not a harness value).

### I-021 Line height as a LayoutUnit (patch 25)

- Profile: settings-scroll-3 "Group apps like Creative..." still 1.00 px low (error 0.04). Chromium's
  line tops step by 18.84375 for an 18.85 px line height (LayoutUnit floor of 1206.4/64); the port
  stepped by 18.85, about 0.006 px more per line, enough to cross a pixel after some 20 lines.
- Hypothesis: flooring the line height to 1/64 px keeps the port's line tops on Chromium's.
- Result: the first build changed only 8 captures by a few pixels: `text.rs` converts the line height
  itself and did not use `line_height_in_pixels`. With both: mean 9.861 -> 9.864 (excl. menus 9.870 ->
  9.873); settings-scroll-3 +0.08, settings-scroll-1 +0.01, none down.
- Decision: KEEP (vendored patch 25).

### I-022 Maximum scroll offset in whole pixels (patch 26)

- Profile: settings-bottom (scrolled to the end): "Group apps like Creative..." 1.00 px low and the
  next line 1.00 px high. Chromium at the end: scrollTop 1600 = scrollHeight 2073 - clientHeight 473,
  both pixel-snapped; the port's maximum was the fractional content height minus the box height.
- Hypothesis: a pixel-snapped maximum (Blink `MaximumScrollOffsetInt`) puts the end of each list
  where Chromium puts it; only the captures scrolled to the end change.
- Result: mean 9.864 -> 9.867 (excl. menus 9.873 -> 9.877); settings-bottom +0.07, idle-bottom +0.04,
  no other capture changed.
- Decision: KEEP (vendored patch 26).

### I-023 Icon tiles: the icon box does not shrink (settings-scroll-2)

- Profile: the Start-button icon tiles: label 2 px high, icon squashed. The original is a CSS grid
  (`grid h-12 place-items-center`): its content (24 + 2 + 14.5 px) is taller than the 38 px content
  box, and grid rows overflow at the bottom. The port's flex column shrank the 24 px icon box to 21.5.
- Change: `flex_shrink_0` on the icon box (`taskbar_customization.rs`).
- Result (same run as I-024; it changes only this capture): settings-scroll-2 9.87 -> 9.91.
- Decision: KEEP.

### I-024 Harness: emulate WebView2 zoom with root CSS zoom, not the device scale

- Profile: zoom-120 / zoom-80 lost most at the scrollbar (`Note: zoom-80 / zoom-120 scrollbar`). Real
  WebView2 155 (`webview_probe.py 1.2` / `0.8`, `set_webview_zoom` as the app calls it): the thin
  gutter is 8 CSS px at 120% and 12 at 80%, so 10 device px at every zoom. The harness emulated zoom
  with `deviceScaleFactor`, which scales the scrollbar (thumb 7 px at 120%, 4 px at 80%).
- Change: `run-original.mjs` sets CSS `zoom` on the root at scale 1.0. Chromium applies browser zoom as
  a root zoom factor, so layout and text are the same, and scrollbars stay at device size. Input
  coordinates are device pixels, as in the port. The captured thumb is now x 631-636 at both zooms,
  the same pixels as the port.
- Result: zoom-120 9.58 -> 9.79, zoom-80 9.67 -> 9.73 (original captures only; the port did not
  change). Mean after I-023 + I-024: 9.867 -> 9.877 (excl. menus 9.877 -> 9.887), min 9.66.
- Decision: KEEP (a measured fidelity fix of the harness, not a port change).

### I-025 Start-icon SVGs drawn at their painted size

- Profile: the Gem and Diamond icons had no anti-aliasing (binary pixels). GPUI's `img` rasterizes an
  SVG once at its intrinsic size (96 px) and samples it down to 20 px; Chromium rasterizes it at 20 px.
- Change: the two one-color icons draw as `svg()` masks in their stroke colors (`#f8fafc`, `#60a5fa`),
  as every other icon in the port.
- Result: settings-scroll-2 SSIM 0.9889 -> 0.9891 (score 9.91 both); the icon now has the original's
  anti-aliased shape. It sits 1 px right because the tile row is 1 px narrower (label widths: I-026).
- Decision: KEEP (visible fix, score neutral).

### I-026 Kerning was off for all text (patch 27)

- Profile: after I-019..I-025 every line has dy 0, but some lines sit 0.5-1.5 px right. Column profile
  of "Taskbar density": the "T" is identical, the "a" after it starts 1.5 px later in the port, and
  the rest of the line keeps that offset. HarfBuzz (Chromium's shaper) on SegUIVar.ttf at wght 600:
  "Ta" kerns -1.46 px at 13 px. GPUI sets an `IDWriteTypography` on every run; it re-adds liga, clig
  and calt (and nothing at all when the run has no features), but not `kern`, and a typography
  object replaces DirectWrite's default features.
- Hypothesis: listing `kern` puts every glyph after a kerned pair where Chromium puts it, in every
  capture with text; label widths also change, so tile rows and right-aligned text move too.
- Result: mean 9.877 -> 9.902 (excl. menus 9.887 -> 9.913), min 9.66 -> 9.67. 31 of 33 captures up
  (settings-scroll-1 and -2 +0.07, settings +0.05, search-power +0.04, idle +0.02), none down.
- Decision: KEEP (vendored patch 27).

### Incident 2026-10-27 09:28: the bench restored an old prism.json

- `probe.py gpui` (before an E2E re-check of patches 24-27) found prism.json different from the
  backup of 00:14 and copied the backup over it. The installed Prism had saved the owner's newer state
  between 08:54 and 09:28 (parity mode cannot write it: every bridge call answers from the fixture).
  The installed Prism (not restarted) keeps that state in memory and writes the full state
  (`settings`, `history`) at its next change.
- Cause: `Session` took the backup once and treated any later difference as damage.
- Fix: each session stops the installed Prism, then backs up prism.json, then starts the guardian;
  the exit restores only what a test build changed. The E2E re-check waits until the owner's Prism has
  saved again (a session stops the installed Prism, which would drop the state in its memory).

## Session 2026-10-27 (B): performance, the port against the WebView original

Owner: "we need to optimize it much more compared to the webview tauri version". The file index is in the
shared backend (both apps carry it), so its wins shrink both apps; the port-only layers (GPUI window, host,
rendering) are where the port pulls ahead. Both kinds are in scope.

Harness: `PRISM_BENCH_DB=<catalog copy> cargo test --release -p prism-core --lib live_catalog -- --ignored
--nocapture` (index load time and size, per-query search time) on a copy of the owner's catalog (5.84 M rows,
`target/tmp/bench/catalog.db`, copied with the SQLite backup API). The E2E bench (`bench/`) for J2-J5.

Profile (temporary timers, removed): index load 6.98 s = SQL fetch with owned strings 3.11 s + push 3.07 s
(of which `path_penalty` 2.55 s) + other 0.8 s. Index memory 217 MiB of the app's 385 MiB.

### P-001 `path_penalty` tests only the needles that can start at each separator

- Hot spot: `catalog/search.rs` `path_penalty`: 2.55 s of the 6.98 s load (37%), because every backslash of
  every path tests all 25 `LOCATION_PENALTIES` needles.
- Hypothesis: group the needles by their first byte after the separator (a 128-entry bitmask table built at
  compile time); a path segment then tests 0-5 needles, not 25. `path_penalty` should drop to about 0.3 s and
  the load by about 2 s (6.9 -> 4.9 s), with identical penalties.
- Test first: an equivalence test against the sliding scan over generated paths (every needle, mixed case,
  at the start, middle and end).
- Result (interleaved test binaries, same catalog copy): load A 6.12/6.64/6.64/6.66/6.70 s, B 4.36/4.95/4.95/
  5.01/5.03/5.04 s: median 6.64 -> 4.95 s (-1.69 s, -25%), every B below every A (one-sided rank-sum p about
  0.002). Index size unchanged (217 MiB). prism-core 255 passed; judge 277/277.
- Decision: KEEP.

### P-002 No driver worker threads for GPUI's Direct3D device (patch 28)

- Profile: the port's UI layer alone (`bench/uimem.py`: the release build in parity mode, fixture data, no
  file index or SQLite) holds 143.5 MiB private and 116 threads. `bench/memmap.py` (committed private memory
  by region): 92 MiB in six blocks of 4-31 MiB; a tracing global allocator (temporary) showed no Rust
  allocation of 4 MiB or more, so the blocks are native: the GPU driver and the D3D runtime.
- Hypothesis: NVIDIA's threaded optimization starts worker threads with their own command-buffer pools for
  each D3D11 device. GPUI submits from one thread, so `D3D11_CREATE_DEVICE_PREVENT_INTERNAL_THREADING_OPTIMIZATIONS`
  should remove those threads and some memory, with identical pixels.
- Result (interleaved, n=3 per block, 2 blocks each): private 144.4-146.7 -> 134.8-135.7 MiB (-10 MiB, every
  B below every A), working set 72.3 -> 66.3 MiB, threads 116 -> 49 (-67). The idle-scrolled capture is
  pixel-identical. J2/J3 latency is checked in the combined E2E A/B at the end of the session.
- Decision: KEEP (vendored patch 28), pending the E2E latency check.
- E2E check (`results/raw/20261027-111527`, gpui-head = HEAD with P-001 and P-002 against gpui-base):
  warm first frame 12 (p95 17) -> 17 (p95 27) ms, p 0.0073; settled, echo and rows done unchanged. P-002 is
  the only change in that range that touches frame presentation. The owner stopped desktop testing before
  the attribution run (HEAD with and without the flag).
- Decision, revised: REVERT. Latency outranks the thread count; the 67 driver threads sleep when idle.

### P-003 One Direct3D device, not two (reverted)

- Hypothesis: `get_adapter` creates a full device only to test support and drops it; GPUI then creates the
  real one. One device should save driver memory.
- Result: private 133.0-137.6 MiB against 135.7-136.4 MiB (ranges overlap), working set -1 MiB. The dropped
  device's memory is released; no gain beyond noise.
- Decision: REVERT.

### P-004 Each distinct file name stored once (interned name index)

- Profile: the name index holds 217 MiB of the app's about 385 MiB (both apps carry it). On the owner's
  catalog only 1,253,313 of 5,842,337 names are distinct (21%): the name text is 117.2 MiB, the distinct text
  44.0 MiB. The search scans every row's name.
- Hypothesis: one copy per distinct name (with a u32 hash table, and per-name chains of rows for the posting
  lists) should cut about 50 MiB and make the scan up to about 4.7x shorter, with identical results.
- Change (a subagent, reviewed): `catalog/name_index.rs`. Distinct names in `text`/`ends`/`masks`/`heads`;
  rows in `ids`/`flags`/`places`/`next`; an open-addressing table of name numbers. A `Hit` carries the name
  number. Appends link a row in front of its name's chain.
- Result (interleaved test binaries, 11 runs each): index 217 -> 144.4 MiB (-72.6 MiB, -33%); every query
  faster (report 7.2 -> 4.3 ms, invoice 2024 15.0 -> 6.0, rpt 8.3 -> 4.4; the slowest B run below the
  fastest A run for each query); load 4.48 -> 4.68 s (+4.5%, inside the 10% limit set before the run). The
  top 10 equals the exhaustive reference for all 12 queries in every run. prism-core 262 passed (7 new
  tests; a cut-off mutation fails one).
- Decision: KEEP.

### P-005 The index load reads each path in place (no copy)

- Profile (temporary timers on HEAD after P-004): load 5.65 s = SQL fetch 3.37 s (60%) + push 1.38 s +
  totals query 0.42 s. The fetch copied every row's `display_path` into a new `String` (644 MB of path text
  on the owner's catalog), only for three numbers: the location penalty, the depth and the length.
- Hypothesis: `NameRow` carries the three numbers, computed from the borrowed SQLite text in the row
  callback (also outside the index's write lock); one allocation and about 640 MB of copying go away. The
  load should drop by about 0.5-1 s, with the same index.
- Result (interleaved test binaries): load A 4.83/5.36/5.31/5.29 s, B 4.15/4.39/4.41/4.36 s: median 5.30 ->
  4.38 s (-0.93 s, -17%), every B below every A; index 144.4 MiB both; the live reference checks passed in
  every run. prism-core 262 passed.
- Decision: KEEP.

### Defect 2026-10-27: descenders cut in result titles (patch 30)

- Owner's report: in folder results the last pixel row of each "g" is missing ("grill-with-docs-ui",
  "grilling"). The title is `truncate` (overflow hidden) at 13.5 px with leading 1.25: a 16.875 px box under
  18 px of ascent + descent, baseline at top + 14. With the clip edge and the glyph row both rounded
  (patches 11 and 24), a row top at a fractional y of 0.5-0.625 cuts the descender's last row; the owner's
  screenshot fits only that range. Reproduced in parity mode at view zoom 80 ("grilling": the original
  keeps row 241, the port lost row 242).
- Fix: patch 30 (single-line ellipsis clips only horizontally). The zoom-80 capture now keeps the row.
