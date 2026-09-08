//! Native Win32 floating on-screen display (OSD) HUD for volume feedback.
//!
//! Creates a lightweight layered, transparent pill above the taskbar that shows
//! the application name, volume percentage, and a smooth level bar.
//! Runs with zero latency, never steals window focus (WS_EX_NOACTIVATE), and
//! automatically fades out after 1.2s of inactivity.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::Mutex;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GetDC,
    GetMonitorInfoW, MonitorFromPoint, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, FONT_CHARSET,
    FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FONT_QUALITY, FW_SEMIBOLD, HGDIOBJ, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;

#[inline]
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, KillTimer, PostMessageW,
    RegisterClassW, SetTimer, SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow,
    CS_HREDRAW, CS_VREDRAW, HWND_TOPMOST, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA,
    WM_APP, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

const OSD_WIDTH: i32 = 244;
const OSD_HEIGHT: i32 = 56;
const TIMER_HIDE: usize = 1;
const HIDE_DELAY_MS: u32 = 1200;
const WM_UPDATE_OSD: u32 = WM_APP + 50;

static OSD_HWND: AtomicIsize = AtomicIsize::new(0);
static OSD_THREAD_INITIALIZED: AtomicBool = AtomicBool::new(false);
static OSD_REQUEST_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
struct OsdState {
    title: String,
    percentage: u32,
    muted: bool,
    point: POINT,
}

static LATEST_STATE: Mutex<Option<OsdState>> = Mutex::new(None);

pub fn show(title: &str, percentage: u32, muted: bool, cursor: POINT) {
    let state = OsdState {
        title: title.to_string(),
        percentage: percentage.min(100),
        muted,
        point: cursor,
    };

    if let Ok(mut lock) = LATEST_STATE.lock() {
        *lock = Some(state);
    }
    OSD_REQUEST_GENERATION.fetch_add(1, Ordering::SeqCst);
    ensure_osd_thread();

    let h = OSD_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let hwnd = HWND(h as *mut _);
            let _ = PostMessageW(Some(hwnd), WM_UPDATE_OSD, WPARAM(0), LPARAM(0));
        }
    }
}

fn ensure_osd_thread() {
    if OSD_THREAD_INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    let start_generation = OSD_REQUEST_GENERATION.load(Ordering::SeqCst);
    std::thread::spawn(move || osd_thread_proc(start_generation));
}

fn osd_thread_proc(start_generation: u64) {
    unsafe {
        let class_name = w!("PrismVolumeOsdClass");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(osd_wnd_proc),
            hInstance: windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .unwrap_or_default()
                .into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class_name,
            w!("Prism Volume OSD"),
            WS_POPUP,
            0,
            0,
            OSD_WIDTH,
            OSD_HEIGHT,
            None,
            None,
            Some(wc.hInstance),
            None,
        );

        let hwnd = match hwnd {
            Ok(h) => h,
            Err(_) => {
                finish_failed_osd_thread(start_generation);
                return;
            }
        };

        OSD_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        let _ = PostMessageW(Some(hwnd), WM_UPDATE_OSD, WPARAM(0), LPARAM(0));

        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        OSD_HWND.store(0, Ordering::SeqCst);
        OSD_THREAD_INITIALIZED.store(false, Ordering::SeqCst);
    }
}

fn finish_failed_osd_thread(start_generation: u64) {
    OSD_THREAD_INITIALIZED.store(false, Ordering::SeqCst);
    if should_retry_osd_thread(
        start_generation,
        OSD_REQUEST_GENERATION.load(Ordering::SeqCst),
    ) {
        ensure_osd_thread();
    }
}

fn should_retry_osd_thread(start_generation: u64, latest_generation: u64) -> bool {
    latest_generation != start_generation
}

unsafe extern "system" fn osd_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_UPDATE_OSD => {
            let state = {
                let lock = LATEST_STATE.lock().ok();
                lock.and_then(|guard| guard.clone())
            };
            if let Some(state) = state {
                render_and_position(hwnd, &state);
                let _ = SetTimer(Some(hwnd), TIMER_HIDE, HIDE_DELAY_MS, None);
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_HIDE => {
            let _ = KillTimer(Some(hwnd), TIMER_HIDE);
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_HIDE);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn render_and_position(hwnd: HWND, state: &OsdState) {
    let monitor = MonitorFromPoint(state.point, MONITOR_DEFAULTTONEAREST);
    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
        return;
    }

    // Place the window on the target monitor before asking Windows for that window's DPI.
    let provisional_position = position_in_work_area(
        Rect::from(monitor_info.rcWork),
        state.point,
        OSD_WIDTH,
        OSD_HEIGHT,
        96,
    );
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        provisional_position.x,
        provisional_position.y,
        OSD_WIDTH,
        OSD_HEIGHT,
        SWP_NOACTIVATE,
    );
    let dpi = GetDpiForWindow(hwnd).max(96);
    let width = scale_for_dpi(OSD_WIDTH, dpi);
    let height = scale_for_dpi(OSD_HEIGHT, dpi);
    let position = position_in_work_area(
        Rect::from(monitor_info.rcWork),
        state.point,
        width,
        height,
        dpi,
    );

    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        position.x,
        position.y,
        width,
        height,
        SWP_NOACTIVATE,
    );

    // Render 32-bit ARGB DIB with rounded pill background and typography
    render_osd_surface(hwnd, state, width, height);
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl From<RECT> for Rect {
    fn from(value: RECT) -> Self {
        Self {
            left: value.left,
            top: value.top,
            right: value.right,
            bottom: value.bottom,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OsdPosition {
    x: i32,
    y: i32,
}

fn scale_for_dpi(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}

fn position_in_work_area(
    work_area: Rect,
    point: POINT,
    width: i32,
    height: i32,
    dpi: u32,
) -> OsdPosition {
    let margin = scale_for_dpi(12, dpi);
    let offset = scale_for_dpi(16, dpi);
    let min_x = work_area.left + margin;
    let max_x = (work_area.right - width - margin).max(min_x);
    let min_y = work_area.top + margin;
    let max_y = (work_area.bottom - height - margin).max(min_y);

    let (x, y) = if point.x < work_area.left {
        (work_area.left + offset, point.y - height / 2)
    } else if point.x >= work_area.right {
        (work_area.right - width - offset, point.y - height / 2)
    } else if point.y < work_area.top {
        (point.x - width / 2, work_area.top + offset)
    } else {
        (point.x - width / 2, work_area.bottom - height - offset)
    };

    OsdPosition {
        x: x.clamp(min_x, max_x),
        y: y.clamp(min_y, max_y),
    }
}

unsafe fn render_osd_surface(hwnd: HWND, state: &OsdState, width: i32, height: i32) {
    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(Some(screen_dc));

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // Top-down DIB
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let bitmap = match CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
        Ok(bm) => bm,
        Err(_) => {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(None, screen_dc);
            return;
        }
    };

    let old_bmp = SelectObject(mem_dc, bitmap.into());
    let pixel_slice = std::slice::from_raw_parts_mut(bits as *mut u32, (width * height) as usize);

    let mut logical_pixels = vec![0; (OSD_WIDTH * OSD_HEIGHT) as usize];
    render_osd_pixels(&mut logical_pixels, state);
    scale_pixels(
        &logical_pixels,
        OSD_WIDTH,
        OSD_HEIGHT,
        pixel_slice,
        width,
        height,
    );

    let pt_src = POINT { x: 0, y: 0 };
    let size_wnd = SIZE {
        cx: width,
        cy: height,
    };
    let blend = BLENDFUNCTION {
        BlendOp: 0, // AC_SRC_OVER
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: 1, // AC_SRC_ALPHA
    };

    let _ = UpdateLayeredWindow(
        hwnd,
        Some(screen_dc),
        None,
        Some(&size_wnd),
        Some(mem_dc),
        Some(&pt_src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );

    let _ = SelectObject(mem_dc, old_bmp);
    let _ = DeleteObject(bitmap.into());
    let _ = DeleteDC(mem_dc);
    let _ = ReleaseDC(None, screen_dc);
}

unsafe fn render_osd_pixels(pixel_slice: &mut [u32], state: &OsdState) {
    // 1. Draw Liquid Glass Pill background (concentric 16px radius, subtle vertical gradient, specular highlight rim)
    draw_liquid_glass_pill(pixel_slice, OSD_WIDTH, OSD_HEIGHT);

    // 2. Draw Anti-Aliased Vector Speaker Icon
    draw_speaker_icon(
        pixel_slice,
        OSD_WIDTH,
        16,
        11,
        state.muted,
        state.percentage,
    );

    // 3. Draw Capsule Progress Bar
    draw_progress_bar(
        pixel_slice,
        OSD_WIDTH,
        Rect {
            left: 16,
            top: 37,
            right: OSD_WIDTH - 16,
            bottom: 42,
        },
        state.percentage,
        state.muted,
        state.title.ends_with("(No Audio)"),
    );

    // 4. Render and composite typography with pure grayscale antialiasing (no ClearType chromatic fringe)
    render_typography(pixel_slice, OSD_WIDTH, OSD_HEIGHT, state);
}

fn scale_pixels(
    source: &[u32],
    source_width: i32,
    source_height: i32,
    destination: &mut [u32],
    destination_width: i32,
    destination_height: i32,
) {
    for y in 0..destination_height {
        let source_y = y * source_height / destination_height;
        for x in 0..destination_width {
            let source_x = x * source_width / destination_width;
            destination[(y * destination_width + x) as usize] =
                source[(source_y * source_width + source_x) as usize];
        }
    }
}

fn truncate_string(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars - 1).collect();
        format!("{truncated}…")
    }
}

/// Porter-Duff Over operator on premultiplied destination ARGB
#[inline]
fn blend_over(dst_premul: u32, src_rgb: (u8, u8, u8), src_alpha: u8) -> u32 {
    if src_alpha == 0 {
        return dst_premul;
    }
    let dst_a = (dst_premul >> 24) & 0xFF;
    let dst_pr = (dst_premul >> 16) & 0xFF;
    let dst_pg = (dst_premul >> 8) & 0xFF;
    let dst_pb = dst_premul & 0xFF;

    let sa = src_alpha as u32;
    let sr = src_rgb.0 as u32;
    let sg = src_rgb.1 as u32;
    let sb = src_rgb.2 as u32;

    let spr = (sr * sa) / 255;
    let spg = (sg * sa) / 255;
    let spb = (sb * sa) / 255;

    let inv_sa = 255 - sa;
    let out_pr = (spr + (dst_pr * inv_sa) / 255).min(255);
    let out_pg = (spg + (dst_pg * inv_sa) / 255).min(255);
    let out_pb = (spb + (dst_pb * inv_sa) / 255).min(255);
    let out_a = (sa + (dst_a * inv_sa) / 255).min(255);

    (out_a << 24) | (out_pr << 16) | (out_pg << 8) | out_pb
}

#[inline]
fn pack_premultiplied(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let pr = ((r as u32) * (a as u32)) / 255;
    let pg = ((g as u32) * (a as u32)) / 255;
    let pb = ((b as u32) * (a as u32)) / 255;
    ((a as u32) << 24) | (pr << 16) | (pg << 8) | pb
}

fn draw_liquid_glass_pill(pixels: &mut [u32], width: i32, height: i32) {
    let radius = 16.0f32;
    let w_f = width as f32;
    let h_f = height as f32;

    for y in 0..height {
        let y_f = y as f32 + 0.5;
        for x in 0..width {
            let x_f = x as f32 + 0.5;

            let dx = (x_f - radius)
                .min(0.0)
                .abs()
                .max((x_f - (w_f - radius)).max(0.0));
            let dy = (y_f - radius)
                .min(0.0)
                .abs()
                .max((y_f - (h_f - radius)).max(0.0));
            let dist = (dx * dx + dy * dy).sqrt();

            if dist > radius + 0.5 {
                pixels[(y * width + x) as usize] = 0;
                continue;
            }

            let edge_alpha = (radius - dist + 0.5).clamp(0.0, 1.0);

            // Subtle vertical gradient for physical depth (dark obsidian glass)
            let t = y_f / h_f;
            let base_r = (24.0 * (1.0 - t * 0.25)) as u8;
            let base_g = (24.0 * (1.0 - t * 0.25)) as u8;
            let base_b = (30.0 * (1.0 - t * 0.25)) as u8;
            let base_a = (235.0 * edge_alpha) as u8;

            let mut pix = pack_premultiplied(base_r, base_g, base_b, base_a);

            // Specular glass rim (1.0px inner light border)
            if dist >= radius - 1.2 && dist <= radius {
                let rim_top = y_f < h_f * 0.5;
                let rim_intensity = if rim_top { 48.0 } else { 18.0 } * edge_alpha;
                pix = blend_over(pix, (255, 255, 255), rim_intensity as u8);
            }

            pixels[(y * width + x) as usize] = pix;
        }
    }
}

fn draw_speaker_icon(
    pixels: &mut [u32],
    stride: i32,
    ox: i32,
    oy: i32,
    muted: bool,
    percentage: u32,
) {
    let icon_color = if muted {
        (248u8, 113u8, 113u8) // Rose Coral
    } else {
        (226u8, 232u8, 240u8) // Slate 200
    };

    for ly in 0..16 {
        for lx in 0..16 {
            let mut hits = 0u32;
            for sy in 0..4 {
                let py = ly as f32 + (sy as f32 + 0.5) / 4.0;
                for sx in 0..4 {
                    let px = lx as f32 + (sx as f32 + 0.5) / 4.0;
                    let mut inside = false;

                    // 1. Speaker base box (x: 1.5..4.5, y: 5.5..10.5)
                    if (1.5..=4.5).contains(&px) && (5.5..=10.5).contains(&py) {
                        inside = true;
                    }

                    // 2. Speaker cone (x: 4.0..8.5, flaring from 5.5..10.5 to 2.5..13.5)
                    if (4.0..=8.5).contains(&px) {
                        let progress = (px - 4.0) / 4.5;
                        let top_y = 5.5 - progress * 3.0;
                        let bot_y = 10.5 + progress * 3.0;
                        if py >= top_y && py <= bot_y {
                            inside = true;
                        }
                    }

                    if muted {
                        // Diagonal slash: from (2.5, 2.5) to (13.5, 13.5)
                        let dist_to_line = ((px - py).abs()) / 1.414;
                        if dist_to_line <= 0.85
                            && (2.0..=14.0).contains(&px)
                            && (2.0..=14.0).contains(&py)
                        {
                            inside = true;
                        } else if dist_to_line < 1.6 && inside {
                            // Clean cutout gap behind the slash
                            inside = false;
                        }
                    } else if percentage > 0 {
                        // Wave 1 arc
                        let dx1 = px - 4.5;
                        let dy1 = py - 8.0;
                        let r1 = (dx1 * dx1 + dy1 * dy1).sqrt();
                        if (6.2..=7.8).contains(&r1) && dy1.abs() <= dx1 * 1.05 && px > 6.0 {
                            inside = true;
                        }

                        // Wave 2 arc (if volume > 35%)
                        if percentage > 35 {
                            let r2 = r1;
                            if (9.6..=11.2).contains(&r2) && dy1.abs() <= dx1 * 0.95 && px > 8.0 {
                                inside = true;
                            }
                        }
                    }

                    if inside {
                        hits += 1;
                    }
                }
            }

            if hits > 0 {
                let cov = ((hits as f32 / 16.0) * 255.0) as u8;
                let idx = ((oy + ly) * stride + (ox + lx)) as usize;
                if idx < pixels.len() {
                    pixels[idx] = blend_over(pixels[idx], icon_color, cov);
                }
            }
        }
    }
}

fn draw_progress_bar(
    pixels: &mut [u32],
    stride: i32,
    bounds: Rect,
    percentage: u32,
    muted: bool,
    is_no_audio: bool,
) {
    let radius = 2.5f32;
    let y_center = (bounds.top as f32 + bounds.bottom as f32) * 0.5;
    let x_left = bounds.left as f32 + radius;
    let x_right = bounds.right as f32 - radius;

    let fill_total_w = x_right - x_left;
    let fill_w = (fill_total_w * (percentage as f32 / 100.0)).clamp(0.0, fill_total_w);
    let fill_right = x_left + fill_w;

    let fill_color = if muted {
        (248u8, 113u8, 113u8) // Rose Coral
    } else if is_no_audio {
        (100u8, 116u8, 139u8) // Muted Slate
    } else {
        (56u8, 189u8, 248u8) // Luminous Sky Blue / Iris
    };

    for y in bounds.top..=bounds.bottom {
        let y_f = y as f32 + 0.5;
        for x in bounds.left..=bounds.right {
            let x_f = x as f32 + 0.5;
            let idx = (y * stride + x) as usize;
            if idx >= pixels.len() {
                continue;
            }

            // Distance to capsule track
            let px = x_f.clamp(x_left, x_right);
            let py = y_center;
            let dx = x_f - px;
            let dy = y_f - py;
            let track_dist = (dx * dx + dy * dy).sqrt();

            if track_dist <= radius + 0.5 {
                let track_alpha = (radius - track_dist + 0.5).clamp(0.0, 1.0);
                // Subtle 14% white translucent track (premultiplied: no blow-out!)
                pixels[idx] = blend_over(pixels[idx], (255, 255, 255), (35.0 * track_alpha) as u8);

                // Fill with rounded tip
                if fill_w > 0.0 {
                    let f_px = x_f.clamp(x_left, fill_right);
                    let f_dx = x_f - f_px;
                    let f_dist = (f_dx * f_dx + dy * dy).sqrt();
                    if f_dist <= radius + 0.5 {
                        let fill_alpha = (radius - f_dist + 0.5).clamp(0.0, 1.0);
                        pixels[idx] =
                            blend_over(pixels[idx], fill_color, (255.0 * fill_alpha) as u8);
                    }
                }
            }
        }
    }
}

unsafe fn render_typography(pixels: &mut [u32], width: i32, _height: i32, state: &OsdState) {
    let screen_dc = GetDC(None);
    let text_dc = CreateCompatibleDC(Some(screen_dc));

    let text_h = 28;
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -text_h, // Top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let Ok(bitmap) = CreateDIBSection(Some(text_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
    else {
        let _ = DeleteDC(text_dc);
        let _ = ReleaseDC(None, screen_dc);
        return;
    };

    let old_bmp = SelectObject(text_dc, bitmap.into());
    let text_slice = std::slice::from_raw_parts_mut(bits as *mut u32, (width * text_h) as usize);
    text_slice.fill(0);

    // Font: Segoe UI Variable Text / Segoe UI with ANTIALIASED_QUALITY (pure grayscale, 0 color fringe)
    let font_title = CreateFontW(
        -13,
        0,
        0,
        0,
        FW_SEMIBOLD.0 as i32,
        0,
        0,
        0,
        FONT_CHARSET(0),
        FONT_OUTPUT_PRECISION(0),
        FONT_CLIP_PRECISION(0),
        FONT_QUALITY(4), // ANTIALIASED_QUALITY
        0,
        w!("Segoe UI Variable Text"),
    );

    let old_font = SelectObject(text_dc, font_title.into());
    let _ = SetBkMode(text_dc, TRANSPARENT);
    let _ = SetTextColor(text_dc, rgb(255, 255, 255));

    let is_no_audio = state.title.ends_with("(No Audio)");
    let clean_title = if is_no_audio {
        state.title.trim_end_matches(" (No Audio)").trim()
    } else {
        &state.title
    };

    // 1. Draw Title text
    let display_title = truncate_string(clean_title, if is_no_audio { 15 } else { 19 });
    let mut wide_title: Vec<u16> = display_title.encode_utf16().collect();
    let mut title_rect = RECT {
        left: 38,
        top: 10,
        right: if is_no_audio { width - 82 } else { width - 58 },
        bottom: 27,
    };
    windows::Win32::Graphics::Gdi::DrawTextW(
        text_dc,
        &mut wide_title,
        &mut title_rect,
        windows::Win32::Graphics::Gdi::DT_LEFT
            | windows::Win32::Graphics::Gdi::DT_SINGLELINE
            | windows::Win32::Graphics::Gdi::DT_VCENTER,
    );

    // Composite Title with pure crisp white (255, 255, 255)
    for ty in 8..28 {
        for tx in 36..(width - 50) {
            let t_idx = (ty * width + tx) as usize;
            let p_idx = (ty * width + tx) as usize;
            if t_idx < text_slice.len() && p_idx < pixels.len() {
                let cov = (text_slice[t_idx] & 0xFF) as u8;
                if cov > 0 {
                    pixels[p_idx] = blend_over(pixels[p_idx], (255, 255, 255), cov);
                }
            }
        }
    }

    // 2. Clear buffer for Percentage/Status text
    text_slice.fill(0);

    let (pct_text, text_color) = if is_no_audio {
        ("No Audio".to_string(), (148u8, 163u8, 184u8)) // Slate-400
    } else if state.muted {
        ("Muted".to_string(), (248u8, 113u8, 113u8)) // Rose Coral
    } else {
        (format!("{}%", state.percentage), (203u8, 213u8, 225u8)) // Slate-300
    };

    let mut wide_pct: Vec<u16> = pct_text.encode_utf16().collect();
    let mut pct_rect = RECT {
        left: if is_no_audio { width - 80 } else { width - 62 },
        top: 10,
        right: width - 16,
        bottom: 27,
    };
    windows::Win32::Graphics::Gdi::DrawTextW(
        text_dc,
        &mut wide_pct,
        &mut pct_rect,
        windows::Win32::Graphics::Gdi::DT_RIGHT
            | windows::Win32::Graphics::Gdi::DT_SINGLELINE
            | windows::Win32::Graphics::Gdi::DT_VCENTER,
    );

    // Composite Percentage/Status text
    for ty in 8..28 {
        for tx in (width - 86)..(width - 14) {
            let t_idx = (ty * width + tx) as usize;
            let p_idx = (ty * width + tx) as usize;
            if t_idx < text_slice.len() && p_idx < pixels.len() {
                let cov = (text_slice[t_idx] & 0xFF) as u8;
                if cov > 0 {
                    pixels[p_idx] = blend_over(pixels[p_idx], text_color, cov);
                }
            }
        }
    }

    let _ = SelectObject(text_dc, old_font);
    let _ = DeleteObject(HGDIOBJ(font_title.0));
    let _ = SelectObject(text_dc, old_bmp);
    let _ = DeleteObject(bitmap.into());
    let _ = DeleteDC(text_dc);
    let _ = ReleaseDC(None, screen_dc);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK_AREA: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn positions_osd_inside_each_taskbar_edge() {
        assert_eq!(
            position_in_work_area(
                Rect {
                    left: -1880,
                    top: 0,
                    right: 0,
                    bottom: 1040,
                },
                POINT { x: -1900, y: 520 },
                OSD_WIDTH,
                OSD_HEIGHT,
                96,
            ),
            OsdPosition { x: -1864, y: 492 }
        );
        assert_eq!(
            position_in_work_area(
                Rect {
                    left: 1920,
                    top: 0,
                    right: 3800,
                    bottom: 1040,
                },
                POINT { x: 3820, y: 520 },
                OSD_WIDTH,
                OSD_HEIGHT,
                96,
            ),
            OsdPosition { x: 3540, y: 492 }
        );
        assert_eq!(
            position_in_work_area(
                Rect {
                    top: 40,
                    ..WORK_AREA
                },
                POINT { x: 960, y: 10 },
                OSD_WIDTH,
                OSD_HEIGHT,
                96,
            ),
            OsdPosition { x: 838, y: 56 }
        );
        assert_eq!(
            position_in_work_area(
                WORK_AREA,
                POINT { x: 960, y: 1060 },
                OSD_WIDTH,
                OSD_HEIGHT,
                96
            ),
            OsdPosition { x: 838, y: 968 }
        );
    }

    #[test]
    fn dimensions_and_offsets_scale_for_monitor_dpi() {
        let width = scale_for_dpi(OSD_WIDTH, 144);
        let height = scale_for_dpi(OSD_HEIGHT, 144);
        assert_eq!((width, height), (366, 84));
        assert_eq!(
            position_in_work_area(WORK_AREA, POINT { x: 960, y: 1060 }, width, height, 144),
            OsdPosition { x: 777, y: 932 }
        );
    }

    #[test]
    fn failed_start_retries_only_when_a_new_request_arrived() {
        assert!(!should_retry_osd_thread(4, 4));
        assert!(should_retry_osd_thread(4, 5));
    }

    #[test]
    fn pixel_scaling_fills_the_requested_surface() {
        let source = [1, 2, 3, 4];
        let mut destination = [0; 16];
        scale_pixels(&source, 2, 2, &mut destination, 4, 4);
        assert_eq!(destination[0], 1);
        assert_eq!(destination[3], 2);
        assert_eq!(destination[12], 3);
        assert_eq!(destination[15], 4);
    }
}
