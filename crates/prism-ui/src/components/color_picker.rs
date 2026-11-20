//! The in-app color picker of the Appearance settings: a saturation/brightness
//! plane over a hue track. This module holds the color math and the painted
//! parts; the settings sheet owns the state, the drag, and the keys.

use gpui::{Div, ParentElement, Styled, div, linear_color_stop, linear_gradient, prelude::*, relative};

use crate::gfx::{r, zoomed};
use crate::theme::{Rgba, Theme, black, hsla, shadow, white};

/// A color as the picker edits it. The hue and saturation live here, not in
/// the RGB value, so they survive a drag through gray or black.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    /// Degrees, 0..360.
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Hsv {
    pub fn from_rgba(c: Rgba) -> Self {
        let max = c.r.max(c.g).max(c.b);
        let min = c.r.min(c.g).min(c.b);
        let delta = max - min;
        let h = if delta <= f32::EPSILON {
            0.0
        } else if max == c.r {
            60.0 * ((c.g - c.b) / delta).rem_euclid(6.0)
        } else if max == c.g {
            60.0 * ((c.b - c.r) / delta + 2.0)
        } else {
            60.0 * ((c.r - c.g) / delta + 4.0)
        };
        Hsv { h, s: if max <= f32::EPSILON { 0.0 } else { delta / max }, v: max }
    }

    pub fn to_rgba(self) -> Rgba {
        let h = self.h.rem_euclid(360.0) / 60.0;
        let c = self.v * self.s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = self.v - c;
        Rgba { r: r + m, g: g + m, b: b + m, a: 1.0 }
    }

    /// `next` from a typed or chosen color, keeping this hue (and saturation)
    /// where the new color has none to give.
    pub fn retarget(self, next: Rgba) -> Self {
        let mut hsv = Hsv::from_rgba(next);
        if hsv.v <= f32::EPSILON {
            hsv.h = self.h;
            hsv.s = self.s;
        } else if hsv.s <= f32::EPSILON {
            hsv.h = self.h;
        }
        hsv
    }
}

/// The full-saturation color of a hue.
fn hue_color(h: f32) -> Rgba {
    Hsv { h, s: 1.0, v: 1.0 }.to_rgba()
}

/// The plane's three layers: the hue, white fading right to left, black rising
/// from the bottom. Each layer carries the radius (GPUI clips rectangles only).
pub fn plane_layers(h: f32, radius: f32) -> [Div; 3] {
    let layer = || div().absolute().inset_0().rounded(r(radius));
    [
        layer().bg(hsla(hue_color(h))),
        layer().bg(linear_gradient(90.0, linear_color_stop(hsla(white(1.0)), 0.0), linear_color_stop(hsla(white(0.0)), 1.0))),
        layer().bg(linear_gradient(180.0, linear_color_stop(hsla(black(0.0)), 0.0), linear_color_stop(hsla(black(1.0)), 1.0))),
    ]
}

/// The hue track: six two-stop segments, red to red (GPUI gradients take two stops).
pub fn hue_track(height: f32) -> Div {
    div().absolute().inset_0().flex().children((0..6).map(|i| {
        let from = hue_color(i as f32 * 60.0);
        let to = hue_color((i + 1) as f32 * 60.0);
        div()
            .flex_1()
            .h(r(height))
            .when(i == 0, |d| d.rounded_l_full())
            .when(i == 5, |d| d.rounded_r_full())
            .bg(linear_gradient(90.0, linear_color_stop(hsla(from), 0.0), linear_color_stop(hsla(to), 1.0)))
    }))
}

/// A thumb centered on `(x, y)`, both fractions of the parent. White ring, a
/// shadow to lift it off light colors, filled with the color under it.
pub fn thumb(x: f32, y: f32, fill: Rgba, size: f32, t: &Theme, zoom: f32) -> Div {
    div()
        .absolute()
        .left(relative(x))
        .top(relative(y))
        .ml(r(-size / 2.0))
        .mt(r(-size / 2.0))
        .size(r(size))
        .rounded_full()
        .border_2()
        .border_color(hsla(white(1.0)))
        .bg(hsla(fill))
        .shadow(zoomed(vec![shadow(0., 0., 0., 1., black(0.18)), shadow(0., 1., 4., 0., t.shade(0.45))], zoom))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rgba, b: Rgba) -> bool {
        (a.r - b.r).abs() < 1e-4 && (a.g - b.g).abs() < 1e-4 && (a.b - b.b).abs() < 1e-4
    }

    #[test]
    fn round_trips_through_hsv() {
        for (r, g, b) in [(1.0, 0.0, 0.0), (0.2, 0.6, 0.9), (0.5, 0.5, 0.5), (0.95, 0.8, 0.1), (0.3, 0.1, 0.35), (0.0, 0.0, 0.0)] {
            let c = Rgba { r, g, b, a: 1.0 };
            assert!(close(Hsv::from_rgba(c).to_rgba(), c), "{c:?}");
        }
    }

    #[test]
    fn keeps_hue_through_gray_and_black() {
        let start = Hsv { h: 200.0, s: 0.7, v: 0.8 };
        let gray = start.retarget(Rgba { r: 0.5, g: 0.5, b: 0.5, a: 1.0 });
        assert_eq!((gray.h, gray.s), (200.0, 0.0));
        let black = start.retarget(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 });
        assert_eq!((black.h, black.s, black.v), (200.0, 0.7, 0.0));
    }
}
