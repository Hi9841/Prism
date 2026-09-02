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
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject,
    GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, FONT_CHARSET,
    FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FONT_QUALITY, FW_SEMIBOLD,
    HGDIOBJ, TRANSPARENT,
};

#[inline]
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
    GetSystemMetrics, KillTimer, PostMessageW, RegisterClassW, SetTimer, SetWindowPos,
    ShowWindow, TranslateMessage, UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW,
    HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
    SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_APP, WM_DESTROY, WM_TIMER, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const OSD_WIDTH: i32 = 230;
const OSD_HEIGHT: i32 = 52;
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
    let bitmap = match CreateDIBSection(
        Some(mem_dc),
        &bmi,
        DIB_RGB_COLORS,
        &mut bits,
        None,
        0,
    ) {
        Ok(bm) => bm,
        Err(_) => {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(None, screen_dc);
            return;
        }
    };

    let old_bmp = SelectObject(mem_dc, bitmap.into());
    let pixel_slice = std::slice::from_raw_parts_mut(bits as *mut u32, (OSD_WIDTH * OSD_HEIGHT) as usize);

    // Draw anti-aliased dark translucent pill with subtle border into raw buffer
    draw_rounded_pill_buffer(pixel_slice, OSD_WIDTH, OSD_HEIGHT, state.percentage, state.muted);

    // Render text onto the DC using native Segoe UI font
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
        FONT_QUALITY(5),
        0,
        w!("Segoe UI Variable Text"),
    );

    let old_font = SelectObject(mem_dc, font_title.into());
    let _ = SetBkMode(mem_dc, TRANSPARENT);
    let _ = SetTextColor(mem_dc, rgb(255, 255, 255));

    // Title text: truncated if long
    let display_title = truncate_string(&state.title, 20);
    let mut wide_title: Vec<u16> = display_title.encode_utf16().collect();
    let mut title_rect = RECT {
        left: 36,
        top: 10,
        right: OSD_WIDTH - 50,
        bottom: 28,
    };
    windows::Win32::Graphics::Gdi::DrawTextW(
        mem_dc,
        &mut wide_title,
        &mut title_rect,
        windows::Win32::Graphics::Gdi::DT_LEFT | windows::Win32::Graphics::Gdi::DT_SINGLELINE,
    );

    // Percentage text
    let pct_text = if state.muted {
        "Muted".to_string()
    } else {
        format!("{}%", state.percentage)
    };
    let mut wide_pct: Vec<u16> = pct_text.encode_utf16().collect();
    let mut pct_rect = RECT {
        left: OSD_WIDTH - 60,
        top: 10,
        right: OSD_WIDTH - 16,
        bottom: 28,
    };
    let _ = SetTextColor(mem_dc, if state.muted { rgb(255, 120, 120) } else { rgb(200, 205, 215) });
    windows::Win32::Graphics::Gdi::DrawTextW(
        mem_dc,
        &mut wide_pct,
        &mut pct_rect,
        windows::Win32::Graphics::Gdi::DT_RIGHT | windows::Win32::Graphics::Gdi::DT_SINGLELINE,
    );

    // Ensure RGB premultiplication across text pixels where GDI drew
    fix_alpha_premultiplication(pixel_slice, OSD_WIDTH);

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

    let _ = SelectObject(mem_dc, old_font);
    let _ = DeleteObject(HGDIOBJ(font_title.0));
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

fn draw_rounded_pill_buffer(
    pixels: &mut [u32],
    width: i32,
    height: i32,
    percentage: u32,
    muted: bool,
) {
    let radius = 12.0f32;
    let w_f = width as f32;
    let h_f = height as f32;

    for y in 0..height {
        let y_f = y as f32 + 0.5;
        for x in 0..width {
            let x_f = x as f32 + 0.5;

            // Distance to rounded rectangle box
            let dx = (x_f - radius).min(0.0).abs().max((x_f - (w_f - radius)).max(0.0));
            let dy = (y_f - radius).min(0.0).abs().max((y_f - (h_f - radius)).max(0.0));
            let dist = (dx * dx + dy * dy).sqrt();

            if dist > radius {
                pixels[(y * width + x) as usize] = 0; // Transparent
                continue;
            }

            // Anti-aliased outer edge (1.0 px feather)
            let alpha_factor = (radius - dist + 0.5).clamp(0.0, 1.0);
            let is_border = dist >= radius - 1.2;

            // Dark glass background: RGBA(24, 24, 28, 235)
            let (r, g, b, a) = if is_border {
                (255u8, 255u8, 255u8, (38.0 * alpha_factor) as u8)
            } else {
                (22u8, 22u8, 26u8, (235.0 * alpha_factor) as u8)
            };

            // Premultiplied ARGB: (A << 24) | (R * A / 255 << 16) | (G * A / 255 << 8) | (B * A / 255)
            let pr = (r as u32 * a as u32) / 255;
            let pg = (g as u32 * a as u32) / 255;
            let pb = (b as u32 * a as u32) / 255;
            pixels[(y * width + x) as usize] = ((a as u32) << 24) | (pr << 16) | (pg << 8) | pb;
        }
    }

    // Draw speaker icon at (16, 14)
    draw_speaker_icon(pixels, width, 16, 14, muted, percentage);

    // Draw bottom progress bar at x=16..width-16, y=34..38 (height=4px, rounded ends)
    draw_progress_bar(pixels, width, 16, width - 16, 34, 38, percentage, muted);
}

fn draw_speaker_icon(pixels: &mut [u32], stride: i32, ox: i32, oy: i32, muted: bool, percentage: u32) {
    let color = if muted {
        0xFFFF7070 // Reddish muted
    } else {
        0xFFE0E5F0 // Clean light-blue white
    };

    // Draw a small 12x12 speaker shape
    for y in 0..12 {
        for x in 0..12 {
            let is_speaker = (x < 3 && (3..=8).contains(&y))
                || (x >= 3 && x <= 6 && y >= 5 - x && y <= 6 + x);
            if is_speaker {
                let idx = ((oy + y) * stride + (ox + x)) as usize;
                if idx < pixels.len() {
                    pixels[idx] = color;
                }
            }
        }
    }

    // Sound waves
    if !muted && percentage > 0 {
        // Wave 1
        for y in 3..=8 {
            let idx = ((oy + y) * stride + (ox + 8)) as usize;
            if idx < pixels.len() {
                pixels[idx] = 0xAAE0E5F0;
            }
        }
        // Wave 2
        if percentage > 40 {
            for y in 2..=9 {
                let idx = ((oy + y) * stride + (ox + 11)) as usize;
                if idx < pixels.len() {
                    pixels[idx] = 0x80E0E5F0;
                }
            }
        }
    } else if muted {
        // Slash over speaker
        for i in 0..10 {
            let idx = ((oy + 1 + i) * stride + (ox + 2 + i)) as usize;
            if idx < pixels.len() {
                pixels[idx] = 0xFFFF4444;
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
) {
    let bar_width = x_end - x_start;
    let fill_width = (bar_width as f32 * (percentage as f32 / 100.0)).round() as i32;

    let track_color = 0x33FFFFFF; // 20% white track
    let fill_color = if muted {
        0xCCFF6666
    } else {
        0xFF8A8FFF // Iris/accent tint
    };

    for y in y_start..=y_end {
        for x in x_start..=x_end {
            let idx = (y * stride + x) as usize;
            if idx < pixels.len() {
                if x - x_start <= fill_width {
                    pixels[idx] = fill_color;
                } else {
                    pixels[idx] = track_color;
                }
            }
        }
    }
}

fn fix_alpha_premultiplication(pixels: &mut [u32], width: i32) {
    // Ensure text drawn by GDI doesn't have 0 alpha where pixels exist
    for y in 8..30 {
        for x in 34..(width - 12) {
            let idx = (y * width + x) as usize;
            let val = pixels[idx];
            let alpha = (val >> 24) & 0xFF;
            let rgb = val & 0x00FFFFFF;
            if rgb > 0 && alpha < 180 {
                pixels[idx] = (0xFF << 24) | rgb;
            }
        }
    }
}
