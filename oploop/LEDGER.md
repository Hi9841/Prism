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

## Session 2026-10-28 (C): file search and indexing

Owner: "i want to work on the searching and indexing". Offline only (no desktop sessions): the harness runs
the shared backend on a copy of the owner's catalog.

- Catalog copy: `target/tmp/bench/catalog.db` (SQLite backup API, 2026-10-28): 5,850,699 `files` rows, 4.80
  GiB (C:\ 5.06 M, D:\ 1,181, E:\ 20; all on the directory backend, no NTFS rows). About 880 bytes a row.
- Harness: `crates/prism-core/examples/catalog_bench.rs` (index load, then 26 queries: the J3 queries, a
  key-by-key "package", the earlier catalog queries; the first run of each and the median of 7) and
  `oploop/bench/catalog_ab.py` (A B B A interleaving, private bytes at the MARK points, and the result
  lists of every run compared against the first A run).
- Cold cache: opening the file with `FILE_FLAG_NO_BUFFERING` purges it from the Windows file cache
  (`target/tmp/purge.py`), so a cold load (the first load after a reboot) is repeatable without admin rights.
- Baseline binary: `target/oploop/bench-base.exe` (HEAD 4b0903a4 + the new harness). Git branch and
  worktree commands were denied in this session, so the work stays uncommitted on `main` for the owner.
- Build: blaze 0.1.0 fails on this machine (ra-jit cannot build build scripts: inline assembly in std's
  `abort`). The owner approved cargo for this session. Builds use `CARGO_TARGET_DIR=target/oploop-search`
  (another session edits `prism-ui` in the same checkout).
- Disk: Samsung 970 EVO Plus NVMe. A cold sequential read of the whole catalog file takes 2.1 s.

### Journeys

| ID | Journey | Metric | Baseline (median, range, n) |
|---|---|---|---|
| S1 | Cold index load: first full-quality file search after a reboot | `load_ms` after a cache purge | 62.2 / 63.4 / 63.6 / 65.6 s (n=4; one more run took 180 s, cause unknown) |
| S2 | Warm index load: app restart | `load_ms` | 4.28 s (4.19-4.73, n=8, A/A) |
| S3 | Keystroke file search, 26 queries | median of 7 per query, summed; first run summed | 101 ms (98-118); first 155 ms (150-174) (n=8) |
| S4 | Index memory | private bytes after load minus before | 156.2 MiB (155.6-156.7, n=8) |

A/A noise (`results/raw/search-aa.jsonl`, 4 runs a side): load +-4%, per query up to +-13% (`ch`), the
summed search +-1%; no p below 0.11.

Profile S1 (temporary timers, removed), cold / warm: `file_name_totals` 5.5 / 0.36 s, row fetch
(`file_names_after`) 56.1-57.3 / 3.3 s, push about 0.5 s. The fetch scans the `files` table in id order;
its pages lie between the index pages, so SQLite reads one 4 KiB page at a time (about 85 MB/s). The disk
reads the same file in order at 2.3 GB/s.

### S-001 Read the catalog file in order while the index loads

- Hot spot: `db.rs` `file_names_after` during `NameIndex::load`: 56 s of the 62 s cold load (90%), because
  the table scan reads scattered 4 KiB pages one at a time.
- Hypothesis: a background thread that reads the whole catalog file in order (large reads through the
  file cache) while the load runs turns the scattered page reads into cache hits. The cold load should drop
  from about 62 s to about 7 s (2.1 s of reading, overlapped with the 4.2 s warm load). The warm load should
  not change (the thread copies cached pages on another core).
- Test: the load is unchanged code; the A/B compares index memory and the 26 result lists of every run.
- Result, first form (a thread that read the file in 4 MiB blocks, `FILE_FLAG_SEQUENTIAL_SCAN`): cold load
  59.1-62.9 s -> 5.8-6.6 s (n=4), but warm load 4.23 -> 4.36 s (+3.0%, p 0.02): on a warm cache the thread
  still copied 4.8 GB. Not kept in that form.
- Second form: map the file read-only and call `PrefetchVirtualMemory` on the whole view, on a thread
  beside the load; unmap when the call returns (cold 1.66 s, warm 0.19 s: cached pages are not read or
  copied, and nothing joins the working set). A mapped view blocks a truncation (incremental vacuum), so it
  stays mapped only for the call.
- Result (`results/raw/s001b-*.jsonl`): cold load 62.6 s (61.3-64.8) -> 5.75 s (5.63-5.87), -90.8%, every
  B below every A, p 0.03 (n=4); warm load 4.17 -> 4.09 s (-1.8%, p 0.17); index memory, first and warm
  search times unchanged; all 26 result lists identical in every run. Typecheck pass; judge 277/277.
- Decision: KEEP (uncommitted: git commands are denied in this session).

### S-002 Read the index rows on several connections at once

- Profile (after S-001, warm): load 4.1 s = `file_name_totals` 0.36 s + row fetch 3.3 s + push about 0.5 s.
  The fetch is one thread that decodes every row of the 5.85 M-row table (SQLite record decode, a
  `String` per name, `path_penalty`).
- Hypothesis: split the ids into fixed spans read by up to 8 threads, each with its own read-only
  connection; the loader pushes the spans in id order (one bounded channel per thread keeps the order and
  holds at most two spans per thread). The fetch should drop from 3.3 s to about 0.5 s and the warm load
  from 4.1 s to about 1.3 s; the cold load should drop too (more reads in flight). Index identical.
- Test: index memory and the 26 result lists in every A/B run must equal the baseline's.
- Change: `Database::file_names_in_spans` (one `sync_channel(1)` per thread, spans dealt round-robin, the
  caller takes them in span order), `file_id_bounds`, `open_bulk_reader` (read-only, 2 MB page cache, no
  mmap); `NameIndex::load_spans` (spans of 65,536 ids, up to 8 threads) before the existing catch-up loop.
- Result (`results/raw/s002-*.jsonl`, A = S-001b): warm load 4.12 s (3.98-4.16) -> 1.80 s (1.75-1.83),
  -56%, p 0.005 (n=6); cold load 5.65 -> 3.42 s, -40%, p 0.03 (n=4). Index memory 156.3 -> 151.6 MiB (the
  main reader's page cache no longer fills during the load). Summed search 94.9 -> 95.0 ms (p 1.0); four
  single queries 0.1-0.15 ms slower (apply_rose... 0.78 -> 0.90 ms, package* +4-5%, p < 0.05), probably
  the cold main-reader page cache. All result lists identical. Typecheck, clippy (changed files) pass;
  judge 277/277.
- Decision: KEEP.

Profile after S-002 (temporary timers, removed), warm: load 1.76 s = `file_name_totals` 0.36 s (serial,
before the spans) + spans 1.40 s (the loader pushes for 0.55 s of it and waits for decoded spans the
rest). Span phase by thread count: 1: 3.65 s, 2: 2.41, 4: 1.66, 8: 1.35, 12: 1.57, 16: 1.61, 24: 1.69.

### S-003 probes: why the span decode stops scaling at 8 threads (rejected)

- mimalloc as the global allocator (bench only): 8 threads 1.19 s against 1.20 s. The heap is not the limit.
- A 2 GiB memory map for the bulk readers (SQLite's limit): 8 threads 1.35 -> 1.20 s (-0.15 s, n=1 each).
  Remaining suspect: Windows page-cache copies and page faults in one process share kernel locks.
- Decision: REVERT both. The map gains about 9% of the warm load, but on a catalog below 2 GiB its view
  covers the whole file, and a mapped view makes an incremental-vacuum truncation fail during the load.

### S-004 Count the names on several connections

- Hot spot: `db.rs` `file_name_totals`: 0.36 s of the 1.76 s warm load (20%), one thread scanning the
  5.85 M-entry name index (`idx_files_lower_name`) before the spans start.
- Hypothesis: split the scan into 8 `lower_name` ranges on bulk-reader connections, in parallel. It
  should take about 0.06 s, and the warm load should drop from about 1.76 s to about 1.45 s, with the same
  totals (so the same buffer sizes and the same index memory).
- Test: index memory and the 26 result lists equal in every A/B run.
- Result (n=2 per probe, timers): totals 0.36 -> 0.17 s with identical totals (5,850,699 rows, 123,903,886
  bytes), but the span phase after it 1.40 -> 1.84 s, with or without the prefetch thread; load 1.76 ->
  2.00 s. The S-002 binary, run between the probes, still loads in 1.72-1.77 s. Cause not found.
- Decision: REVERT.

### S3 profile: keystroke search needs no change

Warm phase medians, summed over the 26 queries (temporary timers, removed): name scan 51.5 ms, by-id
fetch 13.4 ms, scoring 7.1 ms, disk-existence checks 23.1 ms (about 20 us a path): about 3.6 ms a
keystroke. The slowest are the typo-pass queries (`crhome` 6.1 ms, `invoice 2024` 6.4 ms with no result).
The palette waits 35 ms (`FILE_SEARCH_DELAY_MS`) before each file search, so the best case here saves 1-2
ms of about 55 ms to painted rows. Closed: not worth the code.

### Sweep journey (S5)

| ID | Journey | Metric | Baseline |
|---|---|---|---|
| S5 | Idle sweep of `C:\Users\hi\Desktop` (936,665 entries, 155k folders) into the catalog copy | wall, kernel, user; bytes read | 14.1-14.3 s wall = 10.6 s kernel + 3.75 s user; 1.98 GiB read; 0 written (n=3) |

Harness: `crates/prism-core/examples/catalog_sweep.rs` (`reconcile_scope` on one root; reads the real
disk, writes only the copy). The first run applied the changes since the copy (28.8 s, 66 MiB written).
Scaled to C:\ (5.06 M entries): about 77 s of CPU and 10.7 GB of page reads a sweep, at startup when the
last sweep is older than 24 h and every 6 h.

Profile (temporary timers, removed): `prune_removed_children` 2.2 s, `filter_changed` 2.1 s, insert 0;
the rest (about 10 s) is the walk: `FindFirstFileExW`/`FindNextFileW` and the per-entry strings.

Read-only audit (subagent, Opus): ranked hypotheses H1-H7 (one query per directory for filter and prune;
the sweep holds the reader mutex that keystroke searches need; one watcher transaction per removed path; a
directory rename runs a case-insensitive `LIKE` over the whole table; the upsert rewrites unchanged index
columns; 256 MiB mmap; 8-10 allocations an entry), schema options (integer parent and volume keys, no
`display_path`; about 2 GB smaller; owner decision), and five suspected correctness defects to confirm on
the copy (D1 trailing backslash in `parent` after a watcher rename; D2 `SUBSTR` counts characters but gets
byte offsets; D3 A->B->C rename in one flush; D4 case-only folder rename leaves stale `parent`; D5 new
junctions walked by scope repairs).

### S-005 One catalog query per folder for the change filter and the prune

- Hot spot: `db.rs` `filter_changed` (2.1 s) + `prune_removed_children` (2.2 s): 4.3 s of the 14.2 s idle
  sweep (30%). Each entry's row is looked up twice: by `(volume_id, normalized_path)` in batches of 500,
  then again through `(volume_id, parent)` per folder.
- Hypothesis: per folder, one query through `(volume_id, parent)` returns path, mtime, size and the folder
  flag; it serves both the change filter and the prune. The sweep should drop by about 2 s (14.2 -> about
  12.2 s) and read about half the page bytes (1.98 -> about 1 GiB). A row whose `parent` text is stale is
  then not "unchanged", so it is written again with the right parent (a fix for the D1/D4 class).
- Test first: `catalog_sweep --verify` (catalog rows under the root against a fresh walk of the disk) on a
  test tree in `target/tmp` that the driver changes between sweeps (add, modify, delete files and a folder);
  it must report 0 differences on the baseline and on the candidate.
- Floor first (`oploop/bench/sweep_check.py`: a test tree in `target/tmp`, sweep + `--verify` after each
  step): baseline passes steps 1-3 and fails step 4: after a case-only folder rename, a deleted child stays
  in the catalog (`EXTRA ...\gamma case\file-05.txt`): the audit's D4, confirmed. (Two harness artifacts
  fixed on the way: a folder's write time from its parent's index lags, so `--verify` ignores folder write
  times; a scope repair cannot remove its own deleted root, so that step was dropped.)
- Change: `Database::children_of` (one query per folder: path, write time, size, folder flag) and
  `remove_rows` replace `filter_changed` and `prune_removed_children`; the walk takes each entry out of the
  map, writes only entries that differ, and removes the rows left over (not after a stalled enumeration).
- Result: `sweep_check` PASS on all 4 steps, twice (D4 fixed: a row with a stale `parent` text is not in
  the folder's map, so it is written again with the current parent). Idle Desktop sweep
  (`results/raw/s005-sweep.jsonl`, A B B A x2): 14.3 s (14.0-14.6) -> 11.7 s (11.5-12.0), -18%, every B below
  every A (n=4); page reads 1,987 -> 924 MiB (-53%); user 3.7-4.8 -> 2.2-2.5 s; kernel 9.5-10.3 -> 9.1-9.5 s.
  The first B run wrote 44 MiB (rows with a stale parent, rewritten once). Typecheck, clippy pass; judge
  277/277.
- Decision: KEEP.

### Defect D1 confirmed on the catalog copy (not fixed: correctness work outside this loop)

- 3,931 of 5,856,417 rows have a `parent` with a trailing backslash (`db.rs` `rename_file` builds the
  children's parent as `<new folder>\`). Sources: `C:\Users\hi\source` 2,986, `Desktop` 848, `.vscode` 45,
  `node_modules` 41.
- After the S-005 sweep of Desktop, all 848 Desktop rows are still there, and none exists on disk: they lie
  under cargo `incremental\...\s-...` folders that the watcher saw renamed and that were deleted later.
  Their folder has no row, so no sweep walks or prunes them: ghost rows forever (search hides them with the
  existence check, but they hold index memory and candidate slots).
- Fix needs: the rename prefixes without the trailing separator (and D2: `SUBSTR` with character, not byte,
  offsets), plus a one-time removal of the ghost rows. Owner decision.

### S-006 A folder rename finds its descendants by range, not by `LIKE`

- Hot spot: `db.rs` `rename_file`: `WHERE volume_id = ? AND normalized_path LIKE ? || '%'`. `LIKE` is
  case-insensitive, so SQLite scans every key of the volume in the unique index (5.06 M on C:\) for each
  folder rename, under the writer lock: 1.01 s for a 40k-row folder and the same for a small one; the range
  `>= 'old\' AND < 'old]'` (as `delete_path_rows` uses) takes 5.7 ms (`target/tmp/like.py`, read-only).
  Cargo renames `incremental\...\s-*` folders in every build on this machine.
- Harness: `catalog_sweep --rename <old> <new>` applies a rename that happened on disk as the watcher does,
  then `--verify`; `sweep_check.py` step 5 renames `alpha`, `José`, `deep`. On the S-005 build each rename
  takes 1.15-1.32 s and leaves 9 / 31 / 32 differences: D1 (direct children get `parent` = `new\`) and D2
  (under `José`, `SUBSTR` gets byte offsets but counts characters: the children's paths lose a character).
  Both are the floor; S-006 must leave the same output, line for line.
- Hypothesis: the range finds the same rows (every `normalized_path` is stored lowercase, and the old
  prefix is lowercase), so a rename should drop from about 1.2 s to a few ms.
- Change: `rename_file` selects the descendants with `normalized_path >= 'old\' AND < 'old]'`; the
  unused `escape_like_pattern` goes.
- Result: `sweep_check.py` output identical to the S-005 build's (write times masked): steps 1-4 pass,
  the same 72 rename differences (D1/D2 floor). A 200-file folder rename on the copy, interleaved n=4:
  1,235-1,582 ms -> 1-2 ms. Typecheck, clippy pass; judge 277/277.
- Decision: KEEP.
- Harness notes: a root with a trailing backslash (`...\tmp\`) does not match the stored `parent` text, so
  such a run re-inserts rows instead of pruning them; a rename onto a path that still has rows fails with a
  UNIQUE error, and the watcher ignores the error (`if let Ok`), so the catalog keeps the old paths.

### Session C summary (2026-10-28)

Final A/B, baseline (`bench-base.exe`) against the working tree (`bench-final.exe`), interleaved; the machine
was busier than at the start (other work running), so both sides are slower than in the per-iteration runs:

| Journey | Baseline | Now | Change | n | Noise (A/A) |
|---|---|---|---|---|---|
| S1 cold index load | 77.7 s (73.2-86.1) | 4.12 s (3.94-4.26) | -95% | 4 | one 180 s outlier seen at baseline |
| S2 warm index load | 4.69 s (4.61-4.99) | 2.43 s (2.13-2.72) | -48% | 6 | +-4% |
| S3 keystroke search, 26 queries | 125 ms | 124 ms | 0 (not worked on) | 6 | +-1% |
| S4 index memory | 157.0 MiB | 151.1 MiB | -3.8% | 6 | +-0.5 MiB |
| S5 idle Desktop sweep | 14.3 s; 1.99 GiB read | 11.7 s; 0.92 GiB read | -18%; -53% | 4 | 14.0-14.6 s |
| S6 folder rename (watcher) | 1.24-1.58 s | 1-2 ms | -99.9% | 4 | |

Kept (uncommitted; git branch commands were denied): S-001 (prefetch with `PrefetchVirtualMemory`), S-002
(span load on 8 connections), S-005 (one query per folder in the sweep; fixes D4), S-006 (rename by key
range). Reverted: S-001 first form (read loop), S-003 probes (mimalloc, 2 GiB mmap), S-004 (parallel totals).

Correctness floor: judge 277/277 before and after; typecheck pass; `sweep_check.py` 3/4 steps -> 4/4 (D4);
rename floor (D1/D2) unchanged at 72 differences.

Open (owner decisions): D1/D2 fixes and a one-time removal of the 3,931 rows with a trailing-backslash
parent; schema slimming (integer parent and volume keys, no `display_path`: about 2 GB of the 4.8 GiB);
the 35 ms file-search debounce.

Next hypotheses: (1) the sweep walk with relative directory handles (`NtQueryDirectoryFile` from the parent
handle; the walk is 7.6 s kernel of 11.7 s); (2) one writer transaction per watcher flush, not per removed
path (audit H3; needs a watcher harness); (3) the upsert sets only the columns that can change (audit H5;
fewer index pages written per changed row).

## Session 2026-10-28 (C), continued

Owner: "continue to work on it". Same rules: offline, uncommitted, no schema change and no D1/D2 fix
without the owner.

### S-007 probe: where the walk's kernel time goes

Probe (temporary example, removed): Desktop walk three ways, warm cache, no EcoQoS, 3 rounds each:
A `FindFirstFileExW` with a full path per folder (today): wall 9.45-9.50 s, kernel 8.72-8.73 s, user
0.67-0.72 s; B `NtOpenFile` relative to the parent handle + `NtQueryDirectoryFile` (64 KiB buffer): wall
7.76-8.01 s, kernel 7.36-7.70 s, user 0.23-0.34 s; C as B with a full path: wall 7.73-8.06 s, kernel
7.56-7.72 s. The relative open gains nothing (B = C); the `FindFirstFile`/`FindNextFile` layer costs
about 1.1 s kernel + 0.4 s user. The rest (about 48 us a folder) is the file system and its filter
drivers.

### S-007 Read each folder with `GetFileInformationByHandleEx`

- Hot spot: `scanner.rs` `scan_volume_inner`, `FindFirstFileExW`/`FindNextFileW`: about 1.5 s of CPU of the
  11.7 s idle Desktop sweep over the direct reads (probe above).
- Hypothesis: open each folder with `CreateFileW` (`FILE_FLAG_BACKUP_SEMANTICS`) and read its entries with
  `GetFileInformationByHandleEx(FileFullDirectoryInfo)` into a 64 KiB buffer (the documented Win32 form of
  probe C; the reparse tag is in `EaSize`, as in `dwReserved0` today). The sweep should drop by about 1.2-1.5 s
  (11.7 -> about 10.3 s), with the same rows.
- Test: `sweep_check.py` (steps 1-4 pass, the same rename floor) and an idle Desktop sweep that writes 0 bytes.
- Change: `scanner.rs` `read_folder` (`CreateFileW` + `GetFileInformationByHandleEx(FileFullDirectoryInfo)`,
  one reused 64 KiB buffer; the stall detection moves into it; an error other than `ERROR_NO_MORE_FILES` now
  leaves the listing incomplete, so its rows are not pruned) and `win32_long_path` replace the
  `FindFirstFileExW` loop, `format_win32_search_pattern` and `wide_filename_to_string`.
- Result: `sweep_check.py` output identical to the S-005 build's (steps 1-4 pass, rename floor 72). The
  first idle Desktop sweep wrote 0 bytes (same write times and sizes as `FindFirstFile`). Idle Desktop sweep
  (`results/raw/s007-sweep.jsonl`, A = S-006, 4 x A B B A, n=8 each; the machine was busier than in S-005):
  13.16 s (13.00-13.80) -> 12.37 s (12.24-13.01), -6.0%, exact one-sided p 0.0002; kernel 10.77 -> 10.02 s
  (-7.0%); user unchanged. Smaller than the probe's 1.5 s: the sweep runs under EcoQoS, and the per-folder
  file-system cost is the same. Typecheck, clippy pass; judge 277/277.
- Decision: KEEP.

### S-008 The watcher removes each flush's paths in one transaction

- Hot spot: `watcher.rs` `apply_queued_events` calls `db.remove_file` once per removed path: one writer
  transaction each, so each commit writes the table and index pages again to the WAL.
- Probe (temporary example, removed; 2,000 files in a test tree swept into the copy, then removed, 3 rounds):
  one transaction per path 139.5-140.6 ms and 38.6-40.6 MiB written; one transaction (`remove_rows`)
  15.0-15.1 ms and 0.5 MiB. About 20 KB of WAL per removed file today: a `cargo clean` or a deleted
  `node_modules` of 100k files writes about 2 GB.
- Hypothesis: one `remove_rows` call per flush (same `delete_path_rows` per path, one transaction) cuts a
  flush's removal time about 9x and its WAL bytes about 75x, with the same rows removed.
- Test: the probe's removed-row count must be 2,000 in both forms; `sweep_check.py` unchanged. A flush that
  fails now rolls back all its removals (before: only the failed path); the next sweep removes them either way.
- Change: `apply_queued_events` passes all removed paths to `Database::remove_rows` (now returns the row
  count) in one call; `remove_file` stays for the rename fallback.
- Result: the probe removes 2,000 rows in both forms; 139.6 ms / 38.6 MiB -> 15.1 ms / 0.5 MiB per 2,000
  removed files. `sweep_check.py` PASS, rename floor 72. Typecheck, clippy pass; judge 277/277.
- Decision: KEEP.

### S-009 Write a changed row without touching its index columns

- Hot spot: `db.rs` `insert_batch` (sweep writes and every watcher flush of modified files): the upsert's
  `DO UPDATE SET` lists `lower_name`, `parent`, `is_directory`; SQLite then rewrites the entries of
  `idx_files_lower_name` and `idx_files_vol_parent` even when the values are equal.
- Probe (`target/tmp/h5.py`, a 200k-row SQLite test file, 2,000 updated rows, twice): `SET mtime` only:
  5.6 MiB WAL; with the index columns set to their own values: 12.7-12.9 MiB (2.3x).
- Hypothesis: first `UPDATE` only the non-index columns where the key, the `parent` text and the kind are
  unchanged; run the full upsert only for rows that statement did not match (new rows, a stale `parent`, a
  kind change). The WAL bytes of a sweep over modified files should drop by about half; new rows cost one
  extra key lookup.
- Test: `sweep_check.py` (the stale-parent step 4 must still pass) and an A/B on a test tree whose files are
  all modified between sweeps (bytes written, time).
- First form: an `UPDATE` of the non-index columns, then the full upsert when it matched no row. Modified
  files (5,000 per sweep, `target/tmp/modify_ab.py`, A B B A x3): 1.9 -> 1.2 MiB written, 35.5 -> 23 ms; but new
  files (20,000 per sweep, `target/tmp/insert_ab.py`): 198.5 -> 218 ms (+10%: one failed lookup per new row).
  Not kept in that form.
- Second form: one upsert whose `DO UPDATE` sets only the non-index columns, `WHERE` the name, parent text
  and kind are equal; a conflict it does not apply (0 rows) goes to the full upsert. New rows: one statement.
- Result (A = S-008, n=6 each, interleaved): modified files 4.5 -> 1.2 MiB written (median), 56 -> 26 ms; new
  files 200 ms (191-209) -> 138 ms (128-151) and 18.6 -> 10.7 MiB, every B below every A. The new rows are
  complete in the table and in all three indexes (`target/tmp/indexcheck.py`: 20,100 rows per tree in each);
  why the plain inserts write less was not found. `sweep_check.py` output identical to S-005's (step 4, the
  stale-parent repair, passes). Typecheck, clippy pass; judge 277/277.
- Decision: KEEP.
- Fixture: 241,457 rows of my test trees deleted from the copy (a scope sweep does not write its own root
  folder, so no later sweep prunes them); the copy holds 5,856,564 rows.

### Session C, continued: summary

Final idle Desktop sweep, original scanner (`sweep-base.exe`) against the working tree (`sweep-s009.exe`),
A B B A x2 (`results/raw/final-sweep.jsonl`; the machine was busier than at the S5 baseline): 16.3 s
(16.1-17.0) -> 12.4 s (12.1-12.7), -24%; kernel -18%, user -40%, page reads 1,987 -> 924 MiB; every B below
every A.

| Journey | Session start | Now | Change |
|---|---|---|---|
| S1 cold index load | 77.7 s | 4.1 s | -95% |
| S2 warm index load | 4.69 s | 2.43 s | -48% |
| S3 keystroke search | 125 ms / 26 queries | 124 ms | 0 (closed: 3.6 ms a key behind a 35 ms debounce) |
| S4 index memory | 157 MiB | 151 MiB | -4% |
| S5 idle Desktop sweep | 16.3 s; 1.99 GiB read | 12.4 s; 0.92 GiB read | -24%; -53% |
| S6 watcher folder rename | 1.24-1.58 s | 1-2 ms | -99.9% |
| S7 watcher removal of 2,000 files | 140 ms; 38.6 MiB WAL | 15 ms; 0.5 MiB | -89%; -99% |
| S8 sweep of 5,000 modified files | 4.5 MiB written; 56 ms | 1.2 MiB; 26 ms | -73%; -54% |
| S8b sweep of 20,000 new files | 200 ms; 18.6 MiB | 138 ms; 10.7 MiB | -31%; -42% |

Kept: S-001, S-002, S-005, S-006, S-007, S-008, S-009 (uncommitted). Reverted: S-001 first form, S-003
probes, S-004, S-009 first form.

Diminishing returns without a schema change: the load's span decode is bound by Windows cache copies (three
probes), the sweep is 75% Windows folder enumeration, and its database part reads one table row per child
(a covering `(volume_id, parent, ...)` index would remove those reads: schema, owner decision).

### Defects D1 and D2 fixed (2026-10-28, owner: "continue improving")

- `rename_file` replaces the old folder path in each descendant's path, display path and parent without a
  trailing separator (a direct child's parent becomes `new`, not `new\`: D1), and passes `SUBSTR` a
  character count (D2: a byte count cut characters under non-ASCII folders such as `José`).
- Result: `sweep_check.py` rename step 72 -> 0 differences; the rename step now counts toward PASS/FAIL (the
  S-009 build fails it with 72). Typecheck, clippy pass; judge 277/277.
- Not done (owner decision): a one-time removal of the 3,931 rows that already have a trailing-backslash
  parent. A migration runs inside `Database::open` and would need an unindexed scan at startup.

### UI: opaque surfaces and theme-aware shadows (outside the loop; RULEBOOK deviation 2026-10-28)

Parity captures (port only, `prism --parity`, scenarios restored from git history into `target/tmp/parity`):
dark shell over a white window 34,36,38 -> 11,13,16 (same as over a dark one); settings-panel ghosting 5
levels -> 0; light tile shadows at 40%; light text steps 13.9/7.4/5.7/5.2 -> 16.0/10.5/7.2/5.6:1.

## Session 2026-10-29 (D): app launch

Owner report: "apps take a long time to open sometimes"; the palette goes at once, the app comes late.
Owner approved desktop runs that launch and close real apps (also Discord and Spotify).

### Journeys

| ID | Journey | Harness | Metric |
|---|---|---|---|
| L1 | Enter on an app row to the app window, real Prism | `python oploop/bench/launch_e2e.py <query> <match> [reps] [--kill IMAGE]` (Win key, type, Enter with SendInput) | `hiddenMs` palette hidden, `windowMs` new app window visible, `focusMs` the app window has the foreground |
| L2 | `apps::launch` alone, same thread path as the palette | `cargo run --release -p prism-core --example launch_bench -- <apps.json> <name> <match> [reps]` | `callMs` launch call, `windowMs` app window visible |

### Correctness floor

`cargo check --workspace --all-targets`, `cargo clippy -p prism-core -p prism-ui -p prism-app`, judge
(`cargo build -p prism-judge`, `bun migration/judge/judge.mjs`). No unit tests (owner rule).

### Baseline (installed 1.0.14, 2026-10-29)

| App (launch path) | n | hiddenMs | windowMs | focusMs |
|---|---|---|---|---|
| WezTerm (exe, `ShellExecuteW`) | 11 | 5-8 | 349-405 | = windowMs |
| Spotify (exe, killed between reps) | 8 | 8 (40 first) | 249-276 (1069 cold) | = windowMs |
| Display settings (`ms-settings:`) | 3 | 211-274 | 151-419 | = windowMs |
| Calculator (packaged, `ActivateApplication`) | 6 | 221-357 | 306-455 | **never** |

L2 matches L1 for exe apps (WezTerm call 5 ms, window 351-408 ms; 7-Zip 56 ms): for an exe, Prism adds
about 8 ms. Prism's own CPU in the 3 s after Enter: 0-140 ms; the watcher writes 3-5 MiB while Spotify
starts.

Foreground trace of a Calculator launch: prism 0 ms -> previous window (WezTerm) 221 ms (the palette hides
when `ActivateApplication` returns) -> Calculator frame appears 307 ms -> the previous window keeps the
foreground. Packaged apps open without focus, often behind the window the user came from.

### L-001 Packaged apps get the right to take the foreground

Hypothesis: `apps.rs` `activate_packaged_app` calls `ActivateApplication` without
`CoAllowSetForegroundWindow` on the activation manager, so the activated app cannot take the foreground,
and the palette hide gives it back to the previous window. 125 of the owner's 412 apps take this path.
Calling `CoAllowSetForegroundWindow(&manager)` first (the documented pattern) should make `focusMs` equal
`windowMs` (about 300 ms instead of never).

Result (`launch_ab.py`, 2 blocks x 4 reps, A = HEAD, B = candidate): Calculator `focusMs` never (0/8) ->
304-315 ms, equal to `windowMs` (8/8); `hiddenMs` 219-241 -> 221-227, `windowMs` 302-327 -> 304-315
(unchanged). **Kept** (0f161ce9, branch `oploop-launch`).

Checked, no defect: Discord and Spotify (exe) take the foreground when their window shows; Discord in the
tray comes to the front 126 ms after Enter (3/3). `dedupe_apps` prefers the Start Menu alias, so of the 107
desktop-app AUMID entries only 15 run through the AUMID path (File Explorer, Control Panel, Run, Docker
Desktop, MSYS2 shells). File Explorer through the palette: foreground at 179-211 ms.

### L-002 Desktop app ids open through the Apps folder in-process

Hypothesis: for an AUMID without `!` (a desktop app id), `ActivateApplication` always fails (L2: call
24-36 ms for `org.wezfurlong.wezterm`), and the fallback starts a new `explorer.exe` that hands the item to
the shell (WezTerm window 797-960 ms by this path against 349-408 ms by its exe). Skipping the activation
for ids without `!` and opening `shell:AppsFolder\<id>` with `ShellExecuteW` in Prism's process should
save about 25 ms (activation) plus the `explorer.exe` start, about 100 ms or more.

Result, in-process form (L2, `bench-A` = HEAD, `bench-B` = candidate): File Explorer `windowMs` 493-559 ->
299-346 ms warm (first call in a process 470-480 ms, call 175-192 ms), Control Panel 415-452 -> 263 ms.
**Reverted**: MSYS2 is uninstalled but its Start Menu shortcuts remain, and opening `shell:AppsFolder\C:\msys64\ucrt64.exe`
in Prism's process blocked the launch thread: first with a modal "Problem with Shortcut" box (15.5 s, then
more than 9 min), and with `SEE_MASK_FLAG_NO_UI` still more than 120 s with no box (the shell searches for
the missing target). `explorer.exe` keeps that search out of Prism. The diff is in
`target/tmp/launch/l002-inprocess.diff`.

### L-002a Desktop app ids skip only the activation

Same first half: `aumid.contains('!') && activate_packaged_app(..)`, then `explorer.exe` as before.
L2, File Explorer, 3 interleaved blocks x 4 reps: call 23.6 (22.9-34.3) -> 1.9 (1.8-3.9) ms, p 3e-5;
`windowMs` 481 (459-499) -> 467 (430-535) ms, p 0.07 (not significant). The palette hides 22 ms sooner for
the 15 AUMID-only apps. **Kept** (41056d70). Gate: `cargo check --workspace --all-targets` pass; clippy
prism-core 3 warnings (floor 3); judge 277/277.

### Session D summary

| Journey | Baseline | Now | Change | n |
|---|---|---|---|---|
| L1 Calculator (packaged), Enter to app has focus | never (0/8) | 304-315 ms (8/8) | fixed | 8 + 8 |
| L2 File Explorer (desktop app id), launch call | 23.6 ms (22.9-34.3) | 1.9 ms (1.8-3.9) | -92%, p 3e-5 | 12 + 12 |
| L1 exe apps (WezTerm, Spotify, Discord), Enter to window | 249-405 ms, focus at window | not changed | Prism adds 5-8 ms | 22 |

Kept: L-001 (0f161ce9), L-002a (41056d70). Reverted: L-002 in-process form.

Open, for the owner:
- Packaged apps keep the palette on screen 220-360 ms after Enter: `ActivateApplication` returns after the
  app is activated, and the palette hides only then (the order gives the app Prism's foreground right).
  Hiding before the call changes the UI timing: a taste decision.
- `shell_open` (`ShellExecuteW`, error UI on) also opens `.lnk`/`.url` locations in Prism's process. A
  shortcut with a missing target shows a modal "Problem with Shortcut" box on the launch thread, and the
  launch gate holds until the box closes. The owner's index has no such entry today (the MSYS2
  entries are AUMID-only and go through `explorer.exe`).
- The first `ShellExecuteW` in a new Prism process costs 25-115 ms more (shell DLL load).

### Incident 2026-10-29 ~14:20-14:58: the launch bench ended Explorer

`launch_bench.rs` ended any process that outlived its closed window by 5 s. File Explorer and Control Panel
windows belong to the shell (`explorer.exe`), so the L-002 runs ended the owner's Explorer; Windows restarted
it for a time and then stopped. Prism 1.0.15 then started (update, 14:55) with no Explorer, gave up the
Win-key bridge after its 120 s startup retries, and showed "Win-key mode disabled". Explorer was started
again by hand at 14:58.

- Bench fix: `close` ends only a process created after the launch began.
- Product fix: the startup shortcut retries wait for Explorer for as long as it is missing; only attempts made
  while Explorer runs count toward the 480 (`backend.rs` `apply_startup_shortcut`).

## Session 2026-11-09 (E): search, app performance, and resource use

Owner: "optimize the search and the overall app performance and resource usage". Offline only (no desktop
sessions): parity mode for the UI layer, examples on a catalog copy for the backend, and read-only samples
of the installed app for the field profile.

- Work: branch `oploop-perf` from `9b591f3f` (release 1.0.20). Baseline binaries: `target/oploop/e/base/`
  (`prism.exe`, `catalog_bench.exe`, release profile, built at `9b591f3f`).
- Catalog copy: `target/tmp/bench/data/catalog.db` (SQLite backup API, 2026-11-09, 4.72 GiB), with copies of
  `apps.json`, `prism.json`, `intent-cache.db` and `icons/`.
- Build: cargo (blaze fails on this machine, see session C). `TMP`/`TEMP` = `target/tmp`.

### Field profile (installed 1.0.20, read-only samples, `bench/threads.py`, `bench/memmap.py`)

After 31 minutes: 420 MiB private, 392 MiB working set, 134 threads, 1,080 handles, 51.6 s CPU, 4.3 GB read,
204 MB written. Committed private memory by region: 380.6 MiB in 212 allocations (five blocks of 16 MiB or
more: 141.6 MiB; 22 blocks of 4-16 MiB: 204.6 MiB).

CPU by thread since the start: mouse hook thread (`audio_hook.rs`, `WH_MOUSE_LL`) 15.0 s (14.3 s kernel), main
11.5 s, `VSyncProvider` 5.6 s, 103 NVIDIA driver threads 3.2 s, `prism-flush` 1.8 s.

A sample of 393 s, hidden, while cargo builds ran on the machine (`target/tmp/bench/fieldwatch-1.jsonl`): 6.59 s
CPU (1.7% of one core) = scope repairs on the tokio blocking pool 3.0 s + `prism-flush` 1.6 s + `VSyncProvider`
0.76 s + mouse hook 0.34 s (all of it in the 26 s with cursor movement: 13 ms/s) + other 0.9 s. Writes: 174 MiB
in 76,094 operations (writes in 248 of the 393 s). Thread count: 133 at rest, **647** at the peak.

### Correctness floor (at `9b591f3f`)

| Check | Command | Baseline |
|---|---|---|
| Typecheck | `cargo check --workspace --all-targets` | pass |
| Lint | `cargo clippy -p prism-core -p prism-ui -p prism-app` | prism-core 10, prism-ui 24, prism-app 1 warnings |
| Judge | `cargo build -p prism-judge`, `bun migration/judge/judge.mjs` | 277/277 |
| Parity journeys | `uv run .agents/skills/verify-prism/scripts/verify.py run <scenario>` | see each iteration |

No unit tests (owner rule): each iteration names its end-to-end check.

### Journeys

| ID | Journey | Harness | Metric | Baseline (median, range, n) |
|---|---|---|---|---|
| E1 | UI layer at rest, palette hidden | `uv run oploop/bench/uiidle.py --a <exe>` (parity mode, no backend) | context switches per second (a count), CPU ms per second (cycle counters) | 300 /s (224-334); 2.22 ms/s (1.67-2.67), n=5 |
| E1b | UI layer at rest, palette open, no input | same | same | 1,366 /s (881-1,418); 10.07 ms/s (6.37-10.24), n=5 |

More journeys follow below as their harnesses are built (file churn, keystroke search, memory).

### E-001 The vsync thread waits while no window is visible (patch 33)

- Journey: E1.
- Profile: `vendor/gpui/src/platform/windows/platform.rs:240` `begin_vsync_thread`: the loop calls `DwmFlush`
  and `RedrawWindow` for each window on each vertical blank (144 Hz), also while the palette is hidden. In the
  E1 baseline the `VSyncProvider` thread uses 1.98 of the 2.22 ms/s (89%) and causes almost all of the 300
  context switches per second. In the field: 5.6 s of CPU in 31 minutes, 0.76 s in the 393 s sample.
- Hypothesis: a hidden window gets no `WM_PAINT`, so the loop does no useful work then. If the thread waits
  on a signal while no window is visible (set by `WM_SHOWWINDOW` and by `WM_WINDOWPOSCHANGED` with
  `SWP_SHOWWINDOW`; 1 s timeout as a safety net), E1 should drop from 300 to fewer than 10 context switches
  per second and from 2.2 to about 0.3 ms/s. The first frame after a show does not change: GPUI draws it
  inside `WM_SHOWWINDOW`. E1b should not change.
- Test: `uiidle.py` checks the capture after the hidden period (the palette must paint again), and the
  parity journeys that hide and reopen must pass.
- Change: `vsync.rs` `notify_window_shown` / `take_window_shown` / `wait_for_window_shown` (a flag and a
  condition variable); `platform.rs` `begin_vsync_thread` waits there when no window is visible and the 1 s
  grace after the last show is over; `events.rs` signals from `WM_SHOWWINDOW` (show) and from
  `WM_WINDOWPOSCHANGED` with `SWP_SHOWWINDOW` (`palette_window::raise` shows that way).
- Result (`results/raw/e001-uiidle.jsonl`, A B B A x3, n=6 a side; the machine was busier than at the
  baseline, so both sides are higher): hidden CPU 4.46 ms/s (4.15-4.87) -> 0.61 ms/s (0.50-0.65), -86%,
  every B below every A; context switches 262 /s (233-307) -> 67 /s (61-70), -74%. `VSyncProvider` 3.89 ->
  0.014 ms/s. What is left (0.6 ms/s, about 60 switches a second) is the NVIDIA driver's own threads
  (`nvwgf2umx.dll`). E1b (open, no input): 16.3 (14.8-17.2) -> 15.4 (14.5-16.8) ms/s, ranges overlap.
- Did the work happen: the capture 120 ms after "open, type `note`" that follows the hidden period shows the
  query and the Notepad row, and is the same image in all 8 runs of a second A/B (4 a side).
- Gate: typecheck pass; judge 277/277; parity journeys 23/23 (`target/verify/runs/20261109-235236`).
- Not checked offline: the real shell's show path (`shell.rs` `present_native`). It calls the same
  `ShowWindow(SW_SHOW)`, which sends both signals. A desktop smoke run (`bench/probe.py`) before a release
  covers it.
- Decision: KEEP.

### Journey E3: the catalog while files change (`churn_bench`, `bench/churn_ab.py`)

Harness: `crates/prism-core/examples/churn_bench.rs` opens the catalog copy, registers a test tree
(`target/tmp/bench/churn-tree`) as a volume of its own through the new `FileIndex::sync_volume` (the real
watcher, flusher and scope repairs; the host reports a hidden palette), and starts a child process that
changes files: 100 documents and 20 folders; a build of 150 crates (fingerprints, outputs, a temp folder that
lives 20 ms, a session folder of 20 files that is renamed when the crate is done); a second build of 50
crates; 100 safe saves (a temp file replaces the document), 100 downloads (a rename at the end), 20 folders
that are replaced by a renamed folder; an install of 1,200 packages of 24 files with no pause; a clean that
removes 1,000 packages and the fingerprints. About 80,000 file events in 25 s; 9,300 entries stay. The
child's own file writes are not in the numbers. The window ends 30 s after the child. A second kind of child
compares the catalog rows of the tree with the disk once a second (path, folder flag, parent).

| ID | Metric | Baseline (median, range, n=3) |
|---|---|---|
| E3 | CPU of the process | 4.13 s (3.47-4.41) |
| E3 | bytes written (catalog WAL) | 128.8 MiB (127.3-131.9) in 52,862 operations |
| E3 | threads at the peak (7 before the churn) | 373 (352-375) |
| E3 | private bytes at the peak (187 MiB after the window) | 1,084 MiB (248-1,180) |
| E3 | catalog rows that differ from the disk at the end | 1,220 / 1,212 / 1,220: never equal (an error in each run) |

The first form of the generator (no documents, `results/raw/e-churn-base.jsonl`, n=4) gave CPU 3.79 s
(3.53-4.22), 119.8 MiB (114.3-124.5), 364 threads (351-371), and 991-1,000 differences.

### Defect found by E3: the watcher loses files when a folder is renamed (the floor)

Every baseline run ends with about 1,220 entries of the disk missing from the catalog, for good (only the
6-hour sweep finds them): the 1,000 files of the second build's session folders, the 100 saved documents,
and the files of the replaced folders. Cause, `watcher.rs` `apply_queued_events`: one flush folds its events
into three unordered groups and applies them as renames, then additions, then removals.

- A file added in `a\`, then `a` renamed to `b` in the same flush: the addition still names `a\file`, which
  no longer exists, so it is dropped. The first build hides this: the parent folder is new too, and a new
  folder gets a repair walk.
- A safe save (`Removed(doc)`, `Renamed(tmp -> doc)`): the rename writes the row, and the removal, applied
  last, deletes it.

### E-002 The watcher's fold follows renames (fix)

- Journey: E3 (correctness floor).
- Hypothesis: if a rename moves the pending additions and removals under the old name to the new name, and
  cancels a pending removal of its target (removals below the target go before the renames), then the
  catalog equals the disk a few seconds after the churn ends: differences about 1,220 -> 0 in every run.
  The 1,220 rows are then written, so the WAL bytes should rise by a few percent. A rename that fails
  (a row already has the new path) asks for a repair walk of the target's parent, where it was ignored before.
- Test: `churn_bench` (differences at the end must be 0; it exits with code 1 if not).
- Change, `watcher.rs`: `EventFold` (ordered sets). `rename(from, to)` moves the pending additions and
  removals below `from` to below `to`, cancels a pending removal of `to`, sends removals below `to` ahead of
  the renames, and joins `a -> b`, `b -> c` into `a -> c`. A rename that fails asks for a walk of the
  target's parent.
- First result (`results/raw/e002-churn.jsonl`, n=4 a side): differences 1,204-1,222 -> 1, 0, 0, 40. The
  runs with differences lost one whole session folder: an earlier flush read its files after the rename,
  under the old name, and dropped them as missing; the rename came in the next flush and moved no rows.
- Second form: a renamed folder also gets a repair walk (as a new folder does).
- Result (`results/raw/e002b-churn.jsonl`, A B B A x3, n=6 a side): differences 1,203-1,220 -> 0 in all
  6 runs; the catalog equals the disk 1.06-1.6 s after the churn ends (before: never). Bytes written 130.6
  MiB (126.3-133.6) -> 98.1 MiB (91.7-100.2), -25%, p 0.005: the rows come in the flush's one transaction,
  not from many small repair walks. Read operations -69%. User CPU 1.37 -> 1.74 s (+27%, p 0.005); CPU total
  4.41 s (3.70-5.27) -> 5.25 s (3.89-5.64), +19%, p 0.23 (not significant). The fix does work that the
  baseline skipped (it lost those rows). Threads at the peak 312 -> 310 (no change).
- Gate: typecheck pass; clippy prism-core 10, prism-ui 24, prism-app 1 (the floor); judge 277/277.
- Decision: KEEP (a fix; E3's floor is now 0 differences, and the next A/B runs use this build as A).

### E-003 At most two repair walks at one time

- Journey: E3.
- Profile: `catalog/mod.rs` `request_scope_repair`: each new or renamed folder asks for a repair walk, and
  each walk runs on a blocking-pool thread of its own. When a burst goes quiet (`SCOPE_REPAIR_QUIET`, 2 s),
  all the waiting walks start together: 7 threads before the churn, 310-373 at the peak (647 in the field
  sample). Each walk also reserves a batch of 20,000 rows (`scanner.rs:221`, 3.4 MB) before it reads one
  folder: private bytes reached 1,084-1,241 MiB in 3 of 13 runs. All walks share one reader and one writer
  connection, so they mostly wait on each other.
- Hypothesis: a semaphore of two slots for the walks (and one slot for shard-root walks, so that a long
  walk leaves a slot for the short ones) should hold the peak below 20 threads and remove the private-bytes
  spikes, with the same rows at the end and no later freshness (about 1.1 s). CPU should drop a little
  (fewer thread starts, less lock contention).
- Test: `churn_bench` differences must stay 0; the time until the catalog equals the disk must not rise.
- Change, `catalog/mod.rs`: `repair_slots` (a semaphore of `SCOPE_REPAIR_SLOTS` = 2) and `shard_repair_slot`
  (1, for shard-root walks); a repair takes its slot after its quiet wait and marks the pass start then, so
  the pass covers the requests that came while it waited.
- Result (`results/raw/e003-churn.jsonl`, A = E-002 build, A B B A x3, n=6 a side): threads at the peak 310
  (289-349) -> 14 (14-14), p 0.003; handles at the peak 403 -> 107; private bytes at the peak 213.5 MiB
  (204.3-760.6: two runs above 500 MiB) -> 190.0 MiB (188.7-194.0), p 0.005; user CPU 1.79 -> 1.59 s (-11%,
  p 0.013); CPU total 4.98 s (4.72-5.49) -> 4.45 s (4.28-5.67), -10.5%, p 0.30 (not significant); bytes
  written 100.9 -> 93.4 MiB (ranges overlap). The catalog equals the disk 1.07-1.13 s after the churn in A
  and 1.07-1.10 s in B; differences 0 in all 12 runs.
- Gate: typecheck pass; clippy at the floor (10 / 24 / 1); judge 277/277.
- Decision: KEEP.

### Journey E4: phase 1 of a keystroke (`query_bench`)

Harness: `crates/prism-core/examples/query_bench.rs` loads the app list from the `apps.json` copy (409
apps), builds the settings index (1,028 entries) and the tools index (92) of this PC, and runs
`query::phase1` for 30 queries with no open windows (two words typed a key at a time, the J3 queries,
settings, tools, a phrase, a query with no match). It writes the hits of each query (kind, id, score) for an
identity check between builds. `--verify` compares the scorer with a copy of the original algorithm for
every target of the three indexes and every prefix of every query.

| ID | Metric | Baseline |
|---|---|---|
| E4 | `query::phase1`, median of 21 runs per query, summed over 30 queries | 69.1 ms (2.3 ms a keystroke), first reading |

Phase 1 runs on a runtime thread, not on the UI thread; its reply paints the app, settings and action rows.

### E-004 The phase-1 scorer allocates nothing for an ASCII target

- Journey: E4.
- Profile (by reading; the bench confirms the total): `query/score.rs:7-8` `score_text` makes a lowercase
  `String` of the query and of the target on every call, and `fuzzy_subsequence` (`:40-41`) makes two
  `Vec<char>`. One keystroke calls it for about 11,000 targets (each app, tool, setting and action title
  and each keyword): about 25,000-45,000 allocations. The callers also build a `Vec<&str>` of the keywords
  of each entry (`query/mod.rs:137`, `:149`, `:187`).
- Hypothesis: prepare the query once for each keystroke (lowercase text, characters, count of letters and
  digits). For an ASCII target, compare bytes with ASCII lowercase on the fly (the same result as
  `to_lowercase`, with no allocation); a target with other characters keeps the present path. Pass the
  keywords as an iterator. E4 should drop from 69 ms to about 10-15 ms (2.3 -> under 0.5 ms a keystroke)
  with identical hits and scores.
- Test: `query_bench --verify` (0 mismatches against the original algorithm) and identical hit lists.
- Change: `query/score.rs` `Query` (`new`, `score`, `score_with_keywords`); `query/mod.rs`, `query/catalog.rs`
  and `query/open_windows.rs` prepare one `Query` for each keystroke and pass the keywords as they are stored.
  `score_text` and `score_with_keywords` stay for single calls (`power.rs`).
- Did the result stay the same: `query_bench --verify` compared the scorer with a copy of the original
  algorithm for 2,098,698 pairs (every title and keyword of the three indexes, plus text with other
  scripts, a final sigma, a dotted capital I and full-width letters, against every prefix of 42 queries): 0
  mismatches. The hit lists of the 30 bench queries are identical in all 12 runs.
- Result (A B B A x3, n=6 a side; the machine was busier than at the first reading): 123.3 ms (113.2-128.0)
  -> 26.9 ms (20.3-29.5) for the 30 queries, -78%, p 0.005: 4.1 -> 0.9 ms a keystroke. The slowest query
  left is one letter (`e`, 2.2 ms): many hits to build and sort.
- Gate: typecheck pass (all targets); clippy at the floor (10 / 24 / 1); judge 277/277.
- Decision: KEEP.

### Journey E5: frames for a typed query (`bench/uiframes.py`)

Harness: `prism.exe --parity` with `ZED_MEASUREMENTS=1` (GPUI prints one `frame duration` line for each frame
it builds and paints). The scenario opens the palette, types the query one key at a time with 200 ms between
the keys, and captures the palette. The frame count is exact: the same in every run.

| ID | Query (keys) | Frames at open | Frames for the keys | Time in frames for the keys |
|---|---|---|---|---|
| E5 | `report` (6; 6 file rows) | 23 | 39 (6.5 a key) | 49.6 ms (46.0-54.1), n=3 |
| E5 | `power` (5; power plans) | 22 | 77 (15.4 a key) | 71.4 ms (68.8-73.9), n=2 |
| E5 | `zzqx` (4; no result) | 22-23 | 12 (3.0 a key) | 10.7 ms (9.7-11.6), n=2 |

A key with no result costs three frames: the echo, the file reply (35 ms later) and the power-plan reply
(50 ms later). The 22-23 frames at open paint a 150 ms fade of two footer icons (the only pixels that change
after the first frame, x 520-570, y 590-602): a state of the footer changes at each open. That is motion
design, so it is a note for the owner and not a change here.

### E-005 A power-plan reply that changes nothing draws no frame

- Journey: E5.
- Profile: `prism-ui/src/state/palette.rs:583-589` `query_power_plans`: 50 ms after each key the reply sets
  `power_plans` and calls `rebuild`, which builds every section again and paints a frame. `set_query`
  emptied the list before, and most queries match no plan, so the reply changes nothing: one of the three
  frames of a key with no result.
- Hypothesis: rebuild only when the reply differs from the list. `zzqx` should drop from 12 to 8 frames
  (3 -> 2 a key) and `report` from 39 to 33 (one frame less a key), with the same final image; `power`
  keeps its rows.
- Test: `uiframes.py` (same capture in all runs of both builds) and the parity journey `power-plan-switch`.
- Result (`results/raw/e005-uiframes.jsonl`, A B B A x2, n=4 a side; the counts are exact, the same in each
  run): `zzqx` 12 -> 8 frames (3 -> 2 a key; time in frames 11.5 -> 8.9 ms); `report` 39 -> 34 frames (6.5
  -> 5.67 a key; 52.1 -> 47.9 ms); `power` 77 -> 75. The captures after the typed query and after the second
  open are the same images in all 24 runs.
- Gate: typecheck pass; clippy at the floor; judge 277/277; parity journeys 23/23
  (`target/verify/runs/20261110-114351`), `power-plan-switch` with `--expect set_power_plan`.
- Decision: KEEP.
- Also measured with the frame harness and the owner's real app list (409 apps with icons, the owner's
  settings; `target/tmp/bench/fixture-real.json`, not in the repo): the first open after a start paints its
  first frame in 60.9 ms (55.0-63.3) and 17 frames in 126 ms; a later open paints one frame in 3.7 ms
  (3.1-4.4), n=4. The typed keys cost the same as with the small fixture (38 frames, 50 ms for `report`).

### Field recording of file events (`bench/fsevents.py`)

A read-only watch of `C:\Users\hi` for 600 s on 2026-11-10 (names only, as Prism's watcher sees them; the
bench's own test tree left out): 563,335 events (257,341 added, 250,946 removed, 27,524 renamed), 98% of them
from one other project's build folder. Of the added entries, 17.2% are gone again within 0.75 s, 19.6%
within 2 s, 20.3% within 5 s. Row writes that a watcher with a given flush window applies (additions and
removals that survive the window): 374,239 at 0.75 s; -13.5% at 2 s; -25.2% at 5 s; -32.0% at 10 s; -36.5%
at 30 s. So this machine removes about 250,000 rows from the catalog in 10 minutes of such work, and the
name index reloads in full after 730,000 removals (`name_index.rs` `needs_sync_with`).

### E-006 The name index keeps a small headroom and grows by 1/32

- Journey: S4 (index memory) and S3 (keystroke search), with the new first-append phase of `catalog_bench`
  (1,000 new rows, then one search; `append_ms`, `append_mib`).
- Profile: `name_index.rs:645-661` `Names::reserve` grows each of the eight buffers by 1/8 of its length
  when an append does not fit. The load sizes the buffers exactly, so the first search after any new file
  moves all of them (about 145 MiB copied) and leaves 1/8 of slack: `append_ms` 39.6-44.2 ms (a usual
  search takes about 4 ms), `append_mib` +8.2 to +10.8 MiB (n=4). In the installed app the row buffers are
  24.68 MiB each: 1.125 times their rows.
- Hypothesis: reserve 1/32 of headroom at the load (about 4 MiB, room for about 180,000 new rows) and grow
  by 1/32 after that. The first append then moves nothing: `append_ms` should drop from about 40 ms to
  about 5 ms, and the memory after the append should drop by about 5 MiB, with the same results.
- Test: `catalog_ab.py` compares the result lists of every run.
- Change, `name_index.rs`: `GROWTH_SHARE` = 32; `load_rows` reserves the rows plus 1/32; `shrink_names`
  shrinks the name buffers to their length plus 1/32; `reserve` grows by 1/32.
- Result (`results/raw/e006-catalog.jsonl`, A B B A x3, n=6 a side): the first search after 1,000 new rows
  37.1 ms (30.8-52.7) -> 13.2 ms (12.8-15.4), -64%, p 0.005; private bytes after that append 177.2 MiB
  (175.2-178.4) -> 161.1 MiB (160.3-164.8), -16.1 MiB, p 0.005 (the 1/8 slack of eight buffers: 17.5 MiB by
  arithmetic). Index memory right after the load 149.8 -> 153.5 MiB (+3.6 MiB: the headroom). Load 2.76 ->
  2.72 s (p 0.69); the 26 result lists are identical in all 12 runs.
- Gate: typecheck pass; clippy prism-core 10 (the floor); judge 277/277.
- Decision: KEEP. In the app the first append comes within minutes of each start, so the steady state is
  16 MiB lower.

### E-007 The writer connection's page cache: 8 MiB, not 32 MiB

- Journey: E3 (private bytes after the churn) and its write and CPU numbers as the guard.
- Profile: `catalog/db.rs:225` `configure_pragmas` sets `cache_size = -32000` (31 MiB) for both connections;
  the reader lowers its own to 8 MiB (`:190`), the writer keeps 31 MiB. The watcher's writes fill it: E3's
  private bytes are 161 MiB before the churn and 187-189 MiB after it (+27 MiB), and they stay.
- Hypothesis: with 8 MiB for the writer, the private bytes after the churn should drop by about 20 MiB.
  A flush writes its rows in path order, so few pages are needed twice in one transaction: the bytes
  written and the CPU should stay inside the noise (a cache that is too small writes a page to the WAL more
  than once). If they rise, the change goes out.
- Test: `churn_bench` differences 0; write bytes and CPU not above A beyond noise.
- Change, `catalog/db.rs`: `WRITER_CACHE_KIB` = -8000, set for the writer after `configure_pragmas`.
- Result (`results/raw/e007-churn.jsonl`, A = E-006 build, A B B A x3, n=6 a side): private bytes after the
  churn 193.0 MiB (191.4-194.4) -> 169.4 MiB (167.2-170.6), -23.6 MiB, p 0.005; at the peak 196.9 -> 176.0
  MiB. Bytes written 94.8 -> 95.4 MiB (p 1.0) and CPU 5.05 -> 5.13 s (p 0.94): no change. Page reads 58.7 ->
  74.5 MiB (+27%, p 0.005): pages that left the smaller cache come back from the system file cache, with
  no CPU cost that shows. Fresh after 1.07-1.23 s (A: 1.07-1.10 s); differences 0 in all 12 runs.
- Gate: typecheck pass; clippy prism-core 10 (the floor); judge 277/277.
- Decision: KEEP.

### Journey E3r: a replay of 60 s of real file events (`churn_bench --recording`)

The built-in script of E3 has 1,400 folders in 35,000 entries. The field recording has one folder for each
2.8 entries (a test suite that makes a sandbox for each test). `churn_bench --recording
target/tmp/bench/replay-60s.jsonl` replays the slice 430-490 s of the recording below the test tree, each
event at its recorded time: 193,093 events (88,295 added, 88,270 removed, 8,264 renamed; 96,109 paths, 34,323
of them folders). The 67,586 entries that the slice removes or renames but does not add are made before the
watcher starts.

| ID | Metric | Baseline (E-007 build, 2 runs) |
|---|---|---|
| E3r | CPU of the process for the 60 s of events | 28.9 s / 25.5 s (kernel 22.2 / 19.1, user 6.7 / 6.3) |
| E3r | bytes written | 858 / 845 MiB in about 355,000 operations |
| E3r | bytes read | 791 / 733 MiB |
| E3r | repair walks (`file-index-updated` events) | 27,413 / 27,442 |
| E3r | catalog equal to the disk | 3.1 s after the last event; 0 differences |

By thread (cycle counters, one run): the flusher 8.4 s, the watcher's readers 1.9 s; the rest (about 15 s)
is the repair walks on blocking-pool threads. So one minute of that test suite costs Prism half a core and
850 MiB of writes. The E-002 fix holds on the real pattern (0 differences).

### E-009 A new folder gets a repair walk only when it holds an entry without an event

- Journey: E3r (and E3 as the guard).
- Profile: `watcher.rs` `apply_queued_events`: every added folder asks for a repair walk
  (`dirty_scopes.insert`), 27,400 walks for the slice. A walk costs a task, two timers, a blocking-pool run,
  a listing of the volume root (`ensure_current_shards`), a folder listing, a catalog query, and a
  `file-index-updated` event: about 15 s of the 26-29 s. The walk exists for a folder that arrives with
  content: its children send no events.
- First hypothesis (wrong, logged as it was written): "a folder whose creation time is inside the current
  flush window cannot hold an entry without an event, so it needs no walk". Result of that form: CPU 28.9 ->
  11.0 s, but 1 file missing from the catalog in each run, the same file. Cause: a move between two folders
  is reported as a removal and an addition, not as a rename. A temp folder that is made, filled and moved
  inside one window is new by its creation time and still arrives with content. REVERTED in that form.
- The trace also showed a defect in the reader (and in the bench's first replay): renames that run at one
  time in different folders interleave their halves (old A, old B, new A, new B: 14 cases in the 60 s
  slice, 180 pairs with other events between them). `run_reader_loop` kept one waiting old name, so it
  paired old B with new A: the wrong folder is renamed in the catalog, and the rows of A stay as ghosts.
  The repair walks hid most of that.
- Second hypothesis: list each added or renamed folder at the flush (the listing carries the creation times)
  and ask for a walk only when it holds an entry that this flush has no addition for and that was there at
  the take. A folder made and filled in place needs no walk. With the reader's rename halves paired by
  folder, and the volume root listed only for a repair at the root or one level below it, walks should drop
  from 27,400 to a few thousand (moved folders, renamed folders with rows), CPU from about 27 s to about
  17 s, and bytes read by a third, with 0 differences in E3r and E3.
- Test: `churn_bench` on the replay (now faithful: the bench pairs rename halves by folder too, and prints
  how many replayed operations failed) and on the built-in script, which has a new phase: four threads that
  each make a temp folder with a file and rename it, 150 times, in a folder of their own. Differences must
  be 0, and the time until the catalog equals the disk must not rise.
- Change: `watcher.rs` `holds_unreported_entry` (one listing of the folder; an entry counts when this flush
  has no addition for it and it was made before the take), used for added and for renamed folders;
  `run_reader_loop` keeps up to `MAX_PENDING_RENAMES` old names and pairs a new name with the old name of
  its folder; `catalog/mod.rs` `reconcile_scope_sync` lists the volume root only near the root.
- Result on E3r (`results/raw/e009-replay.jsonl`, A = E-007 build, A B B A x2, n=4 a side; the replay did
  184,829 operations with 0 failures): CPU 33.9 s (33.6-34.3) -> 17.1 s (15.1-18.0), -50%, every B below
  every A, p 0.03; bytes read 807 -> 487 MiB (-40%); bytes written 882 -> 786 MiB (-11%); the catalog equals
  the disk after 3.4-5.6 s in A and 3.0-3.7 s in B; differences 0 in all 8 runs. Walks about 27,400 -> 2,970.
- Guard on E3 (built-in script with the new phase of renames on four threads, 3 runs): differences 0,
  equal after 1.1-1.2 s, repair walks 3,580 -> about 200, CPU and bytes written inside the earlier range.
- Gate: typecheck pass; clippy prism-core 10 (the floor); judge 277/277.
- Decision: KEEP.

### E-008 While the palette is hidden, the watcher flushes after 10 s (or 4,096 events)

- Journey: E3r.
- Profile: `watcher.rs` `run_flusher_loop` applies the queue each 750 ms. In the field recording 20% of new
  entries are gone within 5 s; the analysis gives 25% fewer row writes with a 5 s window and 32% with 10 s.
  Nobody reads the catalog while the palette is hidden.
- Hypothesis: while the host reports a hidden palette, hold the queue until its oldest event is 10 s old or
  4,096 events wait (a quarter of the limit where the queue collapses into walks); flush at the next tick
  when the palette shows. The slice has about 3,200 events a second, so the count limit acts after about
  1.3 s: bytes written should drop by about 10-15% and CPU a little, with 0 differences. Search freshness
  at an open stays inside one 750 ms tick, as now.
- Test: `churn_bench` on the replay (the bench host reports a hidden palette): differences 0.
- Change, `watcher.rs` `run_flusher_loop`: `HIDDEN_FLUSH_AGE` (10 s) and `HIDDEN_FLUSH_EVENTS` (4,096); the
  loop keeps its 750 ms tick and flushes at once when the palette is visible, the queue collapsed, or a
  limit is reached. With no host the palette counts as visible.
- Result (`results/raw/e008-replay.jsonl`, A = E-009 build, A B B A x2, n=4 a side): bytes written 802.3 MiB
  (792.5-814.1) -> 702.1 MiB (687.4-711.5), -12.5%, every B below every A, p 0.03; CPU 16.5 s (15.9-17.5) ->
  15.3 s (14.6-16.1), -7.5%, p 0.06 (not significant); differences 0 in all 8 runs. While hidden, the
  catalog equals the disk 2.9-12.7 s after the last event (A: 2.9-3.1 s): the hold, as designed.
- Gate: typecheck pass; clippy prism-core 10; judge 277/277.
- Decision: KEEP as its own commit. It changes when a hidden Prism writes, not what a search shows: at an
  open the flush comes inside one 750 ms tick, as before. The owner can remove this one commit.

### Session E summary (2026-11-09 and 2026-11-10)

| Journey | Before | Now | Change | n a side |
|---|---|---|---|---|
| E1 UI at rest, hidden: CPU / context switches | 4.46 ms/s / 262 /s | 0.61 ms/s / 67 /s | -86% / -74% | 6 |
| E3 threads at the peak under file churn | 310 (289-349) | 14 | -95% | 6 |
| E3 private bytes at the peak / after the churn | up to 1,241 MiB / 193 MiB | 176 MiB / 169 MiB | no spikes / -24 MiB | 6 |
| E3 catalog rows that differ from the disk | 1,203-1,220, for good | 0 | fixed | 6 |
| E3r 60 s of real file events: CPU | 33.9 s | 15.3 s | -55% | 4 |
| E3r bytes written / read | 882 / 807 MiB | 702 / 444 MiB | -20% / -45% | 4 |
| E4 phase 1, 30 queries | 123.3 ms | 26.9 ms | -78% | 6 |
| E5 frames for a key with no result | 3 | 2 | -1 frame | 4 |
| S3 first search after new files | 37.1 ms | 13.2 ms | -64% | 6 |
| S4 index memory after the first append | 177.2 MiB | 161.1 MiB | -16.1 MiB | 6 |

Kept: E-001 (20bfeec6), E-002 fix (2d12b825), E-003 (ac39df74), E-004 (9bcbc319), E-005 (7b451604), E-006
(877c2533), E-007 (510c9304), E-009 (ce317d21), E-008 (this commit). Reverted: E-009 first form (creation
time rule: it lost a file). Floor: typecheck pass, clippy 10 / 24 / 1, judge 277/277 before and after;
parity journeys 23/23.

Not done, for the owner: the 35 ms file-search debounce; the mouse hook (13 ms of CPU per second of cursor
movement; a hook only near the taskbar, or one inside Explorer); a slimmer catalog schema (rows are about
880 bytes: three copies of each path); a name-index snapshot file (each start reads 4.3 GB); the 6-hour sweep
(no search reads the sizes and times it refreshes); the 150 ms fade of two footer icons at the first open;
a real-app check of E-001 on the agent screen before a release.

Next hypotheses: (1) the name index reloads in full after 730,000 removed rows and searches use the slow SQL
path meanwhile: build the new index aside and swap; (2) one transaction for each flush (renames, additions
and removals are 3 + N transactions now); (3) the first open after a start paints its first frame in 61 ms
(later opens: 3.7 ms): warm the caches once after the app list loads.
