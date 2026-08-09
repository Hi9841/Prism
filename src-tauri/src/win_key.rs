//! Win-key interception (disabled by default).
//!
//! Turns a standalone Win-key press into a Prism toggle while leaving
//! Win+* combos intact. Design rules:
//!
//! - A pure, side-aware state machine (`WinKeyMachine`) decides everything;
//!   it is unit-tested without Win32 involvement.
//! - The hook callback only feeds the machine, inserts the two-event menu
//!   mask, and queues window work. It never blocks on UI or filesystem work.
//! - Every Win32 result is checked. A failed replay triggers a safe
//!   recovery path that disables interception.
//! - A short, exact-host guard dismisses Start when a shell replacement
//!   opens it independently of the swallowed Windows-key events.
//! - Disabling, quitting or failing mid-keypress resets all state, so no
//!   key is ever left stuck or swallowed.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use windows::core::{BOOL, PCWSTR, PWSTR};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HID_USAGE_GENERIC_KEYBOARD, HID_USAGE_PAGE_GENERIC, KEYBOARD_OVERRUN_MAKE_CODE,
};
use windows::Win32::Foundation::{CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_RWIN,
};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RAWKEYBOARD, RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
    GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
    MsgWaitForMultipleObjectsEx, PeekMessageW, PostThreadMessageW, RegisterClassW,
    SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx, EVENT_OBJECT_SHOW,
    HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, OBJID_WINDOW, PM_REMOVE, QS_ALLINPUT, RI_KEY_BREAK,
    SW_HIDE, WH_KEYBOARD_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WINEVENT_OUTOFCONTEXT, WM_APP,
    WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW,
};

/// Magic `dwExtraInfo` value tagging events we synthesize ourselves.
const SYNTH_TAG: usize = 0x50524953; // "PRIS"
/// A short Ctrl tap marks Win as a chord before replacement Start-menu hooks
/// receive Win-up. This is the standard Windows menu-mask sequence; unlike
/// unassigned virtual keys, it is honored by StartAllBack on this machine.
const MENU_MASK_KEY: VIRTUAL_KEY = VK_CONTROL;

/// Number of input events a successful combo replay must send.
const REPLAY_EVENTS: u32 = 2;

/// Keep watching briefly because StartAllBack can react after our hook has
/// already consumed the physical Win-key release.
const START_GUARD_DURATION: Duration = Duration::from_millis(300);
const START_GUARD_INTERVAL: Duration = Duration::from_millis(8);
const ACTION_MESSAGE: u32 = WM_APP + 1;
const REFRESH_HOOK_MESSAGE: u32 = WM_APP + 2;
const TOGGLE_DEBOUNCE_MS: u64 = 50;

/// Event the frontend receives when interception self-disables (e.g. a
/// replay was rejected because the foreground app is elevated).
pub const FAILED_EVENT: &str = "win-mode-failed";

pub fn is_start_guard_active() -> bool {
    START_GUARD_ACTIVE.load(Ordering::Acquire)
}

/// Reinstalls the low-level hook at the front of the chain without leaving an
/// interception gap. Shell replacements can rehook after focus transitions.
pub fn refresh_hook_priority() {
    if !ACTIVE.load(Ordering::Acquire) || RAW_PROVIDER_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let tid = THREAD_ID.load(Ordering::Acquire);
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, REFRESH_HOOK_MESSAGE, WPARAM(0), LPARAM(0));
        }
    }
}

/* ------------------------------------------------------------------ */
/*  Pure state machine (unit-tested)                                    */
/* ------------------------------------------------------------------ */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Win(WinSide),
    Other(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Deliver the event unchanged.
    Pass,
    /// Suppress the event.
    Swallow,
    /// Suppress the first Win-down and queue the inert menu-mask key.
    Mask,
    /// Suppress and toggle the palette (a standalone Win press completed).
    Toggle(WinSide),
    /// Suppress and replay a synthesized Win+key down (the system then sees
    /// the combo exactly once, with the originating Win key).
    Replay { side: WinSide, key: u16 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Press {
    down: bool,
    combo: bool,
}

/// Side-aware press tracking. Both Windows keys keep independent state so
/// their identity and balanced down/up transitions are preserved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WinKeyMachine {
    left: Press,
    right: Press,
}

impl WinKeyMachine {
    /// Const-initializer for the static machine.
    pub const EMPTY: Self = WinKeyMachine {
        left: Press {
            down: false,
            combo: false,
        },
        right: Press {
            down: false,
            combo: false,
        },
    };
    /// Feeds one key event into the machine and returns the decision.
    pub fn feed(&mut self, kind: KeyKind, is_down: bool) -> Decision {
        match kind {
            KeyKind::Win(side) => {
                let press = self.press_mut(side);
                if is_down {
                    if press.down {
                        // Auto-repeat or extra press while already held.
                        Decision::Swallow
                    } else {
                        press.down = true;
                        press.combo = false;
                        Decision::Mask
                    }
                } else if press.down {
                    let standalone = !press.combo;
                    press.down = false;
                    press.combo = false;
                    if standalone {
                        Decision::Toggle(side)
                    } else {
                        // Part of a replay: the synthesized Win-down needs
                        // the real release to pass so the system stays
                        // balanced.
                        Decision::Pass
                    }
                } else {
                    Decision::Pass
                }
            }
            KeyKind::Other(key) => {
                let left_down = self.left.down;
                let right_down = self.right.down;
                if is_down && (left_down || right_down) {
                    // Win+key combo: remember it so the Win release is not
                    // mistaken for a standalone press.
                    self.left.combo |= left_down;
                    self.right.combo |= right_down;
                    if is_modifier(key) {
                        // Modifiers (Shift/Ctrl/Alt) pass through untouched -
                        // the user is already holding them. Replaying them
                        // would duplicate Win-downs and risk stuck keys.
                        Decision::Pass
                    } else {
                        // Replay with the originating side (left wins when
                        // both are held) so the system sees the combo exactly
                        // once.
                        let side = if left_down {
                            WinSide::Left
                        } else {
                            WinSide::Right
                        };
                        Decision::Replay { side, key }
                    }
                } else {
                    Decision::Pass
                }
            }
        }
    }

    /// Clears all press state (used when interception is disabled
    /// mid-keypress; no key stays stuck or half-swallowed).
    pub fn reset(&mut self) {
        self.left = Press::default();
        self.right = Press::default();
    }

    fn press_mut(&mut self, side: WinSide) -> &mut Press {
        match side {
            WinSide::Left => &mut self.left,
            WinSide::Right => &mut self.right,
        }
    }
}

/// True for Shift/Ctrl/Alt (both left and right variants). These pass
/// through the interception untouched; only their presence marks a combo.
fn is_modifier(key: u16) -> bool {
    matches!(
        key,
        0x10 | 0xA0 | 0xA1 // Shift / LShift / RShift
        | 0x11 | 0xA2 | 0xA3 // Ctrl / LCtrl / RCtrl
        | 0x12 | 0xA4 | 0xA5 // Alt / LAlt / RAlt
    )
}

/* ------------------------------------------------------------------ */
/*  Hook plumbing                                                       */
/* ------------------------------------------------------------------ */

static APP: OnceLock<AppHandle> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SHELL_SUPPRESSES_START: AtomicBool = AtomicBool::new(false);
static RAW_PROVIDER_ACTIVE: AtomicBool = AtomicBool::new(false);
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
type HookReady = mpsc::SyncSender<Result<(), String>>;
type StopReady = mpsc::SyncSender<()>;

static START_TX: OnceLock<mpsc::Sender<HookReady>> = OnceLock::new();
static MACHINE: Mutex<WinKeyMachine> = Mutex::new(WinKeyMachine::EMPTY);
static RAW_MACHINE: Mutex<WinKeyMachine> = Mutex::new(WinKeyMachine::EMPTY);
static START_GUARD_ACTIVE: AtomicBool = AtomicBool::new(false);
static START_GUARD_WORKER: AtomicBool = AtomicBool::new(false);
static START_GUARD_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
static LAST_TOGGLE_MS: AtomicU64 = AtomicU64::new(0);
static TOGGLE_CLOCK: OnceLock<Instant> = OnceLock::new();

enum Action {
    MaskFailed,
    Toggle(WinSide),
    Replay { side: WinSide, key: u16 },
}

static ACTION_TX: OnceLock<mpsc::Sender<Action>> = OnceLock::new();
static ACTION_RX: Mutex<Option<mpsc::Receiver<Action>>> = Mutex::new(None);
static STOP_READY: Mutex<Option<StopReady>> = Mutex::new(None);
static RAW_INPUT_CLASS: OnceLock<Result<(), String>> = OnceLock::new();

pub fn init(app: AppHandle) {
    let _ = APP.set(app);
}

pub fn set_shell_suppression(active: bool) {
    SHELL_SUPPRESSES_START.store(active, Ordering::Release);
    RAW_MACHINE.lock().map(|mut machine| machine.reset()).ok();
}

/// Turns Win-key interception on or off. Disabling resets the machine and
/// tears the hook down so normal input is restored immediately.
pub fn set_enabled(on: bool) -> Result<(), String> {
    if on == ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }
    ACTIVE.store(on, Ordering::SeqCst);
    if !on {
        START_GUARD_ACTIVE.store(false, Ordering::Release);
        MACHINE.lock().map(|mut m| m.reset()).ok();
        RAW_MACHINE.lock().map(|mut m| m.reset()).ok();
        clear_queued_actions();
        LAST_TOGGLE_MS.store(0, Ordering::Release);
        let tid = THREAD_ID.load(Ordering::SeqCst);
        if tid != 0 {
            let (stop_tx, stop_rx) = mpsc::sync_channel(1);
            if let Ok(mut slot) = STOP_READY.lock() {
                *slot = Some(stop_tx);
            }
            unsafe {
                let _ = PostThreadMessageW(tid, WM_APP, WPARAM(0), LPARAM(0));
            }
            if stop_rx.recv_timeout(Duration::from_secs(2)).is_err() {
                return Err("timed out while stopping Windows-key interception".to_string());
            }
        }
        return Ok(());
    }

    if START_TX.get().is_none() {
        let (tx, rx) = mpsc::channel();
        let (action_tx, action_rx) = mpsc::channel();
        let _ = ACTION_TX.set(action_tx);
        if let Ok(mut slot) = ACTION_RX.lock() {
            *slot = Some(action_rx);
        }
        let _ = START_TX.set(tx);
        std::thread::spawn(move || pump_loop(rx));
    }

    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    if START_TX
        .get()
        .ok_or_else(|| "hook thread unavailable".to_string())?
        .send(ready_tx)
        .is_err()
    {
        ACTIVE.store(false, Ordering::SeqCst);
        return Err("hook thread unavailable".to_string());
    }

    match ready_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(_) => {
            ACTIVE.store(false, Ordering::SeqCst);
            let tid = THREAD_ID.load(Ordering::SeqCst);
            if tid != 0 {
                unsafe {
                    let _ = PostThreadMessageW(tid, WM_APP, WPARAM(0), LPARAM(0));
                }
            }
            Err("timed out while installing Windows-key interception".to_string())
        }
    }
}

fn pump_loop(rx: mpsc::Receiver<HookReady>) {
    THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    while let Ok(ready) = rx.recv() {
        if ACTIVE.load(Ordering::SeqCst) {
            unsafe { run_pump(ready) };
        } else {
            let _ = ready.send(Err("Windows-key interception was cancelled".to_string()));
        }
    }
    THREAD_ID.store(0, Ordering::SeqCst);
}

/// Installs the hook and pumps messages until a stop request arrives.
unsafe fn run_pump(ready: HookReady) {
    let provider_mode = SHELL_SUPPRESSES_START.load(Ordering::Acquire);
    let raw_input_window = if provider_mode {
        match create_raw_input_window() {
            Ok(window) => Some(window),
            Err(error) => {
                let _ = ready.send(Err(error));
                disable_interception("raw keyboard input registration failed");
                return;
            }
        }
    } else {
        None
    };
    let mut hook = if provider_mode {
        None
    } else {
        match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) {
            Ok(hook) => Some(hook),
            Err(_) => {
                let _ = ready.send(Err("keyboard hook installation failed".to_string()));
                disable_interception("hook installation failed");
                return;
            }
        }
    };
    let show_hook = if provider_mode {
        None
    } else {
        // Generic fallback only: watch exact Start surfaces because an
        // earlier replacement hook may run before Prism's hook.
        let show_hook = SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_SHOW,
            None,
            Some(start_window_shown),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if show_hook.0.is_null() {
            if let Some(hook) = hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            let _ = ready.send(Err("Start-menu event hook installation failed".to_string()));
            disable_interception("Start-menu event hook installation failed");
            return;
        }
        Some(show_hook)
    };
    RAW_PROVIDER_ACTIVE.store(provider_mode, Ordering::Release);
    let _ = ready.send(Ok(()));
    let mut msg = MSG::default();
    'pump: loop {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_APP {
                break 'pump;
            }
            if msg.message == ACTION_MESSAGE {
                continue;
            }
            if msg.message == REFRESH_HOOK_MESSAGE {
                if let Some(old_hook) = hook {
                    if let Ok(new_hook) =
                        SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0)
                    {
                        if UnhookWindowsHookEx(old_hook).is_ok() {
                            hook = Some(new_hook);
                        } else {
                            let _ = UnhookWindowsHookEx(new_hook);
                        }
                    }
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        // Drain window and replay actions outside the callback.
        if let Ok(mut rx_slot) = ACTION_RX.lock() {
            if let Some(rx) = rx_slot.as_mut() {
                while let Ok(action) = rx.try_recv() {
                    if !ACTIVE.load(Ordering::Acquire) {
                        break 'pump;
                    }
                    match action {
                        Action::MaskFailed => {
                            disable_interception("menu-mask input rejected (elevated app?)");
                            break 'pump;
                        }
                        Action::Toggle(_side) => {
                            if !provider_mode {
                                // Generic fallback: dismiss only a positively
                                // identified Start surface if another shell
                                // hook opened one before Prism's hook ran.
                                if unsafe { dismiss_start_menus() } {
                                    refocus_palette();
                                }
                                start_menu_guard();
                            }
                            if let Some(app) = APP.get() {
                                let toggle_app = app.clone();
                                let _ = app.run_on_main_thread(move || {
                                    crate::toggle_palette(&toggle_app);
                                });
                            }
                        }
                        Action::Replay { side, key } => {
                            let win_vk = match side {
                                WinSide::Left => VK_LWIN.0,
                                WinSide::Right => VK_RWIN.0,
                            };
                            if !send_combo(win_vk, key) {
                                disable_interception("input replay rejected (elevated app?)");
                                break 'pump;
                            }
                        }
                    }
                }
            }
        }
        let _ = MsgWaitForMultipleObjectsEx(None, 100, QS_ALLINPUT, Default::default());
    }
    RAW_PROVIDER_ACTIVE.store(false, Ordering::Release);
    RAW_MACHINE.lock().map(|mut machine| machine.reset()).ok();
    if let Some(show_hook) = show_hook {
        let _ = UnhookWinEvent(show_hook);
    }
    if let Some(hook) = hook {
        let _ = UnhookWindowsHookEx(hook);
    }
    if let Some(raw_input_window) = raw_input_window {
        destroy_raw_input_window(raw_input_window);
    }
    if !ACTIVE.load(Ordering::Acquire) {
        clear_queued_actions();
    }
    if let Ok(mut slot) = STOP_READY.lock() {
        if let Some(ready) = slot.take() {
            let _ = ready.send(());
        }
    }
}

unsafe extern "system" fn start_window_shown(
    _hook: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    object_id: i32,
    child_id: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if event != EVENT_OBJECT_SHOW
        || object_id != OBJID_WINDOW.0
        || child_id != 0
        || !ACTIVE.load(Ordering::Acquire)
        || window.0.is_null()
    {
        return;
    }

    let Some(host) = start_menu_host(window) else {
        return;
    };
    if host == StartMenuHost::Native && !START_GUARD_ACTIVE.load(Ordering::Acquire) {
        return;
    }

    let _ = ShowWindow(window, SW_HIDE);
    refocus_palette();
}

/// Safe recovery path: stop intercepting, restore normal input, and tell
/// the frontend so the user can react.
fn disable_interception(reason: &str) {
    ACTIVE.store(false, Ordering::SeqCst);
    START_GUARD_ACTIVE.store(false, Ordering::Release);
    MACHINE.lock().map(|mut m| m.reset()).ok();
    LAST_TOGGLE_MS.store(0, Ordering::Release);
    if let Some(app) = APP.get() {
        let _ = app.emit(FAILED_EVENT, reason.to_string());
    }
}

fn clear_queued_actions() {
    if let Ok(mut rx_slot) = ACTION_RX.lock() {
        if let Some(rx) = rx_slot.as_mut() {
            while rx.try_recv().is_ok() {}
        }
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && ACTIVE.load(Ordering::SeqCst) {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // Skip our own synthesized events.
        if kb.dwExtraInfo == SYNTH_TAG {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // The supported Start-provider path uses raw keyboard HID events for
        // standalone-Win detection. Raw input is independent of ordered hook
        // delivery, so another hook cannot starve Prism of the release event.
        if SHELL_SUPPRESSES_START.load(Ordering::Acquire) {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // Remappers and macro tools must follow the selected binding too.
        // Only Prism's own tagged replay is exempt above.
        {
            let msg = wparam.0 as u32;
            let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
            let vk = kb.vkCode as u16;
            let kind = if vk == VK_LWIN.0 {
                KeyKind::Win(WinSide::Left)
            } else if vk == VK_RWIN.0 {
                KeyKind::Win(WinSide::Right)
            } else {
                KeyKind::Other(vk)
            };
            if is_down || is_up {
                let decision = {
                    let mut machine = match MACHINE.lock() {
                        Ok(m) => m,
                        Err(_) => return CallNextHookEx(None, code, wparam, lparam),
                    };
                    machine.feed(kind, is_down)
                };
                match decision {
                    Decision::Pass => {}
                    Decision::Swallow => return LRESULT(1),
                    Decision::Mask => {
                        // Insert the inert chord marker before returning from
                        // Win-down. This prevents replacement hooks earlier
                        // in the chain from treating Win-up as standalone.
                        if !send_mask_key() {
                            queue_action(Action::MaskFailed);
                        }
                        return LRESULT(1);
                    }
                    Decision::Toggle(side) => {
                        queue_toggle(side);
                        return LRESULT(1);
                    }
                    Decision::Replay { side, key } => {
                        if let Some(tx) = ACTION_TX.get() {
                            let _ = tx.send(Action::Replay { side, key });
                        }
                        return LRESULT(1);
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

const RAW_INPUT_WINDOW_CLASS: &str = "PrismRawKeyboardInput";

unsafe fn ensure_raw_input_class() -> Result<(), String> {
    RAW_INPUT_CLASS
        .get_or_init(|| {
            let module =
                GetModuleHandleW(None).map_err(|error| format!("get Prism module: {error}"))?;
            let class_name = wide(RAW_INPUT_WINDOW_CLASS);
            let class = WNDCLASSW {
                hInstance: HINSTANCE(module.0),
                lpfnWndProc: Some(raw_input_window_proc),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                Err("register raw keyboard window class failed".to_string())
            } else {
                Ok(())
            }
        })
        .clone()
}

unsafe fn create_raw_input_window() -> Result<HWND, String> {
    ensure_raw_input_class()?;
    let module = GetModuleHandleW(None).map_err(|error| format!("get Prism module: {error}"))?;
    let instance = HINSTANCE(module.0);
    let class_name = wide(RAW_INPUT_WINDOW_CLASS);
    let window = match CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        PCWSTR(class_name.as_ptr()),
        PCWSTR::null(),
        WINDOW_STYLE::default(),
        0,
        0,
        0,
        0,
        Some(HWND_MESSAGE),
        None,
        Some(instance),
        None,
    ) {
        Ok(window) => window,
        Err(error) => return Err(format!("create raw keyboard window: {error}")),
    };
    let device = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_KEYBOARD,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: window,
    };
    if let Err(error) =
        RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32)
    {
        let _ = DestroyWindow(window);
        return Err(format!("register raw keyboard input: {error}"));
    }
    Ok(window)
}

unsafe fn destroy_raw_input_window(window: HWND) {
    let remove = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_KEYBOARD,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: HWND::default(),
    };
    let _ = RegisterRawInputDevices(&[remove], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    let _ = DestroyWindow(window);
}

unsafe extern "system" fn raw_input_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_INPUT
        && ACTIVE.load(Ordering::Acquire)
        && RAW_PROVIDER_ACTIVE.load(Ordering::Acquire)
    {
        let mut input = RAWINPUT::default();
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        let read = GetRawInputData(
            HRAWINPUT(lparam.0 as *mut _),
            RID_INPUT,
            Some((&mut input as *mut RAWINPUT).cast()),
            &mut size,
            std::mem::size_of::<RAWINPUTHEADER>() as u32,
        );
        let keyboard_bytes =
            (std::mem::size_of::<RAWINPUTHEADER>() + std::mem::size_of::<RAWKEYBOARD>()) as u32;
        if read != u32::MAX && read >= keyboard_bytes && input.header.dwType == RIM_TYPEKEYBOARD.0 {
            let keyboard = input.data.keyboard;
            let message_down = keyboard.Message == WM_KEYDOWN || keyboard.Message == WM_SYSKEYDOWN;
            let message_up = keyboard.Message == WM_KEYUP || keyboard.Message == WM_SYSKEYUP;
            let is_up = keyboard.Flags as u32 & RI_KEY_BREAK != 0;
            if keyboard.MakeCode as u32 != KEYBOARD_OVERRUN_MAKE_CODE
                && keyboard.VKey < 255
                && ((message_down && !is_up) || (message_up && is_up))
            {
                let kind = if keyboard.VKey == VK_LWIN.0 {
                    KeyKind::Win(WinSide::Left)
                } else if keyboard.VKey == VK_RWIN.0 {
                    KeyKind::Win(WinSide::Right)
                } else {
                    KeyKind::Other(keyboard.VKey)
                };
                let decision = RAW_MACHINE
                    .lock()
                    .map(|mut machine| machine.feed(kind, !is_up))
                    .unwrap_or(Decision::Pass);
                if let Decision::Toggle(side) = decision {
                    queue_toggle(side);
                }
            }
        }
    }
    DefWindowProcW(window, message, wparam, lparam)
}

/// Sends a Win-down + key-down pair in one `SendInput` call so the combo
/// exists as a single atomic sequence. Returns whether every event was
/// accepted (a partial or zero send means the foreground app is elevated
/// or the input was rejected elsewhere).
unsafe fn send_combo(win_vk: u16, key_vk: u16) -> bool {
    let inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(win_vk),
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    dwExtraInfo: SYNTH_TAG,
                    ..Default::default()
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(key_vk),
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    dwExtraInfo: SYNTH_TAG,
                    ..Default::default()
                },
            },
        },
    ];
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) == REPLAY_EVENTS
}

unsafe fn send_mask_key() -> bool {
    let inputs = [
        keyboard_input(MENU_MASK_KEY, KEYBD_EVENT_FLAGS(0)),
        keyboard_input(MENU_MASK_KEY, KEYEVENTF_KEYUP),
    ];
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) == inputs.len() as u32
}

/// Covers delayed Start activation without blocking the low-level hook
/// thread. Repeated toggles share the active guard instead of spawning an
/// unbounded number of workers.
fn start_menu_guard() {
    if START_GUARD_WORKER.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(|| {
        loop {
            if !ACTIVE.load(Ordering::Acquire) {
                break;
            }
            if unsafe { dismiss_start_menus() } {
                refocus_palette();
            }
            let now = toggle_clock_ms();
            if now >= START_GUARD_UNTIL_MS.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(START_GUARD_INTERVAL);
        }
        START_GUARD_ACTIVE.store(false, Ordering::Release);
        START_GUARD_WORKER.store(false, Ordering::Release);
        refresh_hook_priority();

        // A new standalone press can arm the guard while the old worker is
        // exiting. Its queued toggle will start a fresh worker; keep the
        // active flag intact for the show-event callback in the meantime.
        if ACTIVE.load(Ordering::Acquire)
            && toggle_clock_ms() < START_GUARD_UNTIL_MS.load(Ordering::Acquire)
        {
            START_GUARD_ACTIVE.store(true, Ordering::Release);
            start_menu_guard();
        }
    });
}

fn arm_start_menu_guard() {
    let until = toggle_clock_ms() + START_GUARD_DURATION.as_millis() as u64;
    START_GUARD_UNTIL_MS.fetch_max(until, Ordering::AcqRel);
    START_GUARD_ACTIVE.store(true, Ordering::Release);
}

fn toggle_clock_ms() -> u64 {
    TOGGLE_CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

fn queue_toggle(side: WinSide) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let now = toggle_clock_ms();
    loop {
        let previous = LAST_TOGGLE_MS.load(Ordering::Acquire);
        if previous != 0 && now.saturating_sub(previous) < TOGGLE_DEBOUNCE_MS {
            return;
        }
        if LAST_TOGGLE_MS
            .compare_exchange(previous, now, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            break;
        }
    }
    if let Some(tx) = ACTION_TX.get() {
        // The installed provider has already been configured to consume
        // standalone Start requests. The guard is only needed by the generic
        // fallback used on systems without that integration.
        if !SHELL_SUPPRESSES_START.load(Ordering::Acquire) {
            arm_start_menu_guard();
        }
        if tx.send(Action::Toggle(side)).is_ok() {
            let thread_id = THREAD_ID.load(Ordering::SeqCst);
            if thread_id != 0 {
                unsafe {
                    let _ = PostThreadMessageW(thread_id, ACTION_MESSAGE, WPARAM(0), LPARAM(0));
                }
            }
        } else {
            START_GUARD_ACTIVE.store(false, Ordering::Release);
        }
    }
}

fn queue_action(action: Action) {
    if let Some(tx) = ACTION_TX.get() {
        if tx.send(action).is_ok() {
            let thread_id = THREAD_ID.load(Ordering::SeqCst);
            if thread_id != 0 {
                unsafe {
                    let _ = PostThreadMessageW(thread_id, ACTION_MESSAGE, WPARAM(0), LPARAM(0));
                }
            }
        }
    }
}

unsafe fn dismiss_start_menus() -> bool {
    // A replacement menu can remain visible behind Prism after Prism takes
    // focus, so foreground-only detection is insufficient. Enumerate only
    // exact, documented menu-container classes; never touch their hook or
    // settings windows.
    let mut dismissed = false;
    let state = &mut dismissed as *mut bool;
    let _ = EnumWindows(
        Some(dismiss_replacement_start_window),
        LPARAM(state as isize),
    );
    match foreground_start_menu() {
        Some((_window, StartMenuHost::Native)) => {
            let inputs = [
                keyboard_input(VK_ESCAPE, KEYBD_EVENT_FLAGS(0)),
                keyboard_input(VK_ESCAPE, KEYEVENTF_KEYUP),
            ];
            dismissed |=
                SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) == inputs.len() as u32;
        }
        None => {}
        Some((_window, StartMenuHost::StartAllBack | StartMenuHost::OpenShell)) => {}
    }
    dismissed
}

unsafe extern "system" fn dismiss_replacement_start_window(window: HWND, state: LPARAM) -> BOOL {
    if IsWindowVisible(window).as_bool() {
        let mut class_name = [0u16; 128];
        let class_len = GetClassNameW(window, &mut class_name).max(0) as usize;
        match start_menu_class(&class_name[..class_len]) {
            Some(StartMenuHost::StartAllBack | StartMenuHost::OpenShell) => {
                let _ = ShowWindow(window, SW_HIDE);
                if state.0 != 0 {
                    *(state.0 as *mut bool) = true;
                }
            }
            Some(StartMenuHost::Native) | None => {}
        }
    }
    BOOL(1)
}

fn refocus_palette() {
    let Some(app) = APP.get() else {
        return;
    };
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let focus_window = window.clone();
    let _ = app.run_on_main_thread(move || {
        if focus_window.is_visible().unwrap_or(false) {
            let _ = focus_window.set_focus();
        }
    });
}

fn keyboard_input(key: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: flags,
                dwExtraInfo: SYNTH_TAG,
                ..Default::default()
            },
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartMenuHost {
    StartAllBack,
    OpenShell,
    Native,
}

fn start_menu_class(class_name: &[u16]) -> Option<StartMenuHost> {
    if wide_eq_ascii(class_name, "DV2ControlHost")
        || wide_eq_ascii(class_name, "SIBTranslucentLayer")
    {
        Some(StartMenuHost::StartAllBack)
    } else if wide_eq_ascii(class_name, "OpenShell.CMenuContainer")
        || wide_eq_ascii(class_name, "ClassicShell.CMenuContainer")
    {
        Some(StartMenuHost::OpenShell)
    } else {
        None
    }
}

fn is_native_start_process(executable: &[u16]) -> bool {
    wide_eq_ascii(executable, "StartMenuExperienceHost.exe")
        || wide_eq_ascii(executable, "ShellExperienceHost.exe")
}

fn is_search_host_start_surface(class_name: &[u16], executable: &[u16]) -> bool {
    wide_eq_ascii(class_name, "Windows.UI.Core.CoreWindow")
        && wide_eq_ascii(executable, "SearchHost.exe")
}

unsafe fn foreground_start_menu() -> Option<(HWND, StartMenuHost)> {
    let window = GetForegroundWindow();
    if window.0.is_null() {
        return None;
    }

    start_menu_host(window).map(|host| (window, host))
}

unsafe fn start_menu_host(window: HWND) -> Option<StartMenuHost> {
    let mut class_name = [0u16; 128];
    let class_len = GetClassNameW(window, &mut class_name).max(0) as usize;
    if let Some(host) = start_menu_class(&class_name[..class_len]) {
        return Some(host);
    }

    let executable = window_process_executable(window)?;
    if is_native_start_process(&executable)
        || is_search_host_start_surface(&class_name[..class_len], &executable)
    {
        Some(StartMenuHost::Native)
    } else {
        None
    }
}

unsafe fn window_process_executable(window: HWND) -> Option<Vec<u16>> {
    let mut process_id = 0u32;
    GetWindowThreadProcessId(window, Some(&mut process_id));
    if process_id == 0 {
        return None;
    }
    let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) else {
        return None;
    };
    let mut path = [0u16; 512];
    let mut path_len = path.len() as u32;
    let queried = QueryFullProcessImageNameW(
        process,
        PROCESS_NAME_WIN32,
        PWSTR(path.as_mut_ptr()),
        &mut path_len,
    )
    .is_ok();
    let _ = CloseHandle(process);
    queried.then(|| {
        path[..path_len as usize]
            .rsplit(|value| *value == b'\\' as u16 || *value == b'/' as u16)
            .next()
            .unwrap_or_default()
            .to_vec()
    })
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn wide_eq_ascii(wide: &[u16], ascii: &str) -> bool {
    wide.len() == ascii.len()
        && wide
            .iter()
            .zip(ascii.bytes())
            .all(|(left, right)| ascii_lower(*left) == ascii_lower(right as u16))
}

fn ascii_lower(value: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&value) {
        value + 32
    } else {
        value
    }
}

/* ------------------------------------------------------------------ */
/*  Unit tests - the full shortcut matrix                              */
/* ------------------------------------------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;

    fn win(side: WinSide) -> KeyKind {
        KeyKind::Win(side)
    }

    /// Asserts a full sequence yields exactly the expected decisions.
    fn run(events: &[(KeyKind, bool)]) -> Vec<Decision> {
        let mut m = WinKeyMachine::default();
        events.iter().map(|(k, d)| m.feed(*k, *d)).collect()
    }

    #[test]
    fn start_menu_identity_matching_is_case_insensitive_and_exact() {
        let wide = |value: &str| value.encode_utf16().collect::<Vec<_>>();
        assert_eq!(
            start_menu_class(&wide("dv2controlhost")),
            Some(StartMenuHost::StartAllBack)
        );
        assert_eq!(
            start_menu_class(&wide("sibtranslucentlayer")),
            Some(StartMenuHost::StartAllBack)
        );
        assert_eq!(
            start_menu_class(&wide("OpenShell.CMenuContainer")),
            Some(StartMenuHost::OpenShell)
        );
        assert_eq!(
            start_menu_class(&wide("CLASSICSHELL.CMENUCONTAINER")),
            Some(StartMenuHost::OpenShell)
        );
        assert_eq!(start_menu_class(&wide("CabinetWClass")), None);
        assert_eq!(start_menu_class(&wide("OpenShell.CStartHookWindow")), None);
        assert!(is_native_start_process(&wide(
            "StartMenuExperienceHost.exe"
        )));
        assert!(is_native_start_process(&wide("shellexperiencehost.exe")));
        assert!(!is_native_start_process(&wide("explorer.exe")));
        assert!(is_search_host_start_surface(
            &wide("Windows.UI.Core.CoreWindow"),
            &wide("SearchHost.exe")
        ));
        assert!(!is_search_host_start_surface(
            &wide("ServiceWorkerGlobalScopeHost Window Class"),
            &wide("SearchHost.exe")
        ));
        assert!(!is_search_host_start_surface(
            &wide("Windows.UI.Core.CoreWindow"),
            &wide("TextInputHost.exe")
        ));
    }

    #[test]
    fn standalone_left_win_toggles() {
        assert_eq!(
            run(&[(win(WinSide::Left), true), (win(WinSide::Left), false)]),
            vec![Decision::Mask, Decision::Toggle(WinSide::Left)]
        );
    }

    #[test]
    fn standalone_right_win_toggles_with_its_own_side() {
        assert_eq!(
            run(&[(win(WinSide::Right), true), (win(WinSide::Right), false)]),
            vec![Decision::Mask, Decision::Toggle(WinSide::Right)]
        );
    }

    #[test]
    fn held_win_repeats_stay_suppressed_and_toggle_once() {
        assert_eq!(
            run(&[
                (win(WinSide::Left), true),
                (win(WinSide::Left), true), // auto-repeat
                (win(WinSide::Left), false),
            ]),
            vec![
                Decision::Mask,
                Decision::Swallow,
                Decision::Toggle(WinSide::Left)
            ]
        );
    }

    #[test]
    fn rapid_taps_toggle_every_time() {
        let mut m = WinKeyMachine::default();
        for _ in 0..3 {
            assert_eq!(m.feed(win(WinSide::Left), true), Decision::Mask);
            assert_eq!(
                m.feed(win(WinSide::Left), false),
                Decision::Toggle(WinSide::Left)
            );
        }
        assert!(!m.left.down && !m.right.down);
    }

    #[test]
    fn left_win_plus_e_replays_left() {
        assert_eq!(
            run(&[
                (win(WinSide::Left), true),
                (KeyKind::Other(0x45), true), // E
                (KeyKind::Other(0x45), false),
                (win(WinSide::Left), false),
            ]),
            vec![
                Decision::Mask,
                Decision::Replay {
                    side: WinSide::Left,
                    key: 0x45
                },
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn right_win_plus_e_replays_right_not_left() {
        let d = run(&[(win(WinSide::Right), true), (KeyKind::Other(0x45), true)]);
        assert_eq!(
            d[1],
            Decision::Replay {
                side: WinSide::Right,
                key: 0x45
            }
        );
    }

    #[test]
    fn multi_modifier_chord_win_ctrl_shift_b() {
        assert_eq!(
            run(&[
                (KeyKind::Other(0x11), true),  // Ctrl down - untouched
                (KeyKind::Other(0x10), true),  // Shift down - untouched
                (win(WinSide::Left), true),    // Win down - swallowed
                (KeyKind::Other(0x42), true),  // B down - replayed
                (KeyKind::Other(0x42), false), // B up
                (KeyKind::Other(0x10), false), // Shift up
                (KeyKind::Other(0x11), false), // Ctrl up
                (win(WinSide::Left), false),   // Win up passes (combo)
            ]),
            vec![
                Decision::Pass,
                Decision::Pass,
                Decision::Mask,
                Decision::Replay {
                    side: WinSide::Left,
                    key: 0x42
                },
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn win_shift_s_and_other_combos_pass_through() {
        for key in [
            0x45, /*E*/
            0x44, /*D*/
            0x4C, /*L*/
            0x52, /*R*/
            0x09, /*Tab*/
            0x58, /*X*/
            0x47, /*G*/
            0x49, /*I*/
            0x56, /*V*/
            0x53, /*S*/
        ] {
            let d = run(&[
                (win(WinSide::Left), true),
                (KeyKind::Other(0x10), true), // Shift
                (KeyKind::Other(key), true),
                (KeyKind::Other(key), false),
                (KeyKind::Other(0x10), false),
                (win(WinSide::Left), false),
            ]);
            assert_eq!(
                d,
                vec![
                    Decision::Mask,
                    Decision::Pass,
                    Decision::Replay {
                        side: WinSide::Left,
                        key
                    },
                    Decision::Pass,
                    Decision::Pass,
                    Decision::Pass,
                ],
                "key {key:#x}"
            );
        }
    }

    #[test]
    fn modifiers_while_win_held_pass_but_win_does_not_toggle() {
        // Win+Shift alone must not open the palette on release.
        assert_eq!(
            run(&[
                (win(WinSide::Left), true),
                (KeyKind::Other(0x10), true), // Shift
                (KeyKind::Other(0x10), false),
                (win(WinSide::Left), false),
            ]),
            vec![
                Decision::Mask,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass
            ]
        );
        // Same for Ctrl and Alt (both left/right variants).
        for mod_key in [0x11, 0x12, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5] {
            let d = run(&[
                (win(WinSide::Left), true),
                (KeyKind::Other(mod_key), true),
                (KeyKind::Other(mod_key), false),
                (win(WinSide::Left), false),
            ]);
            assert_eq!(
                d,
                vec![
                    Decision::Mask,
                    Decision::Pass,
                    Decision::Pass,
                    Decision::Pass
                ],
                "modifier {mod_key:#x}"
            );
        }
    }

    #[test]
    fn shift_then_win_then_key_works() {
        // Shift pressed before Win still yields a working chord.
        assert_eq!(
            run(&[
                (KeyKind::Other(0x10), true), // Shift first
                (win(WinSide::Left), true),
                (KeyKind::Other(0x53), true), // S
                (KeyKind::Other(0x53), false),
                (win(WinSide::Left), false),
                (KeyKind::Other(0x10), false),
            ]),
            vec![
                Decision::Pass,
                Decision::Mask,
                Decision::Replay {
                    side: WinSide::Left,
                    key: 0x53
                },
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn both_wins_held_replays_left_first_and_releases_pass() {
        assert_eq!(
            run(&[
                (win(WinSide::Left), true),
                (win(WinSide::Right), true),
                (KeyKind::Other(0x45), true),
                (KeyKind::Other(0x45), false),
                (win(WinSide::Right), false),
                (win(WinSide::Left), false),
            ]),
            vec![
                Decision::Mask,
                Decision::Mask,
                Decision::Replay {
                    side: WinSide::Left,
                    key: 0x45
                },
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn combo_then_standalone_press_toggles() {
        let mut m = WinKeyMachine::default();
        // Win+E combo
        assert_eq!(m.feed(win(WinSide::Left), true), Decision::Mask);
        assert_eq!(
            m.feed(KeyKind::Other(0x45), true),
            Decision::Replay {
                side: WinSide::Left,
                key: 0x45
            }
        );
        assert_eq!(m.feed(KeyKind::Other(0x45), false), Decision::Pass);
        assert_eq!(m.feed(win(WinSide::Left), false), Decision::Pass);
        // Standalone again
        assert_eq!(m.feed(win(WinSide::Left), true), Decision::Mask);
        assert_eq!(
            m.feed(win(WinSide::Left), false),
            Decision::Toggle(WinSide::Left)
        );
    }

    #[test]
    fn win_released_before_key_still_passes_key_up() {
        assert_eq!(
            run(&[
                (win(WinSide::Left), true),
                (KeyKind::Other(0x45), true),
                (win(WinSide::Left), false), // released first (combo)
                (KeyKind::Other(0x45), false),
            ]),
            vec![
                Decision::Mask,
                Decision::Replay {
                    side: WinSide::Left,
                    key: 0x45
                },
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn disable_mid_press_resets_and_never_toggles() {
        let mut m = WinKeyMachine::default();
        assert_eq!(m.feed(win(WinSide::Left), true), Decision::Mask);
        m.reset();
        assert!(!m.left.down && !m.right.down);
        // The stray release after reset must pass, never toggle.
        assert_eq!(m.feed(win(WinSide::Left), false), Decision::Pass);
    }

    #[test]
    fn stray_release_without_press_passes() {
        assert_eq!(m_feed_single(KeyKind::Other(0x41), false), Decision::Pass);
        assert_eq!(m_feed_single(win(WinSide::Right), false), Decision::Pass);
    }

    #[test]
    fn non_win_keys_never_touched() {
        let mut m = WinKeyMachine::default();
        for key in [0x41, 0x0D, 0x20, 0x1B] {
            assert_eq!(m.feed(KeyKind::Other(key), true), Decision::Pass);
            assert_eq!(m.feed(KeyKind::Other(key), false), Decision::Pass);
        }
    }

    fn m_feed_single(kind: KeyKind, is_down: bool) -> Decision {
        let mut m = WinKeyMachine::default();
        m.feed(kind, is_down)
    }
}
