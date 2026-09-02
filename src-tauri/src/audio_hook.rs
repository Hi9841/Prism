//! Low-level mouse hook to capture taskbar wheel scrolling.
//!
//! Intercepts WM_MOUSEWHEEL events when the cursor is positioned over the
//! Windows taskbar (Shell_TrayWnd or Shell_SecondaryTrayWnd), adjusts the volume
//! of the hovered application (or master volume), shows the OSD HUD, and consumes
//! the wheel message so the taskbar does not scroll horizontally or trigger workspace switching.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WindowFromPoint, HHOOK, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_APP, WM_MOUSEWHEEL,
};

static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
static HOOK_THREAD_ID: AtomicIsize = AtomicIsize::new(0);
static HOOK_ENABLED: AtomicBool = AtomicBool::new(true);
static APP_HANDLE: Mutex<Option<AppHandle>> = Mutex::new(None);

pub fn init(app: AppHandle) {
    if let Ok(mut lock) = APP_HANDLE.lock() {
        *lock = Some(app);
    }
    start_hook_thread();
}

pub fn set_enabled(enabled: bool) {
    HOOK_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    HOOK_ENABLED.load(Ordering::Relaxed)
}

fn start_hook_thread() {
    if HOOK_THREAD_ID.load(Ordering::SeqCst) != 0 {
        return;
    }
    std::thread::spawn(hook_thread_proc);
}

fn hook_thread_proc() {
    unsafe {
        let tid = windows::Win32::System::Threading::GetCurrentThreadId();
        HOOK_THREAD_ID.store(tid as isize, Ordering::SeqCst);

        let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .unwrap_or_default();

        let hook = SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(low_level_mouse_proc),
            Some(module.into()),
            0,
        );

        let hook = match hook {
            Ok(h) => h,
            Err(_) => {
                HOOK_THREAD_ID.store(0, Ordering::SeqCst);
                return;
            }
        };

        HOOK_HANDLE.store(hook.0 as isize, Ordering::SeqCst);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_APP + 99 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let h = HOOK_HANDLE.swap(0, Ordering::SeqCst);
        if h != 0 {
            let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
        }
        HOOK_THREAD_ID.store(0, Ordering::SeqCst);
    }
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && wparam.0 == WM_MOUSEWHEEL as usize && HOOK_ENABLED.load(Ordering::Relaxed) {
        let mouse = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let pt = mouse.pt;
        let hwnd = WindowFromPoint(pt);

        if crate::audio::is_taskbar_window(hwnd) {
            let mouse_data = (mouse.mouseData >> 16) as i16;
            let delta = if mouse_data > 0 { 0.02f32 } else { -0.02f32 };

            // Dispatch volume adjustment to background worker so the mouse hook returns in < 0.2ms
            std::thread::spawn(move || {
                if let Some(change) = crate::audio::adjust_volume_at_taskbar(pt, delta) {
                    crate::audio_osd::show(&change.title, change.percentage, change.muted, pt);
                    if let Ok(lock) = APP_HANDLE.lock() {
                        if let Some(ref app) = *lock {
                            let _ = app.emit("taskbar-volume-changed", &change);
                        }
                    }
                }
            });

            // Consume wheel scroll message over taskbar
            return LRESULT(1);
        }
    }

    let h = HOOK_HANDLE.load(Ordering::Relaxed);
    let hook = if h != 0 { Some(HHOOK(h as *mut _)) } else { None };
    CallNextHookEx(hook, code, wparam, lparam)
}
