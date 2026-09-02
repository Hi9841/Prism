#![allow(non_snake_case)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};

type Hhook = *mut c_void;
type Hwnd = *mut c_void;
type Hbitmap = *mut c_void;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}

#[repr(C)]
struct MouseHookStruct {
    point: Point,
    window: Hwnd,
    hit_test_code: u32,
    extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pixels_per_meter: i32,
    y_pixels_per_meter: i32,
    colors_used: u32,
    colors_important: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RgbQuad {
    blue: u8,
    green: u8,
    red: u8,
    reserved: u8,
}

#[repr(C)]
#[derive(Default)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [RgbQuad; 1],
}

const HC_ACTION: i32 = 0;
const HWND_MESSAGE: Hwnd = -3isize as Hwnd;
const WM_NULL: u32 = 0;
const WM_SYSCOMMAND: u32 = 0x0112;
const WM_LBUTTONDOWN: usize = 0x0201;
const WM_LBUTTONUP: usize = 0x0202;
const WM_CLOSE: u32 = 0x0010;
const WM_SETTINGCHANGE: u32 = 0x001A;
const STM_SETIMAGE: u32 = 0x0172;
const STM_GETIMAGE: u32 = 0x0173;
const IMAGE_BITMAP: usize = 0;
const SC_TASKLIST: usize = 0xF130;
const CONTROL_DISABLE_WIN_HOTKEY: usize = 1;
const EVENT_HOTKEY_DISABLED: usize = 2;
const CONTROL_START_RECT_LEFT: usize = 4;
const CONTROL_START_RECT_TOP: usize = 5;
const CONTROL_START_RECT_RIGHT: usize = 6;
const CONTROL_START_RECT_BOTTOM: usize = 7;
const EVENT_START_RECT_CONFIGURED: usize = 8;
const EVENT_TASKBAR_START_CLICK_X: usize = 9;
const EVENT_TASKBAR_START_CLICK_Y: usize = 10;
const CONTROL_START_ICON_REFRESH: usize = 11;
const CONTROL_START_ICON_SHUTDOWN: usize = 12;
const EVENT_START_ICON_SHUTDOWN: usize = 13;
const EVENT_START_ICON_REFRESHED: usize = 14;
const CONTROL_SEARCH_RECT_LEFT: usize = 15;
const CONTROL_SEARCH_RECT_TOP: usize = 16;
const CONTROL_SEARCH_RECT_RIGHT: usize = 17;
const CONTROL_SEARCH_RECT_BOTTOM: usize = 18;
const EVENT_SEARCH_RECT_CONFIGURED: usize = 19;
const CONTROL_TASKBAR_PIN: usize = 20;
const CONTROL_TASKBAR_UNPIN: usize = 21;
const EVENT_TASKBAR_PIN_COMPLETED: usize = 22;
const WS_POPUP: u32 = 0x8000_0000;
const SS_BITMAP: u32 = 0x0000_000e;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;
const HWND_TOPMOST: Hwnd = -1isize as Hwnd;
const DIB_RGB_COLORS: u32 = 0;
const BI_RGB: u32 = 0;
const SRCCOPY: u32 = 0x00cc_0020;
const CAPTUREBLT: u32 = 0x4000_0000;
const ICON_MAGIC: &[u8] = b"PRISICON1";
const ICON_TARGET_EDGE: i32 = 24;
static BRIDGE_MESSAGE_ID: OnceLock<u32> = OnceLock::new();
static START_RECT_LEFT: AtomicI32 = AtomicI32::new(0);
static START_RECT_TOP: AtomicI32 = AtomicI32::new(0);
static START_RECT_RIGHT: AtomicI32 = AtomicI32::new(0);
static START_RECT_BOTTOM: AtomicI32 = AtomicI32::new(0);
static START_RECT_READY: AtomicBool = AtomicBool::new(false);
static SEARCH_RECT_LEFT: AtomicI32 = AtomicI32::new(0);
static SEARCH_RECT_TOP: AtomicI32 = AtomicI32::new(0);
static SEARCH_RECT_RIGHT: AtomicI32 = AtomicI32::new(0);
static SEARCH_RECT_BOTTOM: AtomicI32 = AtomicI32::new(0);
static SEARCH_RECT_READY: AtomicBool = AtomicBool::new(false);
static START_PRESS_CAPTURED: AtomicBool = AtomicBool::new(false);
static ICON_WINDOW: Mutex<usize> = Mutex::new(0);
static ICON_BITMAP: Mutex<usize> = Mutex::new(0);
static ICON_BACKGROUND: Mutex<Option<(i32, i32, Vec<u8>)>> = Mutex::new(None);

const BRIDGE_MESSAGE: &[u16] = &[
    80, 114, 105, 115, 109, 46, 83, 104, 101, 108, 108, 66, 114, 105, 100, 103, 101, 46, 118, 49, 0,
];
const OBSERVER_CLASS: &[u16] = &[
    80, 114, 105, 115, 109, 82, 97, 119, 75, 101, 121, 98, 111, 97, 114, 100, 79, 98, 115, 101,
    114, 118, 101, 114, 0,
];

const STATIC_CLASS: &[u16] = &[83, 116, 97, 116, 105, 99, 0];
const ICON_WINDOW_TITLE: &[u16] = &[
    80, 114, 105, 115, 109, 46, 83, 116, 97, 114, 116, 73, 99, 111, 110, 83, 104, 101, 108, 108,
    79, 118, 101, 114, 108, 97, 121, 46, 118, 49, 0,
];
const TASKBAR_CLASS: &[u16] = &[
    83, 104, 101, 108, 108, 95, 84, 114, 97, 121, 87, 110, 100, 0,
];

#[link(name = "user32")]
extern "system" {
    fn CallNextHookEx(hhook: Hhook, code: i32, wparam: usize, lparam: isize) -> isize;
    fn FindWindowExW(parent: Hwnd, child_after: Hwnd, class: *const u16, title: *const u16)
        -> Hwnd;
    fn PostMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn UnregisterHotKey(window: Hwnd, id: i32) -> i32;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *mut c_void,
    ) -> Hwnd;
    fn DestroyWindow(window: Hwnd) -> i32;
    fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
    fn GetDC(window: Hwnd) -> *mut c_void;
    fn GetWindowRect(window: Hwnd, rect: *mut Rect) -> i32;
    fn InvalidateRect(window: Hwnd, rect: *const Rect, erase: i32) -> i32;
    fn ReleaseDC(window: Hwnd, dc: *mut c_void) -> i32;
    fn SendMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
    fn SetWindowPos(
        window: Hwnd,
        insert_after: Hwnd,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    fn ShowWindow(window: Hwnd, command: i32) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn BitBlt(
        destination: *mut c_void,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: *mut c_void,
        source_x: i32,
        source_y: i32,
        operation: u32,
    ) -> i32;
    fn CreateCompatibleDC(dc: *mut c_void) -> *mut c_void;
    fn CreateDIBSection(
        dc: *mut c_void,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut c_void,
        section: *mut c_void,
        offset: u32,
    ) -> Hbitmap;
    fn DeleteDC(dc: *mut c_void) -> i32;
    fn DeleteObject(object: *mut c_void) -> i32;
    fn SelectObject(dc: *mut c_void, object: *mut c_void) -> *mut c_void;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(module_name: *const u16) -> *mut c_void;
    fn Sleep(milliseconds: u32);
}

#[repr(C)]
struct ShellExecuteInfoW {
    cb_size: u32,
    f_mask: u32,
    hwnd: Hwnd,
    verb: *const u16,
    file: *const u16,
    parameters: *const u16,
    directory: *const u16,
    show: i32,
    inst_app: *mut c_void,
    id_list: *mut c_void,
    class: *const u16,
    key_class: *mut c_void,
    hot_key: u32,
    icon_or_monitor: *mut c_void,
    process: *mut c_void,
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
    fn ShellExecuteW(
        hwnd: Hwnd,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show_cmd: i32,
    ) -> isize;
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn handle_taskbar_pin(pinned: bool) -> isize {
    let target_file = match std::env::var_os("TEMP") {
        Some(temp) => PathBuf::from(temp)
            .join("Prism")
            .join("taskbar-pin-target.txt"),
        None => return 0,
    };
    let target = match std::fs::read_to_string(target_file) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => return 0,
    };
    let verb = to_wide(if pinned {
        "taskbarpin"
    } else {
        "taskbarunpin"
    });
    let target = to_wide(&target);
    let mut info = ShellExecuteInfoW {
        cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
        f_mask: 0x0000_0400 | 0x0000_0040,
        hwnd: std::ptr::null_mut(),
        verb: verb.as_ptr(),
        file: target.as_ptr(),
        parameters: std::ptr::null(),
        directory: std::ptr::null(),
        show: SW_HIDE,
        inst_app: std::ptr::null_mut(),
        id_list: std::ptr::null_mut(),
        class: std::ptr::null(),
        key_class: std::ptr::null_mut(),
        hot_key: 0,
        icon_or_monitor: std::ptr::null_mut(),
        process: std::ptr::null_mut(),
    };
    if unsafe { ShellExecuteExW(&mut info) } != 0 {
        return 1;
    }
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_HIDE,
        )
    };
    isize::from(result > 32)
}

unsafe fn observer_window() -> Hwnd {
    FindWindowExW(
        HWND_MESSAGE,
        std::ptr::null_mut(),
        OBSERVER_CLASS.as_ptr(),
        std::ptr::null(),
    )
}

unsafe fn notify_observer(message: u32, event: usize, detail: isize) -> bool {
    let observer = observer_window();
    !observer.is_null() && PostMessageW(observer, message, event, detail) != 0
}

fn is_start_command(message: &Msg) -> bool {
    message.message == WM_SYSCOMMAND && message.wparam & 0xFFF0 == SC_TASKLIST
}

fn bridge_message_id() -> u32 {
    *BRIDGE_MESSAGE_ID.get_or_init(|| unsafe { RegisterWindowMessageW(BRIDGE_MESSAGE.as_ptr()) })
}

fn icon_file_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from).map(|path| {
        path.join("app.prism.launcher")
            .join("taskbar-start-icon.rgba")
    })
}

fn load_icon_pixels() -> Result<(u32, u32, Vec<u8>), isize> {
    let path = icon_file_path().ok_or(-10isize)?;
    let bytes = std::fs::read(path).map_err(|_| -11isize)?;
    let header = ICON_MAGIC.len() + 8;
    if bytes.len() < header || &bytes[..ICON_MAGIC.len()] != ICON_MAGIC {
        return Err(-12);
    }
    let width = u32::from_le_bytes(
        bytes[ICON_MAGIC.len()..ICON_MAGIC.len() + 4]
            .try_into()
            .map_err(|_| -12isize)?,
    );
    let height = u32::from_le_bytes(
        bytes[ICON_MAGIC.len() + 4..ICON_MAGIC.len() + 8]
            .try_into()
            .map_err(|_| -12isize)?,
    );
    let pixel_bytes =
        usize::try_from(u64::from(width) * u64::from(height) * 4).map_err(|_| -13isize)?;
    if width == 0
        || height == 0
        || width > 4_096
        || height > 4_096
        || bytes.len() != header + pixel_bytes
    {
        return Err(-13);
    }
    Ok((width, height, bytes[header..].to_vec()))
}

unsafe fn ensure_icon_window() -> Hwnd {
    let Ok(mut slot) = ICON_WINDOW.lock() else {
        return std::ptr::null_mut();
    };
    if *slot != 0 {
        return *slot as Hwnd;
    }
    let owner = FindWindowW(TASKBAR_CLASS.as_ptr(), std::ptr::null());
    if owner.is_null() {
        return std::ptr::null_mut();
    }
    // The overlay is created as an owned popup (WS_POPUP with a parent), so
    // it is a TOP-LEVEL window, not a taskbar child. Every bridge install
    // loads a fresh DLL copy, and each one owns its own overlay; destroy any
    // stale overlay windows left behind by earlier or crashed instances so
    // they can never stack on top of the new glyph.
    loop {
        let stale = FindWindowExW(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            STATIC_CLASS.as_ptr(),
            ICON_WINDOW_TITLE.as_ptr(),
        );
        if stale.is_null() {
            break;
        }
        let bitmap = SendMessageW(stale, STM_GETIMAGE, IMAGE_BITMAP, 0) as Hbitmap;
        let _ = SendMessageW(stale, STM_SETIMAGE, IMAGE_BITMAP, 0);
        if !bitmap.is_null() {
            let _ = DeleteObject(bitmap);
        }
        let _ = DestroyWindow(stale);
    }
    let window = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        STATIC_CLASS.as_ptr(),
        ICON_WINDOW_TITLE.as_ptr(),
        WS_POPUP | SS_BITMAP,
        0,
        0,
        1,
        1,
        owner,
        std::ptr::null_mut(),
        GetModuleHandleW(std::ptr::null()),
        std::ptr::null_mut(),
    );
    if !window.is_null() {
        *slot = window as usize;
    }
    window
}

unsafe fn capture_background(rect: Rect) -> Option<Vec<u8>> {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let screen = GetDC(std::ptr::null_mut());
    if screen.is_null() {
        return None;
    }
    let memory = CreateCompatibleDC(screen);
    if memory.is_null() {
        let _ = ReleaseDC(std::ptr::null_mut(), screen);
        return None;
    }
    let info = BitmapInfo {
        header: BitmapInfoHeader {
            size: std::mem::size_of::<BitmapInfoHeader>() as u32,
            width,
            height: -height,
            planes: 1,
            bit_count: 32,
            compression: BI_RGB,
            size_image: (width * height * 4) as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = CreateDIBSection(
        screen,
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    if bitmap.is_null() || bits.is_null() {
        let _ = DeleteDC(memory);
        let _ = ReleaseDC(std::ptr::null_mut(), screen);
        return None;
    }
    let previous = SelectObject(memory, bitmap);
    let copied = BitBlt(
        memory,
        0,
        0,
        width,
        height,
        screen,
        rect.left,
        rect.top,
        SRCCOPY | CAPTUREBLT,
    ) != 0;
    let mut frame = vec![0u8; (width * height * 4) as usize];
    if copied {
        std::ptr::copy_nonoverlapping(bits.cast::<u8>(), frame.as_mut_ptr(), frame.len());
        for pixel in frame.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        erase_native_glyph(&mut frame, width, height);
    }
    let _ = SelectObject(memory, previous);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(memory);
    let _ = ReleaseDC(std::ptr::null_mut(), screen);
    copied.then_some(frame)
}

fn erase_native_glyph(frame: &mut [u8], width: i32, height: i32) {
    let edge = (ICON_TARGET_EDGE + 8).min(width).min(height);
    let left = (width - edge) / 2;
    let top = (height - edge) / 2;
    let right = left + edge;
    let bottom = top + edge;
    for y in top..bottom {
        let left_source = ((y * width + left.saturating_sub(1)) * 4) as usize;
        let right_source = ((y * width + right.min(width - 1)) * 4) as usize;
        let left_pixel = frame[left_source..left_source + 4].to_vec();
        let right_pixel = frame[right_source..right_source + 4].to_vec();
        for x in left..right {
            let destination = ((y * width + x) * 4) as usize;
            let offset = (x - left) as u32;
            let span = edge.max(1) as u32;
            for channel in 0..3 {
                frame[destination + channel] = ((u32::from(left_pixel[channel]) * (span - offset)
                    + u32::from(right_pixel[channel]) * offset)
                    / span) as u8;
            }
            frame[destination + 3] = 255;
        }
    }
}

fn compose_frame(
    frame_width: i32,
    frame_height: i32,
    mut frame: Vec<u8>,
    icon_width: u32,
    icon_height: u32,
    icon: &[u8],
) -> Vec<u8> {
    if icon_width == 0 || icon_height == 0 {
        return frame;
    }
    // The glyph region is centered and capped, but the source icon may not
    // be square (custom PNGs are pre-pillarboxed, but older or migrated
    // icon files can carry any aspect). Scale the source uniformly into the
    // region and center the result so a non-square source is never
    // stretched - the icon always keeps its own aspect ratio.
    let max_edge = ICON_TARGET_EDGE
        .min(frame_width.saturating_sub(8))
        .min(frame_height.saturating_sub(8))
        .max(1) as u32;
    let scale = (max_edge as f32 / icon_width.max(icon_height) as f32).min(1.0);
    let draw_width = ((icon_width as f32 * scale).round() as u32).max(1);
    let draw_height = ((icon_height as f32 * scale).round() as u32).max(1);
    let left = (frame_width as u32 - draw_width) / 2;
    let top = (frame_height as u32 - draw_height) / 2;
    for y in 0..draw_height {
        for x in 0..draw_width {
            let source_x = (x * icon_width / draw_width).min(icon_width - 1);
            let source_y = (y * icon_height / draw_height).min(icon_height - 1);
            let source = ((source_y * icon_width + source_x) * 4) as usize;
            let destination = (((top + y) * frame_width as u32 + left + x) * 4) as usize;
            let alpha = u32::from(icon[source + 3]);
            for channel in 0..3 {
                let source_value = u32::from(icon[source + (2 - channel)]);
                let destination_value = u32::from(frame[destination + channel]);
                frame[destination + channel] =
                    ((source_value * alpha + destination_value * (255 - alpha)) / 255) as u8;
            }
        }
    }
    frame
}

unsafe fn create_bitmap(width: i32, height: i32, pixels: &[u8]) -> Hbitmap {
    let info = BitmapInfo {
        header: BitmapInfoHeader {
            size: std::mem::size_of::<BitmapInfoHeader>() as u32,
            width,
            height: -height,
            planes: 1,
            bit_count: 32,
            compression: BI_RGB,
            size_image: pixels.len() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = CreateDIBSection(
        std::ptr::null_mut(),
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    if !bitmap.is_null() && !bits.is_null() {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
    }
    bitmap
}

unsafe fn clear_icon_bitmap(window: Hwnd) {
    let previous = SendMessageW(window, STM_SETIMAGE, IMAGE_BITMAP, 0) as Hbitmap;
    if !previous.is_null() {
        let _ = DeleteObject(previous);
    }
    if let Ok(mut bitmap) = ICON_BITMAP.lock() {
        *bitmap = 0;
    }
}

unsafe fn refresh_icon_window() -> isize {
    let window = ensure_icon_window();
    if window.is_null() {
        return -1;
    }
    let (icon_width, icon_height, icon) = match load_icon_pixels() {
        Ok(icon) => icon,
        Err(-11) => {
            clear_icon_bitmap(window);
            let _ = ShowWindow(window, SW_HIDE);
            if let Ok(mut background) = ICON_BACKGROUND.lock() {
                *background = None;
            }
            return 0;
        }
        Err(error) => return error,
    };
    if !START_RECT_READY.load(Ordering::Acquire) {
        let _ = ShowWindow(window, SW_HIDE);
        return -2;
    }
    let rect = Rect {
        left: START_RECT_LEFT.load(Ordering::Relaxed),
        top: START_RECT_TOP.load(Ordering::Relaxed),
        right: START_RECT_RIGHT.load(Ordering::Relaxed),
        bottom: START_RECT_BOTTOM.load(Ordering::Relaxed),
    };
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return -3;
    }
    // Guard against degenerate or mid-transition rects (taskbar animations,
    // stale geometry from a relayout): rendering a square glyph into a
    // badly-proportioned frame squishes or clips it. Keep the previous
    // frame until a sane rect arrives.
    if width < 16 || height < 16 || width * 2 < height || height * 2 < width {
        return -8;
    }
    let _ = ShowWindow(window, SW_HIDE);
    let cached = ICON_BACKGROUND.lock().ok().and_then(|background| {
        background
            .as_ref()
            .and_then(|(cached_width, cached_height, pixels)| {
                (*cached_width == width && *cached_height == height).then(|| pixels.clone())
            })
    });
    let background = match cached {
        Some(background) => Some(background),
        None => {
            // DWM recomposes asynchronously: a capture taken immediately
            // after the hide can still contain the previous frame with the
            // old glyph at its old position, which then bleeds into the
            // composited frame as a ghost. Give composition a frame to
            // settle before reading the screen.
            unsafe { Sleep(50) };
            capture_background(rect)
        }
    };
    let Some(background) = background else {
        return -5;
    };
    if let Ok(mut cached) = ICON_BACKGROUND.lock() {
        *cached = Some((width, height, background.clone()));
    }
    let frame = compose_frame(width, height, background, icon_width, icon_height, &icon);
    let bitmap = create_bitmap(width, height, &frame);
    if bitmap.is_null() {
        return -4;
    }
    clear_icon_bitmap(window);
    let _ = SendMessageW(window, STM_SETIMAGE, IMAGE_BITMAP, bitmap as isize);
    if let Ok(mut current) = ICON_BITMAP.lock() {
        *current = bitmap as usize;
    }
    let _ = SetWindowPos(
        window,
        HWND_TOPMOST,
        rect.left,
        rect.top,
        width,
        height,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
    let _ = InvalidateRect(window, std::ptr::null(), 0);
    1
}

unsafe fn shutdown_icon_window() {
    let window = ICON_WINDOW
        .lock()
        .ok()
        .map(|slot| *slot as Hwnd)
        .unwrap_or(std::ptr::null_mut());
    if !window.is_null() {
        clear_icon_bitmap(window);
        let _ = ShowWindow(window, SW_HIDE);
        let _ = SendMessageW(window, WM_CLOSE, 0, 0);
        let _ = DestroyWindow(window);
    }
    if let Ok(mut slot) = ICON_WINDOW.lock() {
        *slot = 0;
    }
    if let Ok(mut background) = ICON_BACKGROUND.lock() {
        *background = None;
    }
}

fn point_is_in_start_button(point: &Point) -> bool {
    START_RECT_READY.load(Ordering::Acquire)
        && point.x >= START_RECT_LEFT.load(Ordering::Relaxed)
        && point.x < START_RECT_RIGHT.load(Ordering::Relaxed)
        && point.y >= START_RECT_TOP.load(Ordering::Relaxed)
        && point.y < START_RECT_BOTTOM.load(Ordering::Relaxed)
}

fn point_is_in_search_button(point: &Point) -> bool {
    SEARCH_RECT_READY.load(Ordering::Acquire)
        && point.x >= SEARCH_RECT_LEFT.load(Ordering::Relaxed)
        && point.x < SEARCH_RECT_RIGHT.load(Ordering::Relaxed)
        && point.y >= SEARCH_RECT_TOP.load(Ordering::Relaxed)
        && point.y < SEARCH_RECT_BOTTOM.load(Ordering::Relaxed)
}

fn has_active_icon() -> bool {
    ICON_BITMAP
        .lock()
        .map(|bitmap| *bitmap != 0)
        .unwrap_or(false)
}

unsafe fn notify_start_click(message: u32, point: &Point) -> bool {
    let observer = observer_window();
    !observer.is_null()
        && PostMessageW(
            observer,
            message,
            EVENT_TASKBAR_START_CLICK_X,
            point.x as isize,
        ) != 0
        && PostMessageW(
            observer,
            message,
            EVENT_TASKBAR_START_CLICK_Y,
            point.y as isize,
        ) != 0
}

/// Explorer invokes this callback in the thread that owns its Start command.
/// It touches only queued messages and never receives or synthesizes keyboard input.
#[no_mangle]
pub unsafe extern "system" fn PrismShellGetMessageHook(
    code: i32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if code >= HC_ACTION && wparam != 0 && lparam != 0 {
        let message_id = bridge_message_id();
        let message = &mut *(lparam as *mut Msg);

        let control = message.wparam & u32::MAX as usize;
        if message.message == message_id {
            match control {
                CONTROL_DISABLE_WIN_HOTKEY => {
                    let mut disabled = false;
                    for id in 0..=16 {
                        if UnregisterHotKey(std::ptr::null_mut(), id) != 0 {
                            disabled = true;
                        }
                    }
                    let _ = notify_observer(message_id, EVENT_HOTKEY_DISABLED, disabled as isize);
                    message.message = WM_NULL;
                }
                CONTROL_START_RECT_LEFT => {
                    START_RECT_READY.store(false, Ordering::Release);
                    START_PRESS_CAPTURED.store(false, Ordering::Release);
                    if let Ok(mut background) = ICON_BACKGROUND.lock() {
                        *background = None;
                    }
                    START_RECT_LEFT.store(message.lparam as i32, Ordering::Relaxed);
                    message.message = WM_NULL;
                }
                CONTROL_START_RECT_TOP => {
                    START_RECT_TOP.store(message.lparam as i32, Ordering::Relaxed);
                    message.message = WM_NULL;
                }
                CONTROL_START_RECT_RIGHT => {
                    START_RECT_RIGHT.store(message.lparam as i32, Ordering::Relaxed);
                    message.message = WM_NULL;
                }
                CONTROL_START_RECT_BOTTOM => {
                    START_RECT_BOTTOM.store(message.lparam as i32, Ordering::Relaxed);
                    let valid = START_RECT_RIGHT.load(Ordering::Relaxed)
                        > START_RECT_LEFT.load(Ordering::Relaxed)
                        && START_RECT_BOTTOM.load(Ordering::Relaxed)
                            > START_RECT_TOP.load(Ordering::Relaxed);
                    START_RECT_READY.store(valid, Ordering::Release);
                    let _ =
                        notify_observer(message_id, EVENT_START_RECT_CONFIGURED, valid as isize);
                    // The Start button moved or resized: re-render the glyph
                    // at its new position immediately. Waiting for the next
                    // icon change leaves the overlay stranded at the stale
                    // rect, and a later capture could include the stranded
                    // glyph when the old and new rects overlap.
                    if valid && has_active_icon() {
                        let _ = refresh_icon_window();
                    }
                    message.message = WM_NULL;
                }
                CONTROL_START_ICON_REFRESH => {
                    let result = refresh_icon_window();
                    let _ = notify_observer(message_id, EVENT_START_ICON_REFRESHED, result);
                    message.message = WM_NULL;
                }
                CONTROL_START_ICON_SHUTDOWN => {
                    shutdown_icon_window();
                    let _ = notify_observer(message_id, EVENT_START_ICON_SHUTDOWN, 1);
                    message.message = WM_NULL;
                }
                CONTROL_SEARCH_RECT_LEFT => {
                    SEARCH_RECT_READY.store(false, Ordering::Release);
                    START_PRESS_CAPTURED.store(false, Ordering::Release);
                    SEARCH_RECT_LEFT.store(message.lparam as i32, Ordering::Relaxed);
                    message.message = WM_NULL;
                }
                CONTROL_SEARCH_RECT_TOP => {
                    SEARCH_RECT_TOP.store(message.lparam as i32, Ordering::Relaxed);
                    message.message = WM_NULL;
                }
                CONTROL_SEARCH_RECT_RIGHT => {
                    SEARCH_RECT_RIGHT.store(message.lparam as i32, Ordering::Relaxed);
                    message.message = WM_NULL;
                }
                CONTROL_SEARCH_RECT_BOTTOM => {
                    SEARCH_RECT_BOTTOM.store(message.lparam as i32, Ordering::Relaxed);
                    let valid = SEARCH_RECT_RIGHT.load(Ordering::Relaxed)
                        > SEARCH_RECT_LEFT.load(Ordering::Relaxed)
                        && SEARCH_RECT_BOTTOM.load(Ordering::Relaxed)
                            > SEARCH_RECT_TOP.load(Ordering::Relaxed);
                    SEARCH_RECT_READY.store(valid, Ordering::Release);
                    let _ =
                        notify_observer(message_id, EVENT_SEARCH_RECT_CONFIGURED, valid as isize);
                    message.message = WM_NULL;
                }
                CONTROL_TASKBAR_PIN => {
                    let result = handle_taskbar_pin(true);
                    let _ = notify_observer(message_id, EVENT_TASKBAR_PIN_COMPLETED, result);
                    message.message = WM_NULL;
                }
                CONTROL_TASKBAR_UNPIN => {
                    let result = handle_taskbar_pin(false);
                    let _ = notify_observer(message_id, EVENT_TASKBAR_PIN_COMPLETED, result);
                    message.message = WM_NULL;
                }
                _ => {}
            }
        } else if is_start_command(message) && !observer_window().is_null() {
            // Consume the Start command only while Prism's observer is alive.
            // The raw-input state machine decides whether the key sequence was
            // a standalone Win press, so Win+key chords never open Prism.
            message.message = WM_NULL;
        } else if message.message == WM_SETTINGCHANGE && has_active_icon() {
            // Wallpaper, theme, or layout changes behind the Start button
            // invalidate the cached capture. Re-render so the glyph never
            // sits on a stale background. Only when a custom icon is active.
            let _ = refresh_icon_window();
        }
    }

    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(message: u32, wparam: usize) -> Msg {
        Msg {
            hwnd: std::ptr::null_mut(),
            message,
            wparam,
            lparam: 0,
            time: 0,
            point: Point { x: 0, y: 0 },
            private: 0,
        }
    }

    #[test]
    fn identifies_only_the_shell_start_command() {
        assert!(is_start_command(&message(WM_SYSCOMMAND, SC_TASKLIST)));
        assert!(is_start_command(&message(
            WM_SYSCOMMAND,
            SC_TASKLIST | 0x000f
        )));
        assert!(!is_start_command(&message(WM_SYSCOMMAND, 0xF000)));
        assert!(!is_start_command(&message(WM_NULL, SC_TASKLIST)));
    }
}

/// Explorer invokes this callback before taskbar mouse messages reach the
/// Start button. The configured rectangle is semantic UIA/child-window data
/// supplied by Prism; the hook never discovers controls by caption.
#[no_mangle]
pub unsafe extern "system" fn PrismShellMouseHook(
    code: i32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if code >= HC_ACTION && lparam != 0 {
        let mouse = &*(lparam as *const MouseHookStruct);
        let in_target =
            point_is_in_start_button(&mouse.point) || point_is_in_search_button(&mouse.point);
        if wparam == WM_LBUTTONDOWN {
            let capture = in_target && !observer_window().is_null();
            START_PRESS_CAPTURED.store(capture, Ordering::Release);
            if capture {
                return 1;
            }
        } else if wparam == WM_LBUTTONUP && START_PRESS_CAPTURED.swap(false, Ordering::AcqRel) {
            if in_target {
                let _ = notify_start_click(bridge_message_id(), &mouse.point);
            }
            // The matching down was consumed, so always consume its up as well.
            return 1;
        }
    }

    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}
