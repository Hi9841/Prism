//! Design tokens from `src/styles/tokens.css`: semantic colors per theme,
//! the five accent presets, tile gradients, radii, shadows, and motion.
//!
//! OKLCH and `color-mix(in oklab, ...)` values are converted to sRGB here with
//! the CSS Color 4 formulas Chromium uses, so the port paints the same colors.

pub use gpui::Rgba;
use gpui::{BoxShadow, FontFeatures, Hsla, Styled, point, px};

/// `oklch(l c h / alpha)` as sRGB. Out-of-gamut channels are clipped, as Chromium does.
pub fn oklch(l: f32, c: f32, h: f32, alpha: f32) -> Rgba {
    let h = h.to_radians();
    oklab(l, c * h.cos(), c * h.sin(), alpha)
}

fn oklab(l: f32, a: f32, b: f32, alpha: f32) -> Rgba {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let b = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    Rgba { r: encode(r), g: encode(g), b: encode(b), a: alpha }
}

fn encode(linear: f32) -> f32 {
    let v = if linear <= 0.003_130_8 { 12.92 * linear } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 };
    v.clamp(0.0, 1.0)
}

fn decode(srgb: f32) -> f32 {
    if srgb <= 0.040_45 { srgb / 12.92 } else { ((srgb + 0.055) / 1.055).powf(2.4) }
}

fn to_oklab(c: Rgba) -> [f32; 3] {
    let (r, g, b) = (decode(c.r), decode(c.g), decode(c.b));
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    ]
}

/// `color-mix(in oklab, first <percent>, second)` with premultiplied alpha.
pub fn mix(first: Rgba, percent: f32, second: Rgba) -> Rgba {
    let p = percent / 100.0;
    let alpha = first.a * p + second.a * (1.0 - p);
    if alpha <= 0.0 {
        return Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    }
    let (x, y) = (to_oklab(first), to_oklab(second));
    let channel = |i: usize| (x[i] * first.a * p + y[i] * second.a * (1.0 - p)) / alpha;
    oklab(channel(0), channel(1), channel(2), alpha)
}

/// `color-mix(in oklab, color <percent>, transparent)`.
pub fn fade(color: Rgba, percent: f32) -> Rgba {
    Rgba { a: color.a * percent / 100.0, ..color }
}

/// Source-over compositing of two translucent colors (sRGB, as browsers paint).
pub fn over(top: Rgba, bottom: Rgba) -> Rgba {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 0.0 {
        return Rgba { a: 0.0, ..top };
    }
    let channel = |t: f32, b: f32| (t * top.a + b * bottom.a * (1.0 - top.a)) / a;
    Rgba { r: channel(top.r, bottom.r), g: channel(top.g, bottom.g), b: channel(top.b, bottom.b), a }
}

/// `rgb(r g b / alpha)`.
pub fn rgb_a(r: u8, g: u8, b: u8, alpha: f32) -> Rgba {
    Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: alpha }
}

pub fn white(alpha: f32) -> Rgba {
    rgb_a(255, 255, 255, alpha)
}

pub fn black(alpha: f32) -> Rgba {
    rgb_a(0, 0, 0, alpha)
}

pub const TRANSPARENT: Rgba = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

pub fn hsla(c: Rgba) -> Hsla {
    c.into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accent {
    Iris,
    Azure,
    Mint,
    Amber,
    Rose,
}

impl Accent {
    pub const ALL: [Accent; 5] = [Accent::Iris, Accent::Azure, Accent::Mint, Accent::Amber, Accent::Rose];

    pub fn parse(id: &str) -> Self {
        match id {
            "azure" => Accent::Azure,
            "mint" => Accent::Mint,
            "amber" => Accent::Amber,
            "rose" => Accent::Rose,
            _ => Accent::Iris,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Accent::Iris => "iris",
            Accent::Azure => "azure",
            Accent::Mint => "mint",
            Accent::Amber => "amber",
            Accent::Rose => "rose",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Accent::Iris => "Iris",
            Accent::Azure => "Azure",
            Accent::Mint => "Mint",
            Accent::Amber => "Amber",
            Accent::Rose => "Rose",
        }
    }

    /// `--accent`, `--accent-strong`, `--accent-fg`.
    fn colors(self) -> (Rgba, Rgba, Rgba) {
        match self {
            Accent::Iris => (oklch(0.665, 0.17, 292.0, 1.0), oklch(0.72, 0.17, 292.0, 1.0), oklch(0.985, 0.004, 292.0, 1.0)),
            Accent::Azure => {
                (oklch(0.66, 0.145, 245.0, 1.0), oklch(0.71, 0.145, 245.0, 1.0), oklch(0.985, 0.003, 245.0, 1.0))
            }
            Accent::Mint => {
                (oklch(0.735, 0.125, 168.0, 1.0), oklch(0.785, 0.115, 168.0, 1.0), oklch(0.19, 0.02, 168.0, 1.0))
            }
            Accent::Amber => {
                (oklch(0.77, 0.145, 78.0, 1.0), oklch(0.81, 0.135, 78.0, 1.0), oklch(0.2, 0.03, 78.0, 1.0))
            }
            Accent::Rose => (oklch(0.665, 0.17, 12.0, 1.0), oklch(0.71, 0.16, 12.0, 1.0), oklch(0.985, 0.004, 12.0, 1.0)),
        }
    }

    /// The static swatch color (`--accent-<id>`).
    pub fn swatch(self) -> Rgba {
        self.colors().0
    }
}

/// A two-stop vertical or angled gradient.
#[derive(Clone, Copy, Debug)]
pub struct Gradient {
    pub angle: f32,
    pub from: Rgba,
    pub to: Rgba,
    /// Chromium interpolates in Oklab when a stop is not a legacy sRGB color
    /// (`oklch()`), and in sRGB when every stop is legacy (`rgb()`, hex).
    pub space: gpui::ColorSpace,
    /// The exact Oklab stops (L, a, b, alpha) of an `oklch()` gradient. A stop can lie
    /// outside sRGB; CSS mixes the exact values and clips each pixel, not the stops.
    pub oklab_stops: Option<[[f32; 4]; 2]>,
}

impl Gradient {
    /// A gradient whose CSS stops are legacy sRGB colors.
    pub fn srgb(angle: f32, from: Rgba, to: Rgba) -> Self {
        Self { angle, from, to, space: gpui::ColorSpace::Srgb, oklab_stops: None }
    }

    /// A gradient whose CSS stops are `oklch(l c h / alpha)` colors.
    pub fn oklch(angle: f32, from: (f32, f32, f32, f32), to: (f32, f32, f32, f32)) -> Self {
        let exact = |(l, c, h, a): (f32, f32, f32, f32)| {
            let h = h.to_radians();
            [l, c * h.cos(), c * h.sin(), a]
        };
        Self {
            angle,
            from: oklch(from.0, from.1, from.2, from.3),
            to: oklch(to.0, to.1, to.2, to.3),
            space: gpui::ColorSpace::Oklab,
            oklab_stops: Some([exact(from), exact(to)]),
        }
    }

    pub fn background(self) -> gpui::Background {
        if let Some([from, to]) = self.oklab_stops {
            // Vendored gpui patch 17: `OklabValues` reads L, a, b from the Hsla fields.
            let stop = |[l, a, b, alpha]: [f32; 4], at| gpui::linear_color_stop(Hsla { h: l, s: a, l: b, a: alpha }, at);
            return gpui::linear_gradient(self.angle, stop(from, 0.0), stop(to, 1.0))
                .color_space(gpui::ColorSpace::OklabValues);
        }
        gpui::linear_gradient(
            self.angle,
            gpui::linear_color_stop(hsla(self.from), 0.0),
            gpui::linear_color_stop(hsla(self.to), 1.0),
        )
        .color_space(self.space)
    }
}

/// Every resolved token for one theme mode and accent.
#[derive(Clone, Debug)]
pub struct Theme {
    pub mode: Mode,
    pub accent_id: Accent,
    pub shell_solid: Gradient,
    pub shell_line: Rgba,
    pub shell_hi: Rgba,
    pub field_bg: Rgba,
    pub field_bg_focus: Rgba,
    pub field_hi: Rgba,
    pub field_line: Rgba,
    pub surface: Rgba,
    pub surface_hover: Rgba,
    pub surface_active: Rgba,
    pub seg_active: Rgba,
    pub track: Rgba,
    pub kbd: Gradient,
    pub kbd_line: Rgba,
    pub kbd_hi: Rgba,
    pub line: Rgba,
    pub divider: Rgba,
    pub backdrop: Rgba,
    pub toast: Rgba,
    pub bg_raised: Rgba,
    pub fg: Rgba,
    pub fg_secondary: Rgba,
    pub fg_tertiary: Rgba,
    pub fg_quiet: Rgba,
    pub danger: Rgba,
    pub danger_soft: Rgba,
    pub scroll: Rgba,
    pub scroll_hover: Rgba,
    pub accent: Rgba,
    pub accent_strong: Rgba,
    pub accent_fg: Rgba,
    pub accent_soft: Rgba,
    pub accent_softer: Rgba,
    pub accent_ring: Rgba,
}

impl Theme {
    pub fn new(mode: Mode, accent_id: Accent) -> Self {
        let (accent, accent_strong, accent_fg) = accent_id.colors();
        let common = |t: Theme| Theme {
            accent,
            accent_strong,
            accent_fg,
            accent_soft: fade(accent, 17.0),
            accent_softer: fade(accent, 9.0),
            accent_ring: fade(accent, 45.0),
            ..t
        };
        common(match mode {
            Mode::Dark => Theme {
                mode,
                accent_id,
                shell_solid: Gradient::oklch(180.0, (0.17, 0.009, 268.0, 1.0), (0.152, 0.008, 268.0, 1.0)),
                shell_line: white(0.08),
                shell_hi: white(0.06),
                field_bg: white(0.045),
                field_bg_focus: white(0.06),
                field_hi: white(0.05),
                field_line: white(0.05),
                surface: white(0.06),
                surface_hover: white(0.06),
                surface_active: white(0.075),
                seg_active: white(0.1),
                track: white(0.12),
                kbd: Gradient::srgb(180.0, white(0.08), white(0.03)),
                kbd_line: white(0.09),
                kbd_hi: white(0.06),
                line: white(0.08),
                divider: white(0.2),
                backdrop: rgb_a(5, 5, 9, 0.5),
                toast: oklch(0.16, 0.008, 268.0, 0.85),
                bg_raised: oklch(0.19, 0.009, 268.0, 0.85),
                fg: oklch(0.955, 0.003, 268.0, 1.0),
                fg_secondary: oklch(0.78, 0.007, 268.0, 1.0),
                fg_tertiary: oklch(0.66, 0.007, 268.0, 1.0),
                fg_quiet: oklch(0.61, 0.006, 268.0, 1.0),
                danger: oklch(0.78, 0.14, 20.0, 1.0),
                danger_soft: oklch(0.78, 0.14, 20.0, 0.12),
                scroll: white(0.12),
                scroll_hover: white(0.2),
                accent,
                accent_strong,
                accent_fg,
                accent_soft: accent,
                accent_softer: accent,
                accent_ring: accent,
            },
            Mode::Light => Theme {
                mode,
                accent_id,
                shell_solid: Gradient::oklch(180.0, (0.995, 0.002, 268.0, 1.0), (0.965, 0.004, 268.0, 1.0)),
                shell_line: black(0.08),
                shell_hi: white(0.85),
                field_bg: black(0.03),
                field_bg_focus: black(0.04),
                field_hi: white(0.8),
                field_line: black(0.05),
                surface: black(0.04),
                surface_hover: black(0.05),
                surface_active: black(0.06),
                seg_active: white(0.85),
                track: black(0.12),
                kbd: Gradient::srgb(180.0, white(0.95), white(0.65)),
                kbd_line: black(0.1),
                kbd_hi: white(0.9),
                line: black(0.08),
                divider: black(0.15),
                backdrop: black(0.28),
                toast: oklch(0.995, 0.002, 268.0, 0.9),
                bg_raised: oklch(0.995, 0.002, 268.0, 0.88),
                fg: oklch(0.22, 0.012, 268.0, 1.0),
                fg_secondary: oklch(0.35, 0.012, 268.0, 1.0),
                fg_tertiary: oklch(0.44, 0.01, 268.0, 1.0),
                fg_quiet: oklch(0.5, 0.008, 268.0, 1.0),
                danger: oklch(0.55, 0.16, 20.0, 1.0),
                danger_soft: oklch(0.55, 0.16, 20.0, 0.1),
                scroll: black(0.12),
                scroll_hover: black(0.2),
                accent,
                accent_strong,
                accent_fg,
                accent_soft: accent,
                accent_softer: accent,
                accent_ring: accent,
            },
        })
    }

    pub fn is_light(&self) -> bool {
        self.mode == Mode::Light
    }

    /// Black at `alpha`, for a shadow. The tokens were tuned on the dark
    /// theme, where a shadow falls on a near-black surface; on the light
    /// theme the same values read as smudges, so it paints them at
    /// `LIGHT_SHADOW_STRENGTH`.
    pub fn shade(&self, alpha: f32) -> Rgba {
        black(if self.is_light() { alpha * LIGHT_SHADOW_STRENGTH } else { alpha })
    }

    /// `.tile-<tint>` gradient (145deg).
    pub fn tile(&self, tint: &str) -> Gradient {
        let (from, to) = match tint {
            "iris" => ((0.72, 0.16, 292.0, 0.9), (0.5, 0.19, 295.0, 0.9)),
            "azure" => ((0.74, 0.14, 243.0, 0.9), (0.52, 0.16, 248.0, 0.9)),
            "mint" => ((0.8, 0.11, 168.0, 0.9), (0.6, 0.14, 172.0, 0.9)),
            "amber" => ((0.82, 0.12, 80.0, 0.9), (0.64, 0.15, 72.0, 0.9)),
            "rose" => ((0.74, 0.15, 14.0, 0.9), (0.54, 0.18, 8.0, 0.9)),
            _ if self.is_light() => ((0.55, 0.02, 268.0, 0.95), (0.42, 0.015, 268.0, 0.95)),
            _ => ((0.42, 0.01, 268.0, 0.95), (0.3, 0.012, 268.0, 0.95)),
        };
        Gradient::oklch(145.0, from, to)
    }

    /// `.tile-mono` with `--mono-h: 0.38 + hue * 0.34` (the hue variable is a lightness).
    pub fn mono_tile(&self, hue: f64) -> Gradient {
        let l = (0.38 + hue * 0.34) as f32;
        let (from, to) = if self.is_light() {
            ((l, 0.12, 268.0, 1.0), (l - 0.15, 0.15, 268.0, 1.0))
        } else {
            ((l, 0.09, 268.0, 1.0), (l - 0.12, 0.12, 268.0, 1.0))
        };
        Gradient::oklch(145.0, from, to)
    }

    /// The selected row: one even accent tint, stronger than any neutral
    /// surface in both themes (the old left-edge wash over surface-active read
    /// weaker than a hovered row on the light theme).
    pub fn row_selected_fill(&self) -> Rgba {
        fade(self.accent, if self.mode == Mode::Dark { 24.0 } else { 24.0 })
    }

    /// The selected row's 1px ring.
    pub fn row_selected_ring(&self) -> Rgba {
        fade(self.accent, if self.mode == Mode::Dark { 60.0 } else { 72.0 })
    }

    /// `color-mix(in oklab, var(--accent) 13%, var(--t-surface-active))`.
    pub fn drop_target_fill(&self) -> Rgba {
        mix(self.accent, 13.0, self.surface_active)
    }

    /// `color-mix(in oklab, var(--accent) 72%, var(--t-fg-secondary))`.
    pub fn search_icon_focus(&self) -> Rgba {
        mix(self.accent, 72.0, self.fg_secondary)
    }

    /// `color-mix(in oklab, var(--t-line) 72%, transparent)`.
    pub fn footer_line(&self) -> Rgba {
        fade(self.line, 72.0)
    }

    /// `color-mix(in oklab, var(--t-fg) 10%, transparent)`.
    pub fn preview_hi(&self) -> Rgba {
        fade(self.fg, 10.0)
    }
}

/// `--radius-*` in px.
pub mod radius {
    pub const XS: f32 = 6.0;
    pub const SM: f32 = 10.0;
    pub const MD: f32 = 14.0;
    pub const LG: f32 = 18.0;
}

/// Motion tokens (ms) and easings.
pub mod motion {
    pub const FAST_MS: u64 = 110;
    pub const BASE_MS: u64 = 170;
    pub const SLOW_MS: u64 = 260;

    /// `cubic-bezier(x1, y1, x2, y2)` evaluated at time `t` in 0..=1.
    pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let bezier = |a: f32, b: f32, s: f32| {
            let u = 1.0 - s;
            3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
        };
        let derivative = |a: f32, b: f32, s: f32| {
            let u = 1.0 - s;
            3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
        };
        // Newton iterations on x(s) = t, then bisection as a fallback.
        let mut s = t;
        for _ in 0..8 {
            let x = bezier(x1, x2, s) - t;
            let d = derivative(x1, x2, s);
            if x.abs() < 1e-5 {
                return bezier(y1, y2, s);
            }
            if d.abs() < 1e-6 {
                break;
            }
            s -= x / d;
        }
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        s = t;
        for _ in 0..30 {
            let x = bezier(x1, x2, s);
            if (x - t).abs() < 1e-5 {
                break;
            }
            if x < t {
                lo = s;
            } else {
                hi = s;
            }
            s = (lo + hi) / 2.0;
        }
        bezier(y1, y2, s)
    }

    /// `--ease-out-soft: cubic-bezier(0.22, 1, 0.36, 1)`.
    pub fn ease_out_soft(t: f32) -> f32 {
        cubic_bezier(0.22, 1.0, 0.36, 1.0, t)
    }

    /// CSS `ease-in`: `cubic-bezier(0.42, 0, 1, 1)`.
    pub fn ease_in(t: f32) -> f32 {
        cubic_bezier(0.42, 0.0, 1.0, 1.0, t)
    }

    /// Tailwind `--default-transition-timing-function`: `cubic-bezier(0.4, 0, 0.2, 1)`.
    /// A `transition-*` class without an `ease-*` class uses it.
    pub fn tailwind_default(t: f32) -> f32 {
        cubic_bezier(0.4, 0.0, 0.2, 1.0, t)
    }

    /// Tailwind `ease-out` (`--ease-out`): `cubic-bezier(0, 0, 0.2, 1)`.
    pub fn tailwind_ease_out(t: f32) -> f32 {
        cubic_bezier(0.0, 0.0, 0.2, 1.0, t)
    }

    /// CSS `ease-out`: `cubic-bezier(0, 0, 0.58, 1)`.
    pub fn ease_out(t: f32) -> f32 {
        cubic_bezier(0.0, 0.0, 0.58, 1.0, t)
    }

    /// CSS `ease`: `cubic-bezier(0.25, 0.1, 0.25, 1)`.
    pub fn ease(t: f32) -> f32 {
        cubic_bezier(0.25, 0.1, 0.25, 1.0, t)
    }

    /// `.icon-swap`: `cubic-bezier(0.2, 0, 0, 1)`.
    pub fn icon_swap(t: f32) -> f32 {
        cubic_bezier(0.2, 0.0, 0.0, 1.0, t)
    }
}

/// Share of a shadow's alpha that the light theme paints (see `Theme::shade`).
const LIGHT_SHADOW_STRENGTH: f32 = 0.4;

/// `--shadow-pop`.
pub fn shadow_pop(t: &Theme) -> Vec<BoxShadow> {
    vec![shadow(0.0, 1.0, 2.0, 0.0, t.shade(0.35)), shadow(0.0, 8.0, 24.0, 0.0, t.shade(0.28)), shadow(0.0, 24.0, 64.0, 0.0, t.shade(0.35))]
}

/// `--shadow-raised`.
pub fn shadow_raised(t: &Theme) -> Vec<BoxShadow> {
    vec![shadow(0.0, 1.0, 2.0, 0.0, t.shade(0.3)), shadow(0.0, 4.0, 14.0, 0.0, t.shade(0.24))]
}

/// `--shadow-tile`.
pub fn shadow_tile(t: &Theme) -> Vec<BoxShadow> {
    vec![shadow(0.0, 1.0, 1.5, 0.0, t.shade(0.4)), shadow(0.0, 4.0, 10.0, 0.0, t.shade(0.3))]
}

/// One CSS box shadow, lengths in px (rem-scaled by the caller when needed).
/// CSS blurs with sigma = blur / 2; GPUI's `blur_radius` is the sigma. The
/// vendored `paint_shadows` draws a sharp ring as a border (patch 7).
pub fn shadow(x: f32, y: f32, blur: f32, spread: f32, color: Rgba) -> BoxShadow {
    let sigma = blur / 2.0;
    BoxShadow { color: hsla(color), offset: point(px(x), px(y)), blur_radius: px(sigma), spread_radius: px(spread) }
}

/// Tailwind `tabular-nums` (`font-variant-numeric: tabular-nums`): the OpenType `tnum` feature.
pub fn tabular_nums<E: Styled>(mut element: E) -> E {
    element.text_style().get_or_insert_with(Default::default).font_features =
        Some(FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])));
    element
}

pub const FONT_SANS: &str = "Segoe UI Variable Text";
pub const FONT_SANS_FALLBACK: &str = "Segoe UI";
pub const FONT_MONO: &str = "Cascadia Code";

// PORT STATUS: confidence=medium todos=0
