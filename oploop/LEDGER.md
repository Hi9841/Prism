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
