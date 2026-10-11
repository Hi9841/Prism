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
///
/// It also keeps the state of the container's scrollbar (`wheel_scroll_layer`).
#[derive(Default)]
pub struct WheelScroll {
    target: Option<f32>,
    from: f32,
    start: Option<Instant>,
    /// The content overflowed in the last frame, so the gutter is reserved.
    overflowing: bool,
    /// A thumb drag: the pointer's y and the scroll offset at its start.
    drag: Option<(f32, f32)>,
}

const WHEEL_SCROLL_MS: f32 = 160.0;

/* Chromium's thin Fluent scrollbar (`.scroll-thin`: `scrollbar-width: thin`,
 * measured on the Tauri app): an 11px gutter that is reserved only while the
 * content overflows, 12px arrow buttons at both ends, and a 6px rounded thumb
 * 2px from the right edge. The size follows the device scale, not CSS zoom. */
const SCROLLBAR_GUTTER: f32 = 11.0;
const SCROLLBAR_BUTTON: f32 = 12.0;
const SCROLLBAR_THUMB: f32 = 6.0;
const SCROLLBAR_RIGHT_INSET: f32 = 2.0;
const SCROLLBAR_MIN_THUMB: f32 = 17.0;
/// Chromium scrolls 40px for an arrow click and 87.5% of the view for a track click.
const SCROLLBAR_ARROW_STEP: f32 = 40.0;
const SCROLLBAR_PAGE_FRACTION: f32 = 0.875;

impl WheelScroll {
    /// Advances the animation; returns true when it moved the offset. The
    /// layer steps during paint, after the container painted, so the caller
    /// needs one more frame to show the offset set here (also the last one).
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
        }
        true
    }

    /// The scrollbar gutter, as the container's right padding. Chromium
    /// reserves it only while the content overflows (`overflow-y: auto`).
    pub fn gutter(&self) -> gpui::Pixels {
        gpui::px(if self.overflowing { SCROLLBAR_GUTTER } else { 0.0 })
    }

    /// Starts a smooth scroll by `delta` px from the current target. Returns
    /// false at a boundary, where the scroll belongs to the next scroller.
    fn scroll_by(&mut self, handle: &gpui::ScrollHandle, delta: f32) -> bool {
        let max = f32::from(handle.max_offset().height).max(0.0);
        let current = -f32::from(handle.offset().y);
        let base = self.target.map(|t| -t).unwrap_or(current);
        let next = (base + delta).clamp(0.0, max);
        if (next - base).abs() < 0.01 {
            return false;
        }
        self.from = -current;
        self.target = Some(-next);
        self.start = Some(Instant::now());
        true
    }
}

/// The scrollbar parts in the layer bounds. `None` while nothing overflows.
struct ScrollbarGeometry {
    gutter: gpui::Bounds<gpui::Pixels>,
    track_top: f32,
    track_len: f32,
    thumb: gpui::Bounds<gpui::Pixels>,
    max: f32,
}

impl ScrollbarGeometry {
    fn new(bounds: gpui::Bounds<gpui::Pixels>, handle: &gpui::ScrollHandle) -> Option<Self> {
        let max = f32::from(handle.max_offset().height);
        let view = f32::from(bounds.size.height);
        let track_len = view - 2.0 * SCROLLBAR_BUTTON;
        if max <= 0.0 || track_len <= SCROLLBAR_MIN_THUMB {
            return None;
        }
        let right = f32::from(bounds.right());
        let thumb_len = (track_len * view / (view + max)).clamp(SCROLLBAR_MIN_THUMB, track_len);
        let offset = (-f32::from(handle.offset().y)).clamp(0.0, max);
        let track_top = f32::from(bounds.top()) + SCROLLBAR_BUTTON;
        let thumb_top = track_top + (track_len - thumb_len) * offset / max;
        Some(Self {
            gutter: gpui::Bounds::new(
                gpui::point(gpui::px(right - SCROLLBAR_GUTTER), bounds.top()),
                gpui::size(gpui::px(SCROLLBAR_GUTTER), bounds.size.height),
            ),
            track_top,
            track_len,
            thumb: gpui::Bounds::new(
                gpui::point(gpui::px(right - SCROLLBAR_RIGHT_INSET - SCROLLBAR_THUMB), gpui::px(thumb_top)),
                gpui::size(gpui::px(SCROLLBAR_THUMB), gpui::px(thumb_len)),
            ),
            max,
        })
    }

    /// Scroll offset per pointer pixel along the track.
    fn offset_per_px(&self) -> f32 {
        let free = self.track_len - f32::from(self.thumb.size.height);
        if free > 0.0 { self.max / free } else { 0.0 }
    }

    /// The arrow triangle in one button: 6px wide, its tip 2.5px from the end.
    fn arrow(&self, up: bool) -> Option<gpui::Path<gpui::Pixels>> {
        let center = f32::from(self.thumb.left()) + SCROLLBAR_THUMB / 2.0;
        let half = SCROLLBAR_THUMB / 2.0;
        let (tip, base) = if up {
            let top = self.track_top - SCROLLBAR_BUTTON;
            (top + 2.5, top + 7.25)
        } else {
            let bottom = self.track_top + self.track_len + SCROLLBAR_BUTTON;
            (bottom - 2.5, bottom - 7.25)
        };
        let mut path = gpui::PathBuilder::fill();
        path.move_to(gpui::point(gpui::px(center), gpui::px(tip)));
        path.line_to(gpui::point(gpui::px(center + half), gpui::px(base)));
        path.line_to(gpui::point(gpui::px(center - half), gpui::px(base)));
        path.close();
        path.build().ok()
    }
}

/// The wheel and scrollbar layer for a scroll container. Place it after the
/// container as an absolute, full-size sibling inside a relative wrapper, and
/// give the container `WheelScroll::gutter` as right padding. `color` paints
/// the thumb and arrows (`scrollbar-color: var(--t-scroll) transparent`).
pub fn wheel_scroll_layer(
    handle: gpui::ScrollHandle,
    state: std::rc::Rc<std::cell::RefCell<WheelScroll>>,
    color: Rgba,
) -> impl IntoElement {
    use gpui::{
        DispatchPhase, HitboxBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta,
        ScrollWheelEvent, canvas,
    };
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            if state.borrow_mut().step(&handle) {
                window.request_animation_frame();
            }
            // The container used the last frame's gutter. When the overflow
            // changed, render again with the right one. This converges: the
            // gutter can only make the content taller.
            let overflowing = handle.max_offset().height > gpui::px(0.);
            if std::mem::replace(&mut state.borrow_mut().overflowing, overflowing) != overflowing {
                window.request_animation_frame();
            }

            let geometry = ScrollbarGeometry::new(bounds, &handle);
            if let Some(geometry) = &geometry {
                window.paint_quad(gpui::fill(geometry.thumb, hsla(color)).corner_radii(gpui::px(SCROLLBAR_THUMB / 2.0)));
                for up in [true, false] {
                    if let Some(arrow) = geometry.arrow(up) {
                        window.paint_path(arrow, hsla(color));
                    }
                }
            }

            {
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
                    // At the boundary the next scroller in the chain takes it.
                    if delta != 0.0 && state.borrow_mut().scroll_by(&handle, delta) {
                        cx.stop_propagation();
                        window.refresh();
                    }
                });
            }

            let Some(geometry) = geometry else {
                state.borrow_mut().drag = None;
                return;
            };
            {
                let handle = handle.clone();
                let state = state.clone();
                let ScrollbarGeometry { gutter, thumb, track_top, track_len, .. } = geometry;
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !gutter.contains(&event.position)
                    {
                        return;
                    }
                    cx.stop_propagation();
                    window.prevent_default();
                    let y = f32::from(event.position.y);
                    let mut state = state.borrow_mut();
                    if thumb.contains(&event.position) {
                        state.target = None;
                        state.start = None;
                        state.drag = Some((y, -f32::from(handle.offset().y)));
                        return;
                    }
                    let page = f32::from(gutter.size.height) * SCROLLBAR_PAGE_FRACTION;
                    let delta = if y < track_top {
                        -SCROLLBAR_ARROW_STEP
                    } else if y >= track_top + track_len {
                        SCROLLBAR_ARROW_STEP
                    } else if y < f32::from(thumb.top()) {
                        -page
                    } else {
                        page
                    };
                    if state.scroll_by(&handle, delta) {
                        window.refresh();
                    }
                });
            }
            {
                let handle = handle.clone();
                let state = state.clone();
                let per_px = geometry.offset_per_px();
                let max = geometry.max;
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let Some((from_y, from_offset)) = state.borrow().drag else { return };
                    if !event.dragging() {
                        state.borrow_mut().drag = None;
                        return;
                    }
                    cx.stop_propagation();
                    let next = (from_offset + (f32::from(event.position.y) - from_y) * per_px).clamp(0.0, max);
                    let offset = handle.offset();
                    handle.set_offset(gpui::point(offset.x, gpui::px(-next)));
                    window.refresh();
                });
            }
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture && state.borrow_mut().drag.take().is_some() {
                    cx.stop_propagation();
                }
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
