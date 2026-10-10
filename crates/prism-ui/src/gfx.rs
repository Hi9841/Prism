//! What the browser gave the original for free: embedded assets, lucide icons,
//! data-URL images, CSS transitions, reduced motion, and px lengths that
//! scale with the view zoom (RULEBOOK A11: every length is in rems).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use gpui::{
    AnyElement, AssetSource, BoxShadow, Hsla, Image, ImageFormat, IntoElement, Rems, Rgba, SharedString, Styled,
    Svg, Transformation, Window, rems, svg,
};

use crate::theme::hsla;

mod table {
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}

/// Serves the embedded `assets/` tree.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(table::ASSETS
            .binary_search_by(|(key, _)| (*key).cmp(path))
            .ok()
            .map(|ix| Cow::Borrowed(table::ASSETS[ix].1)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(table::ASSETS
            .iter()
            .filter(|(key, _)| key.starts_with(path))
            .map(|(key, _)| SharedString::from(*key))
            .collect())
    }
}

/// Chromium scrolls 100/3 px per wheel line on Windows (Step 2, M3), at any
/// zoom. GPUI scrolls an overflow container by its own line height, so scroll
/// containers carry this line height and their content resets it.
pub const CHROMIUM_LINE_PX: f32 = 100.0 / 3.0;

/// A CSS pixel length that scales with the view zoom (`px / 16` rems).
pub fn r(px: f32) -> Rems {
    rems(px / 16.0)
}

/// A lucide icon by its file name (`search`, `chevron-down`, ...), sized in CSS px.
pub fn icon(name: &str, size: f32, color: Rgba) -> Svg {
    svg().path(SharedString::from(format!("icons/{name}.svg"))).size(r(size)).flex_none().text_color(hsla(color))
}

/// An icon with a non-default stroke width (`check@3`).
pub fn icon_stroke(name: &str, stroke: f32, size: f32, color: Rgba) -> Svg {
    let suffix = format!("{stroke}").replace('.', "_");
    icon(&format!("{name}@{suffix}"), size, color)
}

/// Rotates an icon about its center (radians).
pub fn rotated(svg: Svg, radians: f32) -> Svg {
    svg.with_transformation(Transformation::rotate(gpui::radians(radians)))
}

/// Scales a list of CSS box shadows by the view zoom.
pub fn zoomed(shadows: Vec<BoxShadow>, zoom: f32) -> Vec<BoxShadow> {
    shadows
        .into_iter()
        .map(|s| BoxShadow {
            offset: gpui::point(s.offset.x * zoom, s.offset.y * zoom),
            blur_radius: s.blur_radius * zoom,
            spread_radius: s.spread_radius * zoom,
            ..s
        })
        .collect()
}

/// Parity captures (`prism --parity`): an opaque backdrop behind the shell and
/// no caret, so both sides render deterministic pixels.
#[derive(Clone, Copy, Debug)]
pub struct ParityOptions {
    pub backdrop: Rgba,
}

static PARITY: std::sync::OnceLock<ParityOptions> = std::sync::OnceLock::new();

pub fn enable_parity(options: ParityOptions) {
    let _ = PARITY.set(options);
}

pub fn parity() -> Option<ParityOptions> {
    PARITY.get().copied()
}

/// Decoded `data:image/...;base64,` URLs, cached by URL (icons and thumbnails).
static DATA_IMAGES: LazyLock<Mutex<HashMap<String, Option<Arc<Image>>>>> = LazyLock::new(Mutex::default);
const DATA_IMAGE_CACHE_LIMIT: usize = 1024;

pub fn data_url_image(url: &str) -> Option<Arc<Image>> {
    let mut cache = DATA_IMAGES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(image) = cache.get(url) {
        return image.clone();
    }
    let image = decode_data_url(url);
    if cache.len() >= DATA_IMAGE_CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(url.to_owned(), image.clone());
    image
}

fn decode_data_url(url: &str) -> Option<Arc<Image>> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime = meta.split(';').next().unwrap_or("");
    let bytes = if meta.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD.decode(data).ok()?
    } else {
        data.as_bytes().to_vec()
    };
    let format = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/bmp" => ImageFormat::Bmp,
        "image/svg+xml" => ImageFormat::Svg,
        _ => ImageFormat::Png,
    };
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

/// PNG bytes as an image (custom Start icon previews).
pub fn png_image(bytes: Vec<u8>) -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Png, bytes))
}

/// An interruptible CSS-style transition of one number.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    ease: fn(f32) -> f32,
}

impl Tween {
    pub fn at(value: f32) -> Self {
        Self { from: value, to: value, start: Instant::now(), duration: Duration::ZERO, ease: linear }
    }

    pub fn value(&self) -> f32 {
        if self.duration.is_zero() {
            return self.to;
        }
        let t = (self.start.elapsed().as_secs_f32() / self.duration.as_secs_f32()).min(1.0);
        self.from + (self.to - self.from) * (self.ease)(t)
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn is_active(&self) -> bool {
        !self.duration.is_zero() && self.start.elapsed() < self.duration
    }

    /// Starts a transition from the current value. No-op when the target is unchanged.
    pub fn set(&mut self, to: f32, duration_ms: u64, ease: fn(f32) -> f32) {
        if (to - self.to).abs() < f32::EPSILON {
            return;
        }
        let from = self.value();
        *self = Self { from, to, start: Instant::now(), duration: Duration::from_millis(duration_ms), ease };
    }

    /// Jumps to a value with no transition.
    pub fn jump(&mut self, to: f32) {
        *self = Self::at(to);
    }

    /// Restarts a keyframe animation `from -> to` (a CSS `animation`).
    pub fn play(&mut self, from: f32, to: f32, duration_ms: u64, ease: fn(f32) -> f32) {
        *self = Self { from, to, start: Instant::now(), duration: Duration::from_millis(duration_ms), ease };
    }

    /// Keeps frames coming while the transition runs.
    pub fn animate(&self, window: &mut Window) -> f32 {
        if self.is_active() {
            window.request_animation_frame();
        }
        self.value()
    }
}

pub fn linear(t: f32) -> f32 {
    t
}

/// `prefers-reduced-motion: reduce` = Windows "Animation effects" off (read at each use).
pub fn reduced_motion() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut enabled = windows::core::BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut _ as *mut std::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && !enabled.as_bool()
}

/// Duration for a decorative animation: 0 under reduced motion (`0.01ms !important`).
pub fn motion_ms(ms: u64) -> u64 {
    if reduced_motion() { 0 } else { ms }
}

/// Text color with an extra opacity (`text-fg/90`).
pub fn alpha(color: Rgba, a: f32) -> Hsla {
    hsla(Rgba { a: color.a * a, ..color })
}

pub fn into_any(element: impl IntoElement) -> AnyElement {
    element.into_any_element()
}

/// Chromium's wheel scrolling for one overflow container: 100/3 px per wheel
/// line at any zoom (M3), animated toward the target like Chrome's smooth
/// scrolling, innermost hovered scroller first, and passed on at a boundary
/// (scroll chaining). GPUI's own wheel handling scrolls by the container's
/// rounded line height, which drifts 1% per line.
#[derive(Default)]
pub struct WheelScroll {
    target: Option<f32>,
    from: f32,
    start: Option<Instant>,
}

const WHEEL_SCROLL_MS: f32 = 160.0;

impl WheelScroll {
    /// Advances the animation; returns true while it runs.
    fn step(&mut self, handle: &gpui::ScrollHandle) -> bool {
        let (Some(target), Some(start)) = (self.target, self.start) else { return false };
        let t = (start.elapsed().as_secs_f32() * 1000.0 / WHEEL_SCROLL_MS).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let y = self.from + (target - self.from) * eased;
        let offset = handle.offset();
        handle.set_offset(gpui::point(offset.x, gpui::px(y)));
        if t >= 1.0 {
            self.target = None;
            self.start = None;
            return false;
        }
        true
    }
}

/// The wheel layer for a scroll container. Place it after the container as an
/// absolute, full-size sibling inside a relative wrapper.
pub fn wheel_scroll_layer(handle: gpui::ScrollHandle, state: std::rc::Rc<std::cell::RefCell<WheelScroll>>) -> impl IntoElement {
    use gpui::{DispatchPhase, HitboxBehavior, ScrollDelta, ScrollWheelEvent, canvas};
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            if state.borrow_mut().step(&handle) {
                window.request_animation_frame();
            }
            let handle = handle.clone();
            let state = state.clone();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || event.modifiers.control || !hitbox.should_handle_scroll(window) {
                    return;
                }
                let delta = match event.delta {
                    ScrollDelta::Lines(lines) => -lines.y * CHROMIUM_LINE_PX,
                    ScrollDelta::Pixels(pixels) => -f32::from(pixels.y),
                };
                if delta == 0.0 {
                    return;
                }
                let max = f32::from(handle.max_offset().height);
                let mut state = state.borrow_mut();
                let current = -f32::from(handle.offset().y);
                let base = state.target.map(|t| -t).unwrap_or(current);
                let next = (base + delta).clamp(0.0, max.max(0.0));
                if (next - base).abs() < 0.01 {
                    // At the boundary: the next scroller in the chain takes it.
                    return;
                }
                cx.stop_propagation();
                state.from = -current;
                state.target = Some(-next);
                state.start = Some(Instant::now());
                window.refresh();
            });
        },
    )
    .absolute()
    .size_full()
}

/* ---------------- Settings panel shadow ---------------- */

// The settings panel's border box in the current frame. Set in prepaint;
// `None` at the start of every frame.
thread_local! {
    static SHEET_PANEL: std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>> = const { std::cell::Cell::new(None) };
}

pub fn set_sheet_panel(panel: Option<gpui::Bounds<gpui::Pixels>>) {
    SHEET_PANEL.with(|cell| cell.set(panel));
}

/// The panel's `box-shadow`: CSS paints an outer shadow outside the border box
/// only, so it is clipped to the area left of the panel.
pub fn sheet_panel_shadow(shadows: Vec<BoxShadow>, radii: gpui::Corners<gpui::Pixels>) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            // The panel prepaints after this element, but every prepaint precedes paint.
            let Some(panel) = SHEET_PANEL.with(|cell| cell.get()) else { return };
            let outside = gpui::Bounds::from_corners(bounds.origin, gpui::point(panel.left(), bounds.bottom()));
            window.with_content_mask(Some(gpui::ContentMask { bounds: outside }), |window| {
                window.paint_shadows(panel, radii, &shadows);
            });
        },
    )
    .absolute()
    .size_full()
}

// PORT STATUS: confidence=medium todos=0
