//! Win32 helpers for the spike: synthetic OS input, window style probes,
//! system settings, and the OLE file drag (copied from Prism's drag.rs).

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, LPARAM, POINT, S_OK,
    WPARAM,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DoDragDrop, IDropSource, IDropSource_Impl,
    OleInitialize,
};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{BHID_DataObject, IShellItem, SHCreateItemFromParsingName};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HRESULT, PCWSTR, implement};

pub fn hwnd_of(window: &gpui::Window) -> HWND {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).expect("window handle").as_raw() {
        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
        _ => unreachable!("Windows only"),
    }
}

// ---- input posted to the spike's own window ---------------------------------
//
// No SendInput: global input would reach whatever app is in the foreground.
// Messages posted to our HWND go through GPUI's real window procedure.
// GPUI reads modifiers with GetKeyState, which follows this thread's key state.

/// Set this thread's keyboard state for Ctrl, Shift, Alt.
pub fn set_mods(ctrl: bool, shift: bool, alt: bool) {
    // A clean state: no key down except the ones requested. NumLock stays toggled on.
    let mut state = [0u8; 256];
    state[VK_NUMLOCK.0 as usize] = 1;
    for (vk, on) in [
        (VK_CONTROL, ctrl), (VK_LCONTROL, ctrl),
        (VK_SHIFT, shift), (VK_LSHIFT, shift),
        (VK_MENU, alt), (VK_LMENU, alt),
    ] {
        state[vk.0 as usize] = if on { 0x80 } else { 0 };
    }
    unsafe {
        let _ = SetKeyboardState(&state);
    }
}

fn post(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), msg, WPARAM(wparam), LPARAM(lparam));
    }
}

/// Post WM_KEYDOWN + WM_KEYUP (WM_SYSKEY* with Alt) for one key.
pub fn post_key(hwnd: HWND, vk: VIRTUAL_KEY, alt: bool) {
    let extended = matches!(vk, VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT | VK_HOME | VK_END | VK_PRIOR | VK_NEXT);
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as isize;
    let base = 1 | (scan << 16) | if extended { 1 << 24 } else { 0 };
    let (down, up) = if alt { (WM_SYSKEYDOWN, WM_SYSKEYUP) } else { (WM_KEYDOWN, WM_KEYUP) };
    let alt_bit = if alt { 1 << 29 } else { 0 };
    post(hwnd, down, vk.0 as usize, base | alt_bit);
    post(hwnd, up, vk.0 as usize, base | alt_bit | (1 << 30) | (1 << 31));
}

fn pack(x: i32, y: i32) -> isize {
    ((x as u16 as u32) | ((y as u16 as u32) << 16)) as i32 as isize
}

/// Client coordinates in physical pixels.
pub fn post_mouse_down(hwnd: HWND, x: i32, y: i32) {
    post(hwnd, WM_LBUTTONDOWN, MK_LBUTTON.0 as usize, pack(x, y));
}

pub fn post_mouse_up(hwnd: HWND, x: i32, y: i32) {
    post(hwnd, WM_LBUTTONUP, 0, pack(x, y));
}

pub fn post_mouse_move(hwnd: HWND, x: i32, y: i32, left_down: bool) {
    let w = if left_down { MK_LBUTTON.0 as usize } else { 0 };
    post(hwnd, WM_MOUSEMOVE, w, pack(x, y));
}

/// Screen coordinates (WM_MOUSEWHEEL carries screen coordinates).
pub fn post_wheel(hwnd: HWND, sx: i32, sy: i32, delta: i16, ctrl: bool) {
    let keys = if ctrl { MK_CONTROL.0 as usize } else { 0 };
    post(hwnd, WM_MOUSEWHEEL, keys | ((delta as u16 as usize) << 16), pack(sx, sy));
}

pub fn client_to_screen(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut p = POINT { x, y };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut p);
    }
    (p.x, p.y)
}

pub fn foreground() -> HWND {
    unsafe { GetForegroundWindow() }
}

// ---- window style ----------------------------------------------------------

pub fn ex_style(hwnd: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
}

pub fn is_topmost(hwnd: HWND) -> bool {
    ex_style(hwnd) & WS_EX_TOPMOST.0 != 0
}

pub fn is_tool_window(hwnd: HWND) -> bool {
    ex_style(hwnd) & WS_EX_TOOLWINDOW.0 != 0
}

pub fn set_topmost(hwnd: HWND, on: bool) -> Result<(), String> {
    let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
    unsafe {
        SetWindowPos(
            hwnd,
            Some(after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    }
    .map_err(|e| e.to_string())
}

pub fn hide(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

pub fn show_no_activate(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

pub fn send_to_back(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

pub fn window_rect(hwnd: HWND) -> (i32, i32, i32, i32) {
    let mut r = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    (r.left, r.top, r.right, r.bottom)
}

pub fn is_visible(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd).as_bool() }
}

// ---- system settings -------------------------------------------------------

/// `true` when Windows "Animation effects" is on (SPI_GETCLIENTAREAANIMATION).
pub fn client_area_animation() -> bool {
    let mut v = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut v as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    v.as_bool()
}

pub fn wheel_scroll_lines() -> u32 {
    let mut v = 0u32;
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some(&mut v as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    v
}

// ---- OLE file drag (Prism src-tauri/src/drag.rs, without the drag image) ----

#[implement(IDropSource)]
struct DropSource(());

#[allow(non_snake_case)]
impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, esc: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if esc.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (keys & MK_LBUTTON) == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Blocks in the OLE modal loop. Returns "dropped" or "cancelled".
pub fn drag_file(path: &Path) -> Result<&'static str, String> {
    unsafe {
        let _ = OleInitialize(None);
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let item: IShellItem =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).map_err(|e| e.to_string())?;
        let data: IDataObject = item
            .BindToHandler(None, &BHID_DataObject)
            .map_err(|e| e.to_string())?;
        let source: IDropSource = DropSource(()).into();
        let mut effect = DROPEFFECT::default();
        let hr = DoDragDrop(&data, &source, DROPEFFECT_COPY | DROPEFFECT_LINK, &mut effect);
        Ok(if hr == DRAGDROP_S_DROP {
            "dropped"
        } else {
            "cancelled"
        })
    }
}
