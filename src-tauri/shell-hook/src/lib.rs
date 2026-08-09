#![allow(non_snake_case)]

use std::ffi::c_void;

type Hhook = *mut c_void;
type Hwnd = *mut c_void;

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

const HC_ACTION: i32 = 0;
const HWND_MESSAGE: Hwnd = -3isize as Hwnd;
const WM_NULL: u32 = 0;
const WM_SYSCOMMAND: u32 = 0x0112;
const SC_TASKLIST: usize = 0xF130;
const CONTROL_DISABLE_WIN_HOTKEY: usize = 1;
const EVENT_HOTKEY_DISABLED: usize = 2;
const EVENT_TOGGLE_PRISM: usize = 3;

const BRIDGE_MESSAGE: &[u16] = &[
    80, 114, 105, 115, 109, 46, 83, 104, 101, 108, 108, 66, 114, 105, 100, 103, 101, 46, 118,
    49, 0,
];
const OBSERVER_CLASS: &[u16] = &[
    80, 114, 105, 115, 109, 82, 97, 119, 75, 101, 121, 98, 111, 97, 114, 100, 79, 98, 115,
    101, 114, 118, 101, 114, 0,
];

#[link(name = "user32")]
extern "system" {
    fn CallNextHookEx(hhook: Hhook, code: i32, wparam: usize, lparam: isize) -> isize;
    fn FindWindowExW(parent: Hwnd, child_after: Hwnd, class: *const u16, title: *const u16)
        -> Hwnd;
    fn PostMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn UnregisterHotKey(window: Hwnd, id: i32) -> i32;
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

/// Explorer invokes this callback in the thread that owns its Start command.
/// It touches only queued messages and never receives or synthesizes keyboard input.
#[no_mangle]
pub unsafe extern "system" fn PrismShellGetMessageHook(
    code: i32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if code >= HC_ACTION && wparam != 0 && lparam != 0 {
        let message_id = RegisterWindowMessageW(BRIDGE_MESSAGE.as_ptr());
        let message = &mut *(lparam as *mut Msg);

        if message.message == message_id && message.wparam == CONTROL_DISABLE_WIN_HOTKEY {
            let disabled = UnregisterHotKey(std::ptr::null_mut(), 1) != 0;
            let _ = notify_observer(message_id, EVENT_HOTKEY_DISABLED, disabled as isize);
            message.message = WM_NULL;
        } else if message.message == WM_SYSCOMMAND
            && message.wparam & 0xFFF0 == SC_TASKLIST
            && notify_observer(message_id, EVENT_TOGGLE_PRISM, 0)
        {
            // Consume the Start command only after Prism accepted the event.
            // If Prism is gone, the command remains untouched and Start opens.
            message.message = WM_NULL;
        }
    }

    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}
