#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::OnceLock;

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

#[repr(C)]
struct MouseHookStruct {
    point: Point,
    window: Hwnd,
    hit_test_code: u32,
    extra_info: usize,
}

const HC_ACTION: i32 = 0;
const HWND_MESSAGE: Hwnd = -3isize as Hwnd;
const WM_NULL: u32 = 0;
const WM_SYSCOMMAND: u32 = 0x0112;
const WM_LBUTTONDOWN: usize = 0x0201;
const WM_LBUTTONUP: usize = 0x0202;
const SC_TASKLIST: usize = 0xF130;
const CONTROL_DISABLE_WIN_HOTKEY: usize = 1;
const EVENT_HOTKEY_DISABLED: usize = 2;
const EVENT_TOGGLE_PRISM: usize = 3;
const CONTROL_START_RECT_LEFT: usize = 4;
const CONTROL_START_RECT_TOP: usize = 5;
const CONTROL_START_RECT_RIGHT: usize = 6;
const CONTROL_START_RECT_BOTTOM: usize = 7;
const EVENT_START_RECT_CONFIGURED: usize = 8;
const EVENT_TASKBAR_START_CLICK_X: usize = 9;
const EVENT_TASKBAR_START_CLICK_Y: usize = 10;
static BRIDGE_MESSAGE_ID: OnceLock<u32> = OnceLock::new();
static START_RECT_LEFT: AtomicI32 = AtomicI32::new(0);
static START_RECT_TOP: AtomicI32 = AtomicI32::new(0);
static START_RECT_RIGHT: AtomicI32 = AtomicI32::new(0);
static START_RECT_BOTTOM: AtomicI32 = AtomicI32::new(0);
static START_RECT_READY: AtomicBool = AtomicBool::new(false);
static START_PRESS_CAPTURED: AtomicBool = AtomicBool::new(false);

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

fn bridge_message_id() -> u32 {
    *BRIDGE_MESSAGE_ID.get_or_init(|| unsafe { RegisterWindowMessageW(BRIDGE_MESSAGE.as_ptr()) })
}

fn point_is_in_start_button(point: &Point) -> bool {
    START_RECT_READY.load(Ordering::Acquire)
        && point.x >= START_RECT_LEFT.load(Ordering::Relaxed)
        && point.x < START_RECT_RIGHT.load(Ordering::Relaxed)
        && point.y >= START_RECT_TOP.load(Ordering::Relaxed)
        && point.y < START_RECT_BOTTOM.load(Ordering::Relaxed)
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
                    let disabled = UnregisterHotKey(std::ptr::null_mut(), 1) != 0;
                    let _ = notify_observer(message_id, EVENT_HOTKEY_DISABLED, disabled as isize);
                    message.message = WM_NULL;
                }
                CONTROL_START_RECT_LEFT => {
                    START_RECT_READY.store(false, Ordering::Release);
                    START_PRESS_CAPTURED.store(false, Ordering::Release);
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
                    let _ = notify_observer(
                        message_id,
                        EVENT_START_RECT_CONFIGURED,
                        valid as isize,
                    );
                    message.message = WM_NULL;
                }
                _ => {}
            }
        } else if message.message == WM_SYSCOMMAND
            && message.wparam & 0xFFF0 == SC_TASKLIST
            && notify_observer(message_id, EVENT_TOGGLE_PRISM, message.lparam)
        {
            // Consume the Start command only after Prism accepted the event.
            // If Prism is gone, the command remains untouched and Start opens.
            message.message = WM_NULL;
        }
    }

    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
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
        if wparam == WM_LBUTTONDOWN {
            let capture = point_is_in_start_button(&mouse.point) && !observer_window().is_null();
            START_PRESS_CAPTURED.store(capture, Ordering::Release);
            if capture {
                return 1;
            }
        } else if wparam == WM_LBUTTONUP && START_PRESS_CAPTURED.swap(false, Ordering::AcqRel) {
            if point_is_in_start_button(&mouse.point) {
                let _ = notify_start_click(bridge_message_id(), &mouse.point);
            }
            // The matching down was consumed, so always consume its up as well.
            return 1;
        }
    }

    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}
