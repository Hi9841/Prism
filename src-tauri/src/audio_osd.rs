//! Native Win32 floating on-screen display (OSD) HUD for volume feedback.
//!
//! Creates a lightweight layered, transparent pill above the taskbar that shows
//! the application name, volume percentage, and a smooth level bar.
//! Runs with zero latency, never steals window focus (WS_EX_NOACTIVATE), and
//! automatically fades out after 1.2s of inactivity.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, FONT_CHARSET, FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FONT_QUALITY,
    FW_SEMIBOLD, HGDIOBJ, TRANSPARENT,
};

#[inline]
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics, KillTimer,
    PostMessageW, RegisterClassW, SetTimer, SetWindowPos, ShowWindow, TranslateMessage,
    UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_APP, WM_DESTROY, WM_TIMER, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const OSD_WIDTH: i32 = 244;
const OSD_HEIGHT: i32 = 56;
const TIMER_HIDE: usize = 1;
const HIDE_DELAY_MS: u32 = 1200;
const WM_UPDATE_OSD: u32 = WM_APP + 50;

static OSD_HWND: AtomicIsize = AtomicIsize::new(0);
static OSD_THREAD_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
struct OsdState {
    title: String,
    percentage: u32,
    muted: bool,
    point: POINT,
}

static LATEST_STATE: Mutex<Option<OsdState>> = Mutex::new(None);

pub fn show(title: &str, percentage: u32, muted: bool, cursor: POINT) {
    ensure_osd_thread();

    let state = OsdState {
        title: title.to_string(),
        percentage: percentage.min(100),
        muted,
        point: cursor,
    };

    if let Ok(mut lock) = LATEST_STATE.lock() {
        *lock = Some(state);
    }

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
    std::thread::spawn(osd_thread_proc);
}

fn osd_thread_proc() {
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
            Err(_) => return,
        };

        OSD_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        OSD_HWND.store(0, Ordering::SeqCst);
    }
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
    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);

    // Position horizontally centered above cursor, clamped to screen edges
    let mut x = state.point.x - (OSD_WIDTH / 2);
    x = x.clamp(12, screen_w - OSD_WIDTH - 12);

    // Position vertically above taskbar
    let mut y = state.point.y - OSD_HEIGHT - 16;
    if y < 20 {
        y = (screen_h - OSD_HEIGHT - 64).max(20);
    }

    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        x,
        y,
        OSD_WIDTH,
        OSD_HEIGHT,
        SWP_NOACTIVATE,
    );

    // Render 32-bit ARGB DIB with rounded pill background and typography
    render_osd_surface(hwnd, state);
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
}

unsafe fn render_osd_surface(hwnd: HWND, state: &OsdState) {
    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(Some(screen_dc));

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: OSD_WIDTH,
            biHeight: -OSD_HEIGHT, // Top-down DIB
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
    let pixel_slice =
        std::slice::from_raw_parts_mut(bits as *mut u32, (OSD_WIDTH * OSD_HEIGHT) as usize);

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
        16,
        OSD_WIDTH - 16,
        37,
        42,
        state.percentage,
        state.muted,
        state.title.ends_with("(No Audio)"),
    );

    // 4. Render and composite typography with pure grayscale antialiasing (no ClearType chromatic fringe)
    render_typography(pixel_slice, OSD_WIDTH, OSD_HEIGHT, state);

    let mut pt_src = POINT { x: 0, y: 0 };
    let mut size_wnd = SIZE {
        cx: OSD_WIDTH,
        cy: OSD_HEIGHT,
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
        Some(&mut size_wnd),
        Some(mem_dc),
        Some(&mut pt_src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );

    let _ = SelectObject(mem_dc, old_bmp);
    let _ = DeleteObject(bitmap.into());
    let _ = DeleteDC(mem_dc);
    let _ = ReleaseDC(None, screen_dc);
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
    let dst_a = ((dst_premul >> 24) & 0xFF) as u32;
    let dst_pr = ((dst_premul >> 16) & 0xFF) as u32;
    let dst_pg = ((dst_premul >> 8) & 0xFF) as u32;
    let dst_pb = (dst_premul & 0xFF) as u32;

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
                    if px >= 1.5 && px <= 4.5 && py >= 5.5 && py <= 10.5 {
                        inside = true;
                    }

                    // 2. Speaker cone (x: 4.0..8.5, flaring from 5.5..10.5 to 2.5..13.5)
                    if px >= 4.0 && px <= 8.5 {
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
                            && px >= 2.0
                            && px <= 14.0
                            && py >= 2.0
                            && py <= 14.0
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
    x_start: i32,
    x_end: i32,
    y_start: i32,
    y_end: i32,
    percentage: u32,
    muted: bool,
    is_no_audio: bool,
) {
    let radius = 2.5f32;
    let y_center = (y_start as f32 + y_end as f32) * 0.5;
    let x_left = x_start as f32 + radius;
    let x_right = x_end as f32 - radius;

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

    for y in y_start..=y_end {
        let y_f = y as f32 + 0.5;
        for x in x_start..=x_end {
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
