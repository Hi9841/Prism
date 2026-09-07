//! Low-level mouse hook to capture taskbar wheel scrolling.
//!
//! Intercepts WM_MOUSEWHEEL events when the cursor is positioned over the
//! Windows taskbar (Shell_TrayWnd or Shell_SecondaryTrayWnd), adjusts the volume
//! of the hovered application (or master volume), shows the OSD HUD, and consumes
//! the wheel message so the taskbar does not scroll horizontally or trigger workspace switching.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WindowFromPoint, HHOOK, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_APP,
    WM_MOUSEWHEEL,
};

static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
static HOOK_THREAD_ID: AtomicIsize = AtomicIsize::new(0);
static HOOK_ENABLED: AtomicBool = AtomicBool::new(true);
static AUDIO_ENABLE_GENERATION: AtomicU64 = AtomicU64::new(0);
static AUDIO_REQUESTS: OnceLock<SyncSender<AudioRequest>> = OnceLock::new();

const AUDIO_QUEUE_CAPACITY: usize = 64;
const MAX_COALESCED_REQUESTS: usize = 16;

#[derive(Clone, Copy, Debug)]
struct AudioRequest {
    point: POINT,
    delta: f32,
    enable_generation: u64,
}

#[derive(Clone, Debug)]
struct ResolvedRequest {
    point: POINT,
    delta: f32,
    enable_generation: u64,
    target: crate::audio::TaskbarTarget,
}

pub fn init(app: AppHandle) {
    AUDIO_REQUESTS.get_or_init(|| start_audio_worker(app));
    start_hook_thread();
}

pub fn set_enabled(enabled: bool) {
    if HOOK_ENABLED.swap(enabled, Ordering::SeqCst) != enabled {
        AUDIO_ENABLE_GENERATION.fetch_add(1, Ordering::SeqCst);
    }
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

fn start_audio_worker(app: AppHandle) -> SyncSender<AudioRequest> {
    let (sender, receiver) = sync_channel(AUDIO_QUEUE_CAPACITY);
    std::thread::spawn(move || audio_worker_proc(app, receiver));
    sender
}

fn audio_worker_proc(app: AppHandle, receiver: Receiver<AudioRequest>) {
    let Ok(_com_apartment) = crate::audio::ComApartment::initialize() else {
        return;
    };

    let mut pending: Option<ResolvedRequest> = None;
    loop {
        let mut current = match pending.take() {
            Some(request) => request,
            None => {
                let Ok(request) = receiver.recv() else {
                    return;
                };
                resolve_request(request)
            }
        };

        let incoming = std::iter::from_fn(|| receiver.try_recv().ok()).map(resolve_request);
        pending = coalesce_batch(&mut current, incoming);

        if !request_is_current(
            current.enable_generation,
            HOOK_ENABLED.load(Ordering::SeqCst),
            AUDIO_ENABLE_GENERATION.load(Ordering::SeqCst),
        ) {
            continue;
        }

        if let Some(change) = crate::audio::adjust_volume_for_target(&current.target, current.delta)
        {
            crate::audio_osd::show(
                &change.title,
                change.percentage,
                change.muted,
                current.point,
            );
            let _ = app.emit("taskbar-volume-changed", &change);
        }
    }
}

fn resolve_request(request: AudioRequest) -> ResolvedRequest {
    ResolvedRequest {
        point: request.point,
        delta: request.delta,
        enable_generation: request.enable_generation,
        target: crate::audio::identify_taskbar_target_at(request.point),
    }
}

fn merge_if_same_target(current: &mut ResolvedRequest, next: &ResolvedRequest) -> bool {
    if current.target != next.target
        || current.enable_generation != next.enable_generation
        || current.delta.signum() != next.delta.signum()
    {
        return false;
    }
    current.delta += next.delta;
    current.point = next.point;
    true
}

fn coalesce_batch(
    current: &mut ResolvedRequest,
    incoming: impl Iterator<Item = ResolvedRequest>,
) -> Option<ResolvedRequest> {
    incoming
        .take(MAX_COALESCED_REQUESTS - 1)
        .find(|next| !merge_if_same_target(current, next))
}

fn enqueue_audio_request(request: AudioRequest) -> bool {
    let Some(sender) = AUDIO_REQUESTS.get() else {
        return false;
    };
    match sender.try_send(request) {
        Ok(()) => true,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
    }
}

fn wheel_delta(mouse_data: i16) -> Option<f32> {
    if mouse_data == 0 {
        None
    } else {
        Some(f32::from(mouse_data) / 120.0 * 0.02)
    }
}

fn request_is_current(request_generation: u64, enabled: bool, current_generation: u64) -> bool {
    enabled && request_generation == current_generation
}

fn hook_thread_proc() {
    unsafe {
        let tid = windows::Win32::System::Threading::GetCurrentThreadId();
        HOOK_THREAD_ID.store(tid as isize, Ordering::SeqCst);

        let module =
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();

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
            if let Some(delta) = wheel_delta(mouse_data) {
                let request = AudioRequest {
                    point: pt,
                    delta,
                    enable_generation: AUDIO_ENABLE_GENERATION.load(Ordering::SeqCst),
                };
                if enqueue_audio_request(request) {
                    return LRESULT(1);
                }
            }
        }
    }

    let h = HOOK_HANDLE.load(Ordering::Relaxed);
    let hook = if h != 0 {
        Some(HHOOK(h as *mut _))
    } else {
        None
    };
    CallNextHookEx(hook, code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(target: crate::audio::TaskbarTarget, delta: f32, x: i32) -> ResolvedRequest {
        ResolvedRequest {
            point: POINT { x, y: 10 },
            delta,
            enable_generation: 3,
            target,
        }
    }

    #[test]
    fn adjacent_requests_for_the_same_target_accumulate() {
        let mut current = request(crate::audio::TaskbarTarget::Master, 0.02, 10);
        let next = request(crate::audio::TaskbarTarget::Master, 0.005, 11);

        assert!(merge_if_same_target(&mut current, &next));
        assert!((current.delta - 0.025).abs() < f32::EPSILON);
        assert_eq!(current.point.x, 11);
    }

    #[test]
    fn target_changes_are_not_coalesced() {
        let mut current = request(crate::audio::TaskbarTarget::Master, 0.02, 10);
        let next = request(
            crate::audio::TaskbarTarget::Application {
                display_title: "Music".to_string(),
                executable_stem: "music".to_string(),
            },
            0.02,
            11,
        );

        assert!(!merge_if_same_target(&mut current, &next));
        assert_eq!(current.delta, 0.02);
        assert_eq!(current.point.x, 10);
    }

    #[test]
    fn direction_changes_are_not_coalesced() {
        let mut current = request(crate::audio::TaskbarTarget::Master, 0.02, 10);
        let next = request(crate::audio::TaskbarTarget::Master, -0.02, 11);

        assert!(!merge_if_same_target(&mut current, &next));
        assert_eq!(current.delta, 0.02);
        assert_eq!(current.point.x, 10);
    }

    #[test]
    fn a_batch_never_exceeds_the_processing_bound() {
        let mut current = request(crate::audio::TaskbarTarget::Master, 0.02, 0);
        let requests = (1..32).map(|x| request(crate::audio::TaskbarTarget::Master, 0.02, x));
        let mut incoming = requests.peekable();

        assert!(coalesce_batch(&mut current, incoming.by_ref()).is_none());
        assert!((current.delta - 0.32).abs() < 0.000_001);
        assert_eq!(incoming.count(), 16);
    }

    #[test]
    fn wheel_delta_preserves_magnitude_and_zero() {
        assert_eq!(wheel_delta(0), None);
        assert_eq!(wheel_delta(120), Some(0.02));
        assert_eq!(wheel_delta(-120), Some(-0.02));
        assert_eq!(wheel_delta(30), Some(0.005));
    }

    #[test]
    fn disabled_and_pre_reenable_requests_are_stale() {
        assert!(request_is_current(3, true, 3));
        assert!(!request_is_current(3, false, 3));
        assert!(!request_is_current(3, true, 4));
    }
}
