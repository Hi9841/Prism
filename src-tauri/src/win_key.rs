//! Win-key observation (disabled by default).
//!
//! Turns a standalone Win-key press into a Prism toggle while leaving
//! Win+* combos and other global keyboard tools intact. Design rules:
//!
//! - A pure, side-aware state machine (`WinKeyMachine`) decides everything;
//!   it is unit-tested without Win32 involvement.
//! - A message-only raw-input observer detects standalone Win in every mode.
//!   Prism never installs a low-level keyboard hook or modifies physical input.
//! - StartAllBack integration disables the provider's Win action reversibly.
//! - A small Explorer message hook takes ownership of `SC_TASKLIST` before
//!   native Start is launched. Without provider integration, it also releases
//!   Explorer's bare-Win hotkey. It fails open if Prism's observer disappears.
//! - The Start button is found by UI Automation ID (with a child-window class
//!   fallback), and an Explorer-thread mouse hook consumes clicks in its rect.
//! - Every Win32 result is checked. Registration failures disable observation.
//! - Disabling, quitting or failing mid-keypress resets all observation state.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use windows::core::{BOOL, PCSTR, PCWSTR};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HID_USAGE_GENERIC_KEYBOARD, HID_USAGE_PAGE_GENERIC, KEYBOARD_OVERRUN_MAKE_CODE,
};
use windows::Win32::Foundation::{
    FreeLibrary, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElement,
    TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_ProcessIdPropertyId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
    VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RAWKEYBOARD, RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    EnumChildWindows, FindWindowW, GetClassNameW, GetShellWindow, GetWindowRect,
    GetWindowThreadProcessId, IsWindowVisible, MsgWaitForMultipleObjectsEx, PeekMessageW,
    PostThreadMessageW, RegisterClassW, RegisterWindowMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, HWND_MESSAGE, MSG, MSGFLT_ALLOW, PM_REMOVE,
    QS_ALLINPUT, RI_KEY_BREAK, WH_GETMESSAGE, WH_MOUSE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW,
};

const ACTION_MESSAGE: u32 = WM_APP + 1;
const TOGGLE_DEBOUNCE_MS: u64 = 50;
const WIN_TOGGLE_RELEASE_GRACE: Duration = Duration::from_millis(30);
/// The Start button rect only needs refreshing occasionally; the UIA query is
/// expensive and runs on Explorer's side.
const START_RECT_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const SHELL_BRIDGE_MESSAGE_NAME: &str = "Prism.ShellBridge.v1";
const SHELL_CONTROL_DISABLE_WIN_HOTKEY: usize = 1;
const SHELL_EVENT_HOTKEY_DISABLED: usize = 2;
const SHELL_CONTROL_START_RECT_LEFT: usize = 4;
const SHELL_CONTROL_START_RECT_TOP: usize = 5;
const SHELL_CONTROL_START_RECT_RIGHT: usize = 6;
const SHELL_CONTROL_START_RECT_BOTTOM: usize = 7;
const SHELL_EVENT_START_RECT_CONFIGURED: usize = 8;
const SHELL_EVENT_TASKBAR_START_CLICK_X: usize = 9;
const SHELL_EVENT_TASKBAR_START_CLICK_Y: usize = 10;
const SHELL_CONTROL_START_ICON_REFRESH: usize = 11;
const SHELL_CONTROL_START_ICON_SHUTDOWN: usize = 12;
const SHELL_EVENT_START_ICON_SHUTDOWN: usize = 13;
const SHELL_EVENT_START_ICON_REFRESHED: usize = 14;
const SHELL_CONTROL_SEARCH_RECT_LEFT: usize = 15;
const SHELL_CONTROL_SEARCH_RECT_TOP: usize = 16;
const SHELL_CONTROL_SEARCH_RECT_RIGHT: usize = 17;
const SHELL_CONTROL_SEARCH_RECT_BOTTOM: usize = 18;
const SHELL_EVENT_SEARCH_RECT_CONFIGURED: usize = 19;
const SHELL_CONTROL_TASKBAR_PIN: usize = 20;
const SHELL_CONTROL_TASKBAR_UNPIN: usize = 21;
const SHELL_EVENT_TASKBAR_PIN_COMPLETED: usize = 22;

/// Event the frontend receives when Win observation self-disables.
pub const FAILED_EVENT: &str = "win-mode-failed";

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
    /// A standalone Win press began; arm generic shell suppression.
    Mask,
    /// Deliver the Win-up and toggle the palette (a standalone press completed).
    Toggle(WinSide),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Press {
    down: bool,
    combo: bool,
}

const VK_CONTROL_CODE: u16 = VK_CONTROL.0;
const VK_LCONTROL_CODE: u16 = VK_LCONTROL.0;
const VK_RCONTROL_CODE: u16 = VK_RCONTROL.0;
const VK_ESCAPE_CODE: u16 = VK_ESCAPE.0;

fn is_ctrl_key(key: u16) -> bool {
    key == VK_CONTROL_CODE || key == VK_LCONTROL_CODE || key == VK_RCONTROL_CODE
}

fn is_escape_key(key: u16) -> bool {
    key == VK_ESCAPE_CODE
}

/// Side-aware press tracking. Both Windows keys and Ctrl+Esc chords keep
/// independent state so their identity and balanced down/up transitions are
/// preserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinKeyMachine {
    left: Press,
    right: Press,
    ctrl_esc: Press,
    non_win_down: [bool; 256],
}

impl Default for WinKeyMachine {
    fn default() -> Self {
        Self::EMPTY
    }
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
        ctrl_esc: Press {
            down: false,
            combo: false,
        },
        non_win_down: [false; 256],
    };
    /// Feeds one key event into the machine and returns the decision.
    pub fn feed(&mut self, kind: KeyKind, is_down: bool) -> Decision {
        match kind {
            KeyKind::Win(side) => {
                let other_win_down = match side {
                    WinSide::Left => self.right.down,
                    WinSide::Right => self.left.down,
                };
                let preexisting_chord =
                    other_win_down || self.non_win_down.iter().any(|down| *down);
                if is_down && other_win_down {
                    match side {
                        WinSide::Left => self.right.combo = true,
                        WinSide::Right => self.left.combo = true,
                    }
                }
                if is_down && self.ctrl_esc.down {
                    self.ctrl_esc.combo = true;
                }
                let press = self.press_mut(side);
                if is_down {
                    if press.down {
                        // Auto-repeat or extra press while already held.
                        Decision::Pass
                    } else {
                        press.down = true;
                        press.combo = preexisting_chord;
                        if preexisting_chord {
                            Decision::Pass
                        } else {
                            Decision::Mask
                        }
                    }
                } else if press.down {
                    let standalone = !press.combo;
                    press.down = false;
                    press.combo = false;
                    if standalone {
                        Decision::Toggle(side)
                    } else {
                        Decision::Pass
                    }
                } else {
                    Decision::Pass
                }
            }
            KeyKind::Other(key) => {
                let left_down = self.left.down;
                let right_down = self.right.down;
                let ctrl_esc_down = self.ctrl_esc.down;
                if left_down || right_down {
                    // Windows can deliver only the release for a Win chord
                    // through raw input. Any non-Win event while Win is held
                    // proves this was not a standalone press.
                    self.left.combo |= left_down;
                    self.right.combo |= right_down;
                }
                if is_down && ctrl_esc_down && !is_ctrl_key(key) && !is_escape_key(key) {
                    // Another key pressed while Ctrl+Esc held: remember combo
                    // so the Esc release is not mistaken for a standalone toggle.
                    self.ctrl_esc.combo = true;
                }

                if is_escape_key(key) {
                    let win_down = left_down || right_down;
                    let other_down = self.has_other_non_win_down(&[
                        VK_CONTROL_CODE,
                        VK_LCONTROL_CODE,
                        VK_RCONTROL_CODE,
                        VK_ESCAPE_CODE,
                    ]);
                    let clean_ctrl_esc = self.is_ctrl_down() && !win_down && !other_down;

                    if let Some(down) = self.non_win_down.get_mut(key as usize) {
                        *down = is_down;
                    }

                    if is_down {
                        if self.ctrl_esc.down {
                            // Auto-repeat while already held.
                            Decision::Pass
                        } else if clean_ctrl_esc {
                            self.ctrl_esc.down = true;
                            self.ctrl_esc.combo = false;
                            Decision::Mask
                        } else {
                            Decision::Pass
                        }
                    } else if self.ctrl_esc.down {
                        let standalone = !self.ctrl_esc.combo;
                        self.ctrl_esc.down = false;
                        self.ctrl_esc.combo = false;
                        if standalone {
                            Decision::Toggle(WinSide::Left)
                        } else {
                            Decision::Pass
                        }
                    } else {
                        Decision::Pass
                    }
                } else {
                    if let Some(down) = self.non_win_down.get_mut(key as usize) {
                        *down = is_down;
                    }
                    Decision::Pass
                }
            }
        }
    }

    /// Clears all press state when Win observation is disabled mid-keypress.
    pub fn reset(&mut self) {
        self.left = Press::default();
        self.right = Press::default();
        self.ctrl_esc = Press::default();
        self.non_win_down.fill(false);
    }

    fn press_mut(&mut self, side: WinSide) -> &mut Press {
        match side {
            WinSide::Left => &mut self.left,
            WinSide::Right => &mut self.right,
        }
    }

    fn is_ctrl_down(&self) -> bool {
        self.non_win_down[VK_CONTROL_CODE as usize]
            || self.non_win_down[VK_LCONTROL_CODE as usize]
            || self.non_win_down[VK_RCONTROL_CODE as usize]
    }

    fn has_other_non_win_down(&self, excluded_keys: &[u16]) -> bool {
        self.non_win_down
            .iter()
            .enumerate()
            .any(|(k, &down)| down && !excluded_keys.contains(&(k as u16)))
    }
}

/* ------------------------------------------------------------------ */
/*  Hook plumbing                                                       */
/* ------------------------------------------------------------------ */

static APP: OnceLock<AppHandle> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static PROVIDER_SUPPRESSES_START: AtomicBool = AtomicBool::new(false);
static RAW_OBSERVER_ACTIVE: AtomicBool = AtomicBool::new(false);
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
type HookReady = mpsc::SyncSender<Result<(), String>>;
type StopReady = mpsc::SyncSender<()>;

static START_TX: OnceLock<mpsc::Sender<HookReady>> = OnceLock::new();
static RAW_MACHINE: Mutex<WinKeyMachine> = Mutex::new(WinKeyMachine::EMPTY);
static SHELL_BRIDGE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Registered once; calling RegisterWindowMessageW on every raw-input message
/// (the window proc path) is wasteful.
static BRIDGE_MESSAGE_ID: OnceLock<Result<u32, String>> = OnceLock::new();
static SHELL_BRIDGE_ACK: AtomicU32 = AtomicU32::new(0);
static SHELL_START_RECT_ACK: AtomicU32 = AtomicU32::new(0);
static SHELL_SEARCH_RECT_ACK: AtomicU32 = AtomicU32::new(0);
static SHELL_START_CLICK_X: AtomicI32 = AtomicI32::new(0);
static SHELL_TASKBAR_THREAD: AtomicU32 = AtomicU32::new(0);
static SHELL_ICON_SHUTDOWN_ACK: AtomicU32 = AtomicU32::new(0);
static SHELL_TASKBAR_PIN_ACK: AtomicU32 = AtomicU32::new(0);
static SHELL_TASKBAR_PIN_REQUEST: Mutex<()> = Mutex::new(());
static LAST_TOGGLE_MS: AtomicU64 = AtomicU64::new(0);
static TOGGLE_CLOCK: OnceLock<Instant> = OnceLock::new();
static PENDING_WIN_TOGGLE: Mutex<PendingWinToggle> = Mutex::new(PendingWinToggle::EMPTY);
/// Set when taskbar geometry changes (alignment moves, resizes) so the pump
/// refreshes the Start rect immediately instead of up to 5 seconds later.
static START_RECT_REFRESH_REQUEST: AtomicBool = AtomicBool::new(false);

enum Action {
    ToggleWin(WinSide),
    ToggleTaskbar(POINT),
}

struct PendingWinToggle {
    side: Option<WinSide>,
    blocked_keys: [bool; 256],
    post_release_keys: [bool; 256],
    deadline: Option<Instant>,
}

impl PendingWinToggle {
    const EMPTY: Self = Self {
        side: None,
        blocked_keys: [false; 256],
        post_release_keys: [false; 256],
        deadline: None,
    };

    fn arm(&mut self, side: WinSide, blocked_keys: [bool; 256], now: Instant) {
        self.side = Some(side);
        self.blocked_keys = blocked_keys;
        self.post_release_keys.fill(false);
        self.deadline = Some(now + WIN_TOGGLE_RELEASE_GRACE);
    }

    fn cancel(&mut self) {
        self.side = None;
        self.blocked_keys.fill(false);
        self.post_release_keys.fill(false);
        self.deadline = None;
    }

    fn observe_key(&mut self, key: u16, is_down: bool, now: Instant) {
        let key = canonical_non_win_key(key);
        if self.side.is_none() || key as usize >= self.blocked_keys.len() {
            return;
        }
        let key = key as usize;
        if is_down {
            // This transition occurred after Win-up, so a snapshot that saw
            // this key held reflected new typing rather than the Win chord.
            self.blocked_keys[key] = false;
            self.post_release_keys[key] = true;
            if self.deadline.is_none() {
                self.deadline = Some(now + WIN_TOGGLE_RELEASE_GRACE);
            }
            return;
        }
        if self.post_release_keys[key] {
            self.post_release_keys[key] = false;
            return;
        }
        // A key-up without a post-release key-down belongs to the Win chord.
        self.cancel();
    }

    fn take_if_ready(&mut self, now: Instant) -> Option<WinSide> {
        let deadline = self.deadline?;
        if now < deadline {
            return None;
        }
        if self.blocked_keys.iter().any(|down| *down) {
            // Wait for ordered input to identify whether the held key belonged
            // to the Win chord or was pressed after Win-up.
            self.deadline = None;
            return None;
        }
        let side = self.side.take();
        self.deadline = None;
        side
    }

    fn wait_duration(&self, now: Instant) -> Option<Duration> {
        self.side
            .and(self.deadline)
            .map(|deadline| deadline.saturating_duration_since(now))
    }
}

static ACTION_TX: OnceLock<mpsc::Sender<Action>> = OnceLock::new();
static ACTION_RX: Mutex<Option<mpsc::Receiver<Action>>> = Mutex::new(None);
static STOP_READY: Mutex<Option<StopReady>> = Mutex::new(None);
static RAW_INPUT_CLASS: OnceLock<Result<(), String>> = OnceLock::new();

pub fn init(app: AppHandle) {
    let _ = APP.set(app);
}

pub fn set_provider_suppression(active: bool) {
    PROVIDER_SUPPRESSES_START.store(active, Ordering::Release);
    RAW_MACHINE.lock().map(|mut machine| machine.reset()).ok();
    cancel_pending_win_toggle();
    clear_queued_actions();
}

pub(crate) fn notify_start_icon_changed() {
    let taskbar_thread = SHELL_TASKBAR_THREAD.load(Ordering::Acquire);
    let Ok(message) = shell_bridge_message() else {
        return;
    };
    if taskbar_thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(
                taskbar_thread,
                message,
                WPARAM(SHELL_CONTROL_START_ICON_REFRESH),
                LPARAM(0),
            );
        }
    }
}

pub(crate) fn shell_bridge_taskbar_pin(path: &Path, pinned: bool) -> Result<(), String> {
    if !SHELL_BRIDGE_ACTIVE.load(Ordering::Acquire) {
        return Err("shell bridge not active".into());
    }
    let _request = SHELL_TASKBAR_PIN_REQUEST
        .lock()
        .map_err(|_| "taskbar pin request lock is poisoned".to_string())?;
    let taskbar_thread = SHELL_TASKBAR_THREAD.load(Ordering::Acquire);
    if taskbar_thread == 0 {
        return Err("shell taskbar thread not available".into());
    }

    let target_file = std::env::temp_dir()
        .join("Prism")
        .join("taskbar-pin-target.txt");
    if let Some(parent) = target_file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create pin request directory: {error}"))?;
    }
    std::fs::write(&target_file, path.to_string_lossy().as_bytes())
        .map_err(|error| format!("failed to write pin target file: {error}"))?;

    SHELL_TASKBAR_PIN_ACK.store(0, Ordering::Release);
    let control = if pinned {
        SHELL_CONTROL_TASKBAR_PIN
    } else {
        SHELL_CONTROL_TASKBAR_UNPIN
    };
    unsafe {
        PostThreadMessageW(
            taskbar_thread,
            shell_bridge_message()?,
            WPARAM(control),
            LPARAM(0),
        )
        .map_err(|error| format!("failed to post pin request to Explorer: {error}"))?;
    }

    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(1500) {
        match SHELL_TASKBAR_PIN_ACK.load(Ordering::Acquire) {
            2 => return Ok(()),
            1 => return Err("Explorer rejected the taskbar pin request".into()),
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    Err("Explorer taskbar pin request timed out".into())
}

/// Asks the observation pump to re-query the Start button rectangle now.
/// Taskbar moves (alignment repair, density changes) should reposition the
/// glyph overlay immediately rather than on the next interval tick.
pub(crate) fn request_start_rect_refresh() {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    START_RECT_REFRESH_REQUEST.store(true, Ordering::Release);
    let thread_id = THREAD_ID.load(Ordering::SeqCst);
    if thread_id != 0 {
        unsafe {
            let _ = PostThreadMessageW(thread_id, ACTION_MESSAGE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Turns Win-key observation on or off. Disabling resets the machine and
/// tears down native observers immediately.
pub fn set_enabled(on: bool) -> Result<(), String> {
    if on == ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }
    ACTIVE.store(on, Ordering::SeqCst);
    if !on {
        SHELL_BRIDGE_ACTIVE.store(false, Ordering::Release);
        RAW_MACHINE.lock().map(|mut m| m.reset()).ok();
        cancel_pending_win_toggle();
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
                return Err("timed out while stopping Windows-key observation".to_string());
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
        .ok_or_else(|| "observer thread unavailable".to_string())?
        .send(ready_tx)
        .is_err()
    {
        ACTIVE.store(false, Ordering::SeqCst);
        return Err("observer thread unavailable".to_string());
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
            Err("timed out while starting Windows-key observation".to_string())
        }
    }
}

fn pump_loop(rx: mpsc::Receiver<HookReady>) {
    THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    while let Ok(ready) = rx.recv() {
        if ACTIVE.load(Ordering::SeqCst) {
            unsafe { run_pump(ready) };
        } else {
            let _ = ready.send(Err("Windows-key observation was cancelled".to_string()));
        }
    }
    THREAD_ID.store(0, Ordering::SeqCst);
}

/// Registers raw input and the Explorer Start-command bridge.
unsafe fn run_pump(ready: HookReady) {
    let provider_mode = PROVIDER_SUPPRESSES_START.load(Ordering::Acquire);
    let raw_input_window = match create_raw_input_window() {
        Ok(window) => window,
        Err(error) => {
            let _ = ready.send(Err(error));
            disable_observation("raw keyboard observer registration failed");
            return;
        }
    };
    RAW_OBSERVER_ACTIVE.store(true, Ordering::Release);
    let com_initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
    let mut shell_bridge = match ShellBridge::install(!provider_mode) {
        Ok(bridge) => {
            SHELL_BRIDGE_ACTIVE.store(true, Ordering::Release);
            bridge
        }
        Err(error) => {
            debug_trace(&format!("bridge-install-error {error}"));
            RAW_OBSERVER_ACTIVE.store(false, Ordering::Release);
            destroy_raw_input_window(raw_input_window);
            let _ = ready.send(Err(error));
            disable_observation("Explorer Start-command bridge installation failed");
            if com_initialized {
                CoUninitialize();
            }
            return;
        }
    };
    SHELL_TASKBAR_THREAD.store(shell_bridge.taskbar_thread, Ordering::Release);
    notify_start_icon_changed();
    let _ = ready.send(Ok(()));
    debug_trace("bridge-install-ok");
    let mut msg = MSG::default();
    'pump: loop {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_APP {
                break 'pump;
            }
            if msg.message == ACTION_MESSAGE {
                continue;
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        flush_pending_win_toggle();
        // Drain native actions outside the callback.
        if let Ok(mut rx_slot) = ACTION_RX.lock() {
            if let Some(rx) = rx_slot.as_mut() {
                while let Ok(action) = rx.try_recv() {
                    if !ACTIVE.load(Ordering::Acquire) {
                        break 'pump;
                    }
                    match action {
                        Action::ToggleWin(_side) => {
                            if let Some(app) = APP.get() {
                                let toggle_app = app.clone();
                                let start_rect = shell_bridge.start_rect();
                                let _ = app.run_on_main_thread(move || {
                                    crate::toggle_palette_from_win(&toggle_app, start_rect);
                                });
                            }
                        }
                        Action::ToggleTaskbar(click) => {
                            if let Some(app) = APP.get() {
                                let toggle_app = app.clone();
                                let start_rect = shell_bridge.start_rect();
                                let _ = app.run_on_main_thread(move || {
                                    crate::toggle_palette_from_taskbar(
                                        &toggle_app,
                                        click,
                                        start_rect,
                                    );
                                });
                            }
                        }
                    }
                }
            }
        }
        shell_bridge.refresh_start_rect();
        // The pump has no timer work faster than the Start-rect refresh
        // interval; input already wakes the loop through the message queue.
        // A long timeout removes a permanent 1 Hz wakeup from the hot path.
        let wait = pending_win_toggle_wait()
            .unwrap_or(START_RECT_REFRESH_INTERVAL)
            .min(START_RECT_REFRESH_INTERVAL);
        let _ = MsgWaitForMultipleObjectsEx(
            None,
            wait.as_millis().min(u128::from(u32::MAX)) as u32,
            QS_ALLINPUT,
            Default::default(),
        );
    }
    SHELL_BRIDGE_ACTIVE.store(false, Ordering::Release);
    SHELL_TASKBAR_THREAD.store(0, Ordering::Release);
    RAW_OBSERVER_ACTIVE.store(false, Ordering::Release);
    RAW_MACHINE.lock().map(|mut machine| machine.reset()).ok();
    cancel_pending_win_toggle();
    // Ask the Explorer-thread renderer to tear down its window while the hook
    // and observer are still alive, then release the bridge.
    drop(shell_bridge);
    destroy_raw_input_window(raw_input_window);
    if com_initialized {
        CoUninitialize();
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

/// Safe recovery path: stop observing and tell the frontend so the user can react.
#[cfg(debug_assertions)]
fn debug_trace(message: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(
        std::env::temp_dir()
            .join("Prism")
            .join("semantic-debug.log"),
    ) {
        let _ = writeln!(file, "{message}");
    }
}

#[cfg(not(debug_assertions))]
fn debug_trace(_message: &str) {}

fn disable_observation(reason: &str) {
    debug_trace(&format!("observation-disabled {reason}"));
    ACTIVE.store(false, Ordering::SeqCst);
    SHELL_BRIDGE_ACTIVE.store(false, Ordering::Release);
    RAW_MACHINE.lock().map(|mut m| m.reset()).ok();
    cancel_pending_win_toggle();
    clear_queued_actions();
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

type ShellHookProc = unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT;

struct ShellBridge {
    module: HMODULE,
    progman_hook: HHOOK,
    taskbar_message_hook: Option<HHOOK>,
    app_manager_hook: Option<HHOOK>,
    taskbar_mouse_hook: HHOOK,
    taskbar_thread: u32,
    start_button_locator: StartButtonLocator,
    start_rect: RECT,
    search_button_locator: SearchButtonLocator,
    search_rect: Option<RECT>,
    last_rect_refresh: Option<Instant>,
    library_path: PathBuf,
}

struct StartButtonLocator {
    taskbar: HWND,
    automation: Option<AutomationStartButton>,
}

struct AutomationStartButton {
    taskbar: IUIAutomationElement,
    condition: IUIAutomationCondition,
    /// Resolved Start button element. It stays valid while the taskbar lives,
    /// so bounding-rectangle refreshes reuse it instead of re-running the
    /// expensive `FindFirst` descendant traversal every interval. Re-resolved
    /// automatically when Explorer restarts (stale elements fail gracefully).
    cached: Option<IUIAutomationElement>,
}

impl ShellBridge {
    unsafe fn install(release_win_hotkey: bool) -> Result<Self, String> {
        let library_path = write_shell_hook_library()?;
        let library_wide = wide(&library_path.to_string_lossy());
        let module = LoadLibraryW(PCWSTR(library_wide.as_ptr()))
            .map_err(|error| format!("load Explorer bridge: {error}"))?;
        let hook_proc =
            match GetProcAddress(module, PCSTR(c"PrismShellGetMessageHook".as_ptr().cast())) {
                Some(proc) => {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, ShellHookProc>(proc)
                }
                None => {
                    let _ = FreeLibrary(module);
                    let _ = std::fs::remove_file(&library_path);
                    return Err("Explorer bridge export is missing".to_string());
                }
            };
        let mouse_hook_proc =
            match GetProcAddress(module, PCSTR(c"PrismShellMouseHook".as_ptr().cast())) {
                Some(proc) => {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, ShellHookProc>(proc)
                }
                None => {
                    let _ = FreeLibrary(module);
                    let _ = std::fs::remove_file(&library_path);
                    return Err("Explorer mouse bridge export is missing".to_string());
                }
            };

        let progman = match find_shell_window("Progman") {
            Ok(window) => window,
            Err(error) => {
                let _ = FreeLibrary(module);
                let _ = std::fs::remove_file(&library_path);
                return Err(error);
            }
        };
        let progman_thread = GetWindowThreadProcessId(progman, None);
        if progman_thread == 0 {
            let _ = FreeLibrary(module);
            let _ = std::fs::remove_file(&library_path);
            return Err("Explorer desktop thread is unavailable".to_string());
        }
        let progman_hook = match SetWindowsHookExW(
            WH_GETMESSAGE,
            Some(hook_proc),
            Some(HINSTANCE(module.0)),
            progman_thread,
        ) {
            Ok(hook) => hook,
            Err(error) => {
                let _ = FreeLibrary(module);
                let _ = std::fs::remove_file(&library_path);
                return Err(format!("hook Explorer Start command: {error}"));
            }
        };

        let taskbar = match find_shell_window("Shell_TrayWnd") {
            Ok(window) => window,
            Err(error) => {
                cleanup_shell_hook(progman_hook, module, &library_path);
                return Err(error);
            }
        };
        let taskbar_thread = GetWindowThreadProcessId(taskbar, None);
        if taskbar_thread == 0 {
            cleanup_shell_hook(progman_hook, module, &library_path);
            return Err("Explorer taskbar thread is unavailable".to_string());
        }
        let taskbar_message_hook = if taskbar_thread == progman_thread {
            None
        } else {
            match SetWindowsHookExW(
                WH_GETMESSAGE,
                Some(hook_proc),
                Some(HINSTANCE(module.0)),
                taskbar_thread,
            ) {
                Ok(hook) => Some(hook),
                Err(error) => {
                    cleanup_shell_hook(progman_hook, module, &library_path);
                    return Err(format!("hook Explorer taskbar command: {error}"));
                }
            }
        };
        let mut taskbar_process_id = 0;
        if GetWindowThreadProcessId(taskbar, Some(&mut taskbar_process_id)) == 0
            || taskbar_process_id == 0
        {
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module, &library_path);
            return Err("Explorer taskbar process is unavailable".to_string());
        }
        let (start_button_locator, start_rect) =
            match StartButtonLocator::new(taskbar, taskbar_process_id) {
                Ok(locator) => locator,
                Err(error) => {
                    if let Some(hook) = taskbar_message_hook {
                        let _ = UnhookWindowsHookEx(hook);
                    }
                    cleanup_shell_hook(progman_hook, module, &library_path);
                    return Err(error);
                }
            };
        let (search_button_locator, search_rect) =
            SearchButtonLocator::new(taskbar, taskbar_process_id);
        let taskbar_mouse_hook = match SetWindowsHookExW(
            WH_MOUSE,
            Some(mouse_hook_proc),
            Some(HINSTANCE(module.0)),
            taskbar_thread,
        ) {
            Ok(hook) => hook,
            Err(error) => {
                if let Some(hook) = taskbar_message_hook {
                    let _ = UnhookWindowsHookEx(hook);
                }
                cleanup_shell_hook(progman_hook, module, &library_path);
                return Err(format!("hook Explorer Start-button mouse path: {error}"));
            }
        };
        let bridge_message = match shell_bridge_message() {
            Ok(message) => message,
            Err(error) => {
                let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
                if let Some(hook) = taskbar_message_hook {
                    let _ = UnhookWindowsHookEx(hook);
                }
                cleanup_shell_hook(progman_hook, module, &library_path);
                return Err(error);
            }
        };
        SHELL_START_RECT_ACK.store(0, Ordering::Release);
        if let Err(error) = post_start_button_rect(taskbar_thread, bridge_message, start_rect) {
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module, &library_path);
            return Err(error);
        }
        if wait_for_ack(&SHELL_START_RECT_ACK, Duration::from_secs(1)) != 2 {
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module, &library_path);
            return Err("Explorer did not accept the Start-button rectangle".to_string());
        }

        SHELL_SEARCH_RECT_ACK.store(0, Ordering::Release);
        let _ = post_search_button_rect(taskbar_thread, bridge_message, search_rect);
        let _ = wait_for_ack(&SHELL_SEARCH_RECT_ACK, Duration::from_millis(250));

        if !release_win_hotkey {
            #[cfg(debug_assertions)]
            eprintln!("[win-key] Explorer Start-command bridge active (Progman thread {progman_thread}, taskbar thread {taskbar_thread})");
            return Ok(Self {
                module,
                progman_hook,
                taskbar_message_hook,
                app_manager_hook: None,
                taskbar_mouse_hook,
                taskbar_thread,
                start_button_locator,
                start_rect,
                search_button_locator,
                search_rect,
                last_rect_refresh: None,
                library_path,
            });
        }

        let app_manager = match find_shell_window("ApplicationManager_ImmersiveShellWindow") {
            Ok(window) => window,
            Err(error) => {
                let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
                if let Some(hook) = taskbar_message_hook {
                    let _ = UnhookWindowsHookEx(hook);
                }
                cleanup_shell_hook(progman_hook, module, &library_path);
                return Err(error);
            }
        };
        let app_manager_thread = GetWindowThreadProcessId(app_manager, None);
        if app_manager_thread == 0 {
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module, &library_path);
            return Err("Explorer application-manager thread is unavailable".to_string());
        }

        let app_manager_hook = if app_manager_thread == progman_thread {
            None
        } else {
            match SetWindowsHookExW(
                WH_GETMESSAGE,
                Some(hook_proc),
                Some(HINSTANCE(module.0)),
                app_manager_thread,
            ) {
                Ok(hook) => Some(hook),
                Err(error) => {
                    let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
                    if let Some(hook) = taskbar_message_hook {
                        let _ = UnhookWindowsHookEx(hook);
                    }
                    cleanup_shell_hook(progman_hook, module, &library_path);
                    return Err(format!("enter Explorer hotkey thread: {error}"));
                }
            }
        };

        SHELL_BRIDGE_ACK.store(0, Ordering::Release);
        let message = match shell_bridge_message() {
            Ok(message) => message,
            Err(error) => {
                if let Some(hook) = app_manager_hook {
                    let _ = UnhookWindowsHookEx(hook);
                }
                let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
                if let Some(hook) = taskbar_message_hook {
                    let _ = UnhookWindowsHookEx(hook);
                }
                cleanup_shell_hook(progman_hook, module, &library_path);
                return Err(error);
            }
        };
        if let Err(error) = PostThreadMessageW(
            app_manager_thread,
            message,
            WPARAM(SHELL_CONTROL_DISABLE_WIN_HOTKEY),
            LPARAM(0),
        ) {
            if let Some(hook) = app_manager_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module, &library_path);
            return Err(format!("ask Explorer to release bare Win: {error}"));
        }

        let acknowledged = wait_for_ack(&SHELL_BRIDGE_ACK, Duration::from_secs(1));
        if acknowledged == 0 {
            if let Some(hook) = app_manager_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module, &library_path);
            return Err("Explorer did not acknowledge the bare-Win handoff".to_string());
        }

        #[cfg(debug_assertions)]
        eprintln!(
            "[win-key] Explorer bridge active (Progman thread {progman_thread}, hotkey thread {app_manager_thread}, hotkey {})",
            if acknowledged == 2 {
                "released"
            } else {
                "already released"
            }
        );
        Ok(Self {
            module,
            progman_hook,
            taskbar_message_hook,
            app_manager_hook,
            taskbar_mouse_hook,
            taskbar_thread,
            start_button_locator,
            start_rect,
            search_button_locator,
            search_rect,
            last_rect_refresh: None,
            library_path,
        })
    }

    fn start_rect(&self) -> Option<RECT> {
        valid_rect(self.start_rect).then_some(self.start_rect)
    }

    #[allow(dead_code)]
    fn search_rect(&self) -> Option<RECT> {
        self.search_rect.filter(|r| valid_rect(*r))
    }

    fn refresh_start_rect(&mut self) {
        let requested = START_RECT_REFRESH_REQUEST.swap(false, Ordering::AcqRel);
        let now = Instant::now();
        if !requested
            && self
                .last_rect_refresh
                .is_some_and(|last| now.duration_since(last) < START_RECT_REFRESH_INTERVAL)
        {
            return;
        }
        self.last_rect_refresh = Some(now);

        let Ok(message) = shell_bridge_message() else {
            return;
        };

        if let Some(rect) = self.start_button_locator.rect() {
            if !same_rect(rect, self.start_rect)
                && post_start_button_rect(self.taskbar_thread, message, rect).is_ok()
            {
                self.start_rect = rect;
            }
        }

        let search_rect = self.search_button_locator.rect();
        let search_changed = match (search_rect, self.search_rect) {
            (Some(new_r), Some(old_r)) => !same_rect(new_r, old_r),
            (None, None) => false,
            _ => true,
        };
        if search_changed
            && post_search_button_rect(self.taskbar_thread, message, search_rect).is_ok()
        {
            self.search_rect = search_rect;
        }
    }
}

impl StartButtonLocator {
    fn new(taskbar: HWND, process_id: u32) -> Result<(Self, RECT), String> {
        let automation = unsafe { create_automation_start_button(taskbar, process_id).ok() };
        let mut locator = Self {
            taskbar,
            automation,
        };
        let rect = locator.rect().ok_or_else(|| {
            "Start button was not found by AutomationId or taskbar child class".to_string()
        })?;
        Ok((locator, rect))
    }

    fn rect(&mut self) -> Option<RECT> {
        if let Some(automation) = self.automation.as_mut() {
            let rect = if let Some(start) = automation.cached.as_ref() {
                let rect = unsafe { start.CurrentBoundingRectangle() };
                if let Ok(rect) = rect {
                    if valid_rect(rect) {
                        return Some(rect);
                    }
                }
                // The element went stale (Explorer restarted). Re-resolve it.
                automation.cached = None;
                None
            } else {
                None
            };
            let rect = rect.or_else(|| unsafe {
                automation
                    .taskbar
                    .FindFirst(TreeScope_Descendants, &automation.condition)
                    .ok()
                    .inspect(|start| automation.cached = Some(start.clone()))
                    .and_then(|start| start.CurrentBoundingRectangle().ok())
            });
            if let Some(rect) = rect {
                if valid_rect(rect) {
                    return Some(rect);
                }
            }
        }
        unsafe { child_start_button_rect(self.taskbar) }
    }
}

unsafe fn create_automation_start_button(
    taskbar_window: HWND,
    taskbar_process_id: u32,
) -> Result<AutomationStartButton, String> {
    let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        .map_err(|error| format!("create UI Automation client: {error}"))?;
    let taskbar = uia
        .ElementFromHandle(taskbar_window)
        .map_err(|error| format!("resolve Explorer taskbar automation root: {error}"))?;
    let automation_id: VARIANT = "StartButton".into();
    let automation_id_condition = uia
        .CreatePropertyCondition(UIA_AutomationIdPropertyId, &automation_id)
        .map_err(|error| format!("match StartButton AutomationId: {error}"))?;
    let process_id: VARIANT = (taskbar_process_id as i32).into();
    let process_condition = uia
        .CreatePropertyCondition(UIA_ProcessIdPropertyId, &process_id)
        .map_err(|error| format!("match Explorer process: {error}"))?;
    let condition = uia
        .CreateAndCondition(&automation_id_condition, &process_condition)
        .map_err(|error| format!("combine StartButton identity conditions: {error}"))?;
    Ok(AutomationStartButton {
        taskbar,
        condition,
        cached: None,
    })
}

unsafe fn child_start_button_rect(taskbar: HWND) -> Option<RECT> {
    let mut rect = None;
    let _ = EnumChildWindows(
        Some(taskbar),
        Some(find_start_button_child),
        LPARAM((&mut rect as *mut Option<RECT>) as isize),
    );
    rect
}

unsafe extern "system" fn find_start_button_child(window: HWND, detail: LPARAM) -> BOOL {
    if !IsWindowVisible(window).as_bool() {
        return BOOL(1);
    }
    let mut class_name = [0u16; 64];
    let length = GetClassNameW(window, &mut class_name);
    let class_name = &class_name[..length.max(0) as usize];
    if is_start_button_class(class_name) {
        let mut rect = RECT::default();
        if GetWindowRect(window, &mut rect).is_ok() && valid_rect(rect) {
            *(detail.0 as *mut Option<RECT>) = Some(rect);
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn is_start_button_class(class_name: &[u16]) -> bool {
    ascii_class_eq(class_name, "Start") || ascii_class_eq(class_name, "StartButton")
}

fn ascii_class_eq(class_name: &[u16], expected: &str) -> bool {
    class_name.len() == expected.len()
        && class_name
            .iter()
            .zip(expected.bytes())
            .all(|(actual, expected)| (*actual as u8).eq_ignore_ascii_case(&expected))
}

fn valid_rect(rect: RECT) -> bool {
    rect.right > rect.left && rect.bottom > rect.top
}

fn same_rect(left: RECT, right: RECT) -> bool {
    left.left == right.left
        && left.top == right.top
        && left.right == right.right
        && left.bottom == right.bottom
}

fn post_start_button_rect(thread: u32, message: u32, rect: RECT) -> Result<(), String> {
    for (control, coordinate) in [
        (SHELL_CONTROL_START_RECT_LEFT, rect.left),
        (SHELL_CONTROL_START_RECT_TOP, rect.top),
        (SHELL_CONTROL_START_RECT_RIGHT, rect.right),
        (SHELL_CONTROL_START_RECT_BOTTOM, rect.bottom),
    ] {
        unsafe {
            PostThreadMessageW(
                thread,
                message,
                WPARAM(control),
                LPARAM(coordinate as isize),
            )
            .map_err(|error| format!("configure Explorer Start-button rectangle: {error}"))?;
        }
    }
    Ok(())
}

fn post_search_button_rect(thread: u32, message: u32, rect: Option<RECT>) -> Result<(), String> {
    let rect = rect.unwrap_or_default();
    for (control, coordinate) in [
        (SHELL_CONTROL_SEARCH_RECT_LEFT, rect.left),
        (SHELL_CONTROL_SEARCH_RECT_TOP, rect.top),
        (SHELL_CONTROL_SEARCH_RECT_RIGHT, rect.right),
        (SHELL_CONTROL_SEARCH_RECT_BOTTOM, rect.bottom),
    ] {
        unsafe {
            PostThreadMessageW(
                thread,
                message,
                WPARAM(control),
                LPARAM(coordinate as isize),
            )
            .map_err(|error| format!("configure Explorer Search-button rectangle: {error}"))?;
        }
    }
    Ok(())
}

struct SearchButtonLocator {
    taskbar: HWND,
    automation: Option<AutomationSearchButton>,
}

struct AutomationSearchButton {
    taskbar: IUIAutomationElement,
    condition: IUIAutomationCondition,
    cached: Option<IUIAutomationElement>,
}

impl SearchButtonLocator {
    fn new(taskbar: HWND, process_id: u32) -> (Self, Option<RECT>) {
        let automation = unsafe { create_automation_search_button(taskbar, process_id).ok() };
        let mut locator = Self {
            taskbar,
            automation,
        };
        let rect = locator.rect();
        (locator, rect)
    }

    fn rect(&mut self) -> Option<RECT> {
        if let Some(automation) = self.automation.as_mut() {
            let rect = if let Some(search) = automation.cached.as_ref() {
                let rect = unsafe { search.CurrentBoundingRectangle() };
                if let Ok(rect) = rect {
                    if valid_rect(rect) {
                        return Some(rect);
                    }
                }
                // The element went stale (Explorer restarted). Re-resolve it.
                automation.cached = None;
                None
            } else {
                None
            };
            let rect = rect.or_else(|| unsafe {
                automation
                    .taskbar
                    .FindFirst(TreeScope_Descendants, &automation.condition)
                    .ok()
                    .inspect(|search| automation.cached = Some(search.clone()))
                    .and_then(|search| search.CurrentBoundingRectangle().ok())
            });
            if let Some(rect) = rect {
                if valid_rect(rect) {
                    return Some(rect);
                }
            }
        }
        unsafe { child_search_button_rect(self.taskbar) }
    }
}

unsafe fn create_automation_search_button(
    taskbar_window: HWND,
    taskbar_process_id: u32,
) -> Result<AutomationSearchButton, String> {
    let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        .map_err(|error| format!("create UI Automation client: {error}"))?;
    let taskbar = uia
        .ElementFromHandle(taskbar_window)
        .map_err(|error| format!("resolve Explorer taskbar automation root: {error}"))?;
    let id_search_button: VARIANT = "SearchButton".into();
    let cond_search_button = uia
        .CreatePropertyCondition(UIA_AutomationIdPropertyId, &id_search_button)
        .map_err(|error| format!("match SearchButton AutomationId: {error}"))?;
    let id_search_box: VARIANT = "SearchBox".into();
    let cond_search_box = uia
        .CreatePropertyCondition(UIA_AutomationIdPropertyId, &id_search_box)
        .map_err(|error| format!("match SearchBox AutomationId: {error}"))?;
    let id_search_box_button: VARIANT = "SearchBoxButton".into();
    let cond_search_box_button = uia
        .CreatePropertyCondition(UIA_AutomationIdPropertyId, &id_search_box_button)
        .map_err(|error| format!("match SearchBoxButton AutomationId: {error}"))?;
    let id_search: VARIANT = "Search".into();
    let cond_search = uia
        .CreatePropertyCondition(UIA_AutomationIdPropertyId, &id_search)
        .map_err(|error| format!("match Search AutomationId: {error}"))?;
    let id_search_flyout: VARIANT = "SearchButtonFlyout".into();
    let cond_search_flyout = uia
        .CreatePropertyCondition(UIA_AutomationIdPropertyId, &id_search_flyout)
        .map_err(|error| format!("match SearchButtonFlyout AutomationId: {error}"))?;
    let id_taskbar_search: VARIANT = "TaskbarSearchButton".into();
    let cond_taskbar_search = uia
        .CreatePropertyCondition(UIA_AutomationIdPropertyId, &id_taskbar_search)
        .map_err(|error| format!("match TaskbarSearchButton AutomationId: {error}"))?;

    let id_or1 = uia
        .CreateOrCondition(&cond_search_button, &cond_search_box)
        .map_err(|error| format!("combine SearchButton or SearchBox: {error}"))?;
    let id_or2 = uia
        .CreateOrCondition(&id_or1, &cond_search_box_button)
        .map_err(|error| format!("combine Search conditions: {error}"))?;
    let id_or3 = uia
        .CreateOrCondition(&id_or2, &cond_search)
        .map_err(|error| format!("combine Search conditions: {error}"))?;
    let id_or4 = uia
        .CreateOrCondition(&id_or3, &cond_search_flyout)
        .map_err(|error| format!("combine Search conditions: {error}"))?;
    let id_condition = uia
        .CreateOrCondition(&id_or4, &cond_taskbar_search)
        .map_err(|error| format!("combine Search conditions: {error}"))?;

    let process_id: VARIANT = (taskbar_process_id as i32).into();
    let process_condition = uia
        .CreatePropertyCondition(UIA_ProcessIdPropertyId, &process_id)
        .map_err(|error| format!("match Explorer process: {error}"))?;
    let condition = uia
        .CreateAndCondition(&id_condition, &process_condition)
        .map_err(|error| format!("combine SearchButton identity conditions: {error}"))?;
    Ok(AutomationSearchButton {
        taskbar,
        condition,
        cached: None,
    })
}

unsafe fn child_search_button_rect(taskbar: HWND) -> Option<RECT> {
    let mut rect = None;
    let _ = EnumChildWindows(
        Some(taskbar),
        Some(find_search_button_child),
        LPARAM((&mut rect as *mut Option<RECT>) as isize),
    );
    rect
}

unsafe extern "system" fn find_search_button_child(window: HWND, detail: LPARAM) -> BOOL {
    if !IsWindowVisible(window).as_bool() {
        return BOOL(1);
    }
    let mut class_name = [0u16; 64];
    let length = GetClassNameW(window, &mut class_name);
    let class_name = &class_name[..length.max(0) as usize];
    if is_search_button_class(class_name) {
        let mut rect = RECT::default();
        if GetWindowRect(window, &mut rect).is_ok() && valid_rect(rect) {
            *(detail.0 as *mut Option<RECT>) = Some(rect);
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn is_search_button_class(class_name: &[u16]) -> bool {
    ascii_class_eq(class_name, "TraySearch")
        || ascii_class_eq(class_name, "TraySearchBox")
        || ascii_class_eq(class_name, "TraySearchButton")
        || ascii_class_eq(class_name, "SearchButton")
        || ascii_class_eq(class_name, "SearchBox")
        || ascii_class_eq(class_name, "UniversalSearchBand")
        || ascii_class_eq(class_name, "SearchControl")
}

impl Drop for ShellBridge {
    fn drop(&mut self) {
        request_start_icon_shutdown(self.taskbar_thread);
        unsafe {
            let _ = UnhookWindowsHookEx(self.progman_hook);
            if let Some(hook) = self.taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            if let Some(hook) = self.app_manager_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            let _ = UnhookWindowsHookEx(self.taskbar_mouse_hook);
            let _ = FreeLibrary(self.module);
        }
        let _ = std::fs::remove_file(&self.library_path);
        #[cfg(debug_assertions)]
        eprintln!("[win-key] Explorer bridge released");
    }
}

fn request_start_icon_shutdown(thread: u32) {
    let Ok(message) = shell_bridge_message() else {
        return;
    };
    SHELL_ICON_SHUTDOWN_ACK.store(0, Ordering::Release);
    unsafe {
        let _ = PostThreadMessageW(
            thread,
            message,
            WPARAM(SHELL_CONTROL_START_ICON_SHUTDOWN),
            LPARAM(0),
        );
    }
    let _ = wait_for_ack(&SHELL_ICON_SHUTDOWN_ACK, Duration::from_millis(250));
}

unsafe fn cleanup_shell_hook(hook: HHOOK, module: HMODULE, library_path: &PathBuf) {
    let _ = UnhookWindowsHookEx(hook);
    let _ = FreeLibrary(module);
    let _ = std::fs::remove_file(library_path);
}

fn shell_bridge_message() -> Result<u32, String> {
    BRIDGE_MESSAGE_ID
        .get_or_init(|| {
            let name = wide(SHELL_BRIDGE_MESSAGE_NAME);
            let message = unsafe { RegisterWindowMessageW(PCWSTR(name.as_ptr())) };
            if message == 0 {
                Err("register Explorer bridge message failed".to_string())
            } else {
                Ok(message)
            }
        })
        .clone()
}

unsafe fn find_shell_window(class_name: &str) -> Result<HWND, String> {
    if class_name.eq_ignore_ascii_case("Progman") {
        let shell_wnd = GetShellWindow();
        if !shell_wnd.0.is_null() {
            return Ok(shell_wnd);
        }
    }
    let class_name_wide = wide(class_name);
    let mut last_error = None;
    for _ in 0..20 {
        match FindWindowW(PCWSTR(class_name_wide.as_ptr()), PCWSTR::null()) {
            Ok(window) if !window.0.is_null() => return Ok(window),
            Ok(_) => last_error = Some("window handle was null".to_string()),
            Err(error) => last_error = Some(error.to_string()),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "Explorer window '{class_name}' is unavailable: {}",
        last_error.unwrap_or_else(|| "timed out".to_string())
    ))
}

fn wait_for_ack(acknowledgement: &AtomicU32, timeout: Duration) -> u32 {
    let started = Instant::now();
    let mut msg = MSG::default();
    while started.elapsed() < timeout {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
        }
        let acknowledged = acknowledgement.load(Ordering::Acquire);
        if acknowledged != 0 {
            return acknowledged;
        }
        unsafe {
            let _ = MsgWaitForMultipleObjectsEx(None, 25, QS_ALLINPUT, Default::default());
        }
    }
    acknowledgement.load(Ordering::Acquire)
}

fn write_shell_hook_library() -> Result<PathBuf, String> {
    let directory = std::env::temp_dir().join("Prism").join("shell-hooks");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create Explorer bridge directory: {error}"))?;
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_stale_hook =
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("prism-shell-hook-") && name.ends_with(".dll")
                    });
            if is_stale_hook {
                // Loaded DLLs remain locked on Windows, so this removes only
                // debris from already-terminated Prism instances.
                let _ = std::fs::remove_file(path);
            }
        }
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = directory.join(format!(
        "prism-shell-hook-{}-{nonce}.dll",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("create Explorer bridge DLL: {error}"))?;
    file.write_all(include_bytes!(env!("PRISM_SHELL_HOOK_DLL")))
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("write Explorer bridge DLL: {error}"))?;
    Ok(path)
}

const RAW_INPUT_WINDOW_CLASS: &str = "PrismRawKeyboardObserver";

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
                Err("register raw keyboard observer class failed".to_string())
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
    let window = CreateWindowExW(
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
    )
    .map_err(|error| format!("create raw keyboard observer window: {error}"))?;
    let bridge_message = match shell_bridge_message() {
        Ok(message) => message,
        Err(error) => {
            let _ = DestroyWindow(window);
            return Err(error);
        }
    };
    // Explorer normally runs at medium integrity. Allow only Prism's private
    // registered message so its injected bridge can acknowledge an elevated
    // debug or administrator-launched Prism process.
    if let Err(error) = ChangeWindowMessageFilterEx(window, bridge_message, MSGFLT_ALLOW, None) {
        let _ = DestroyWindow(window);
        return Err(format!("allow Explorer bridge acknowledgments: {error}"));
    }
    // INPUTSINK only observes input while Prism is in the background. It does
    // not set NOLEGACY, so normal key messages and third-party tools continue
    // to receive the original keyboard stream.
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
        return Err(format!("register raw keyboard observer: {error}"));
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

fn classify_raw_key(
    virtual_key: u16,
    make_code: u16,
    flags: u16,
    message: u32,
) -> Option<(KeyKind, bool)> {
    let is_up = flags as u32 & RI_KEY_BREAK != 0;
    let message_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
    let message_up = message == WM_KEYUP || message == WM_SYSKEYUP;
    if make_code as u32 == KEYBOARD_OVERRUN_MAKE_CODE
        || virtual_key >= 255
        || !((message_down && !is_up) || (message_up && is_up))
    {
        return None;
    }
    let kind = if virtual_key == VK_LWIN.0 {
        KeyKind::Win(WinSide::Left)
    } else if virtual_key == VK_RWIN.0 {
        KeyKind::Win(WinSide::Right)
    } else {
        KeyKind::Other(virtual_key)
    };
    Some((kind, !is_up))
}

fn cancel_pending_win_toggle() {
    if let Ok(mut pending) = PENDING_WIN_TOGGLE.lock() {
        pending.cancel();
    }
}

fn schedule_win_toggle(side: WinSide, blocked_keys: [bool; 256]) {
    if let Ok(mut pending) = PENDING_WIN_TOGGLE.lock() {
        pending.arm(side, blocked_keys, Instant::now());
    }
}

fn flush_pending_win_toggle() {
    if let Ok(mut pending) = PENDING_WIN_TOGGLE.lock() {
        let side = pending.take_if_ready(Instant::now());
        if let Some(side) = side {
            // Keep the pending lock through enqueueing. Disable/reset paths
            // take this same lock after clearing ACTIVE, so a candidate either
            // queues before their final drain or observes inactive.
            if ACTIVE.load(Ordering::Acquire)
                && RAW_OBSERVER_ACTIVE.load(Ordering::Acquire)
                && SHELL_BRIDGE_ACTIVE.load(Ordering::Acquire)
            {
                queue_action(Action::ToggleWin(side));
            }
        }
    }
}

fn pending_win_toggle_wait() -> Option<Duration> {
    PENDING_WIN_TOGGLE
        .lock()
        .ok()
        .and_then(|pending| pending.wait_duration(Instant::now()))
}

fn non_win_keys_down() -> [bool; 256] {
    let mut down = [false; 256];
    for key in 0x08u16..=0xfe {
        if key != VK_LWIN.0 && key != VK_RWIN.0 {
            let is_down = unsafe { GetAsyncKeyState(i32::from(key)) } as u16 & 0x8000 != 0;
            down[canonical_non_win_key(key) as usize] |= is_down;
        }
    }
    down
}

fn canonical_non_win_key(key: u16) -> u16 {
    match key {
        key if key == VK_LSHIFT.0 || key == VK_RSHIFT.0 => VK_SHIFT.0,
        key if key == VK_LCONTROL.0 || key == VK_RCONTROL.0 => VK_CONTROL.0,
        key if key == VK_LMENU.0 || key == VK_RMENU.0 => VK_MENU.0,
        _ => key,
    }
}

fn observe_pending_win_key(key: u16, is_down: bool) {
    if let Ok(mut pending) = PENDING_WIN_TOGGLE.lock() {
        pending.observe_key(key, is_down, Instant::now());
    }
}

fn should_defer_toggle(kind: KeyKind) -> bool {
    matches!(kind, KeyKind::Win(_))
}

unsafe extern "system" fn raw_input_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if shell_bridge_message().is_ok_and(|bridge_message| message == bridge_message) {
        match wparam.0 {
            SHELL_EVENT_HOTKEY_DISABLED => {
                SHELL_BRIDGE_ACK.store(if lparam.0 != 0 { 2 } else { 1 }, Ordering::Release);
            }
            SHELL_EVENT_START_RECT_CONFIGURED => {
                SHELL_START_RECT_ACK.store(if lparam.0 != 0 { 2 } else { 1 }, Ordering::Release);
            }
            SHELL_EVENT_SEARCH_RECT_CONFIGURED => {
                SHELL_SEARCH_RECT_ACK.store(if lparam.0 != 0 { 2 } else { 1 }, Ordering::Release);
            }
            SHELL_EVENT_START_ICON_SHUTDOWN => {
                SHELL_ICON_SHUTDOWN_ACK.store(if lparam.0 != 0 { 2 } else { 1 }, Ordering::Release);
            }
            SHELL_EVENT_START_ICON_REFRESHED => {
                debug_trace(&format!("start-icon-refresh {}", lparam.0));
            }
            SHELL_EVENT_TASKBAR_PIN_COMPLETED => {
                SHELL_TASKBAR_PIN_ACK.store(if lparam.0 != 0 { 2 } else { 1 }, Ordering::Release);
            }
            SHELL_EVENT_TASKBAR_START_CLICK_X => {
                SHELL_START_CLICK_X.store(lparam.0 as i32, Ordering::Release);
            }
            SHELL_EVENT_TASKBAR_START_CLICK_Y
                if ACTIVE.load(Ordering::Acquire)
                    && RAW_OBSERVER_ACTIVE.load(Ordering::Acquire)
                    && SHELL_BRIDGE_ACTIVE.load(Ordering::Acquire) =>
            {
                queue_action(Action::ToggleTaskbar(POINT {
                    x: SHELL_START_CLICK_X.load(Ordering::Acquire),
                    y: lparam.0 as i32,
                }));
            }
            _ => {}
        }
        return LRESULT(0);
    }
    if message == WM_INPUT
        && ACTIVE.load(Ordering::Acquire)
        && RAW_OBSERVER_ACTIVE.load(Ordering::Acquire)
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
            if let Some((kind, is_down)) = classify_raw_key(
                keyboard.VKey,
                keyboard.MakeCode,
                keyboard.Flags,
                keyboard.Message,
            ) {
                if let KeyKind::Other(key) = kind {
                    observe_pending_win_key(key, is_down);
                }
                let decision = RAW_MACHINE
                    .lock()
                    .map(|mut machine| machine.feed(kind, is_down))
                    .unwrap_or(Decision::Pass);
                match decision {
                    Decision::Toggle(side) if should_defer_toggle(kind) => {
                        schedule_win_toggle(side, non_win_keys_down());
                    }
                    Decision::Toggle(side) => {
                        queue_action(Action::ToggleWin(side));
                    }
                    Decision::Mask | Decision::Pass => {}
                }
            }
        }
    }
    DefWindowProcW(window, message, wparam, lparam)
}

fn toggle_clock_ms() -> u64 {
    TOGGLE_CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

#[allow(dead_code)]
fn point_from_message(detail: isize) -> POINT {
    let packed = detail as u32;
    POINT {
        x: (packed as u16 as i16) as i32,
        y: ((packed >> 16) as u16 as i16) as i32,
    }
}

fn queue_action(action: Action) {
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

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
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
    fn start_button_child_fallback_uses_class_not_caption() {
        assert!(is_start_button_class(&wide("Start")[..5]));
        assert!(is_start_button_class(&wide("startbutton")[..11]));
        assert!(!is_start_button_class(&wide("Button")[..6]));
        assert!(!is_start_button_class(&wide("SearchButton")[..12]));
    }

    #[test]
    fn search_button_child_fallback_uses_class_not_caption() {
        assert!(is_search_button_class(&wide("TraySearch")[..10]));
        assert!(is_search_button_class(&wide("traysearchbox")[..13]));
        assert!(is_search_button_class(&wide("TraySearchButton")[..16]));
        assert!(is_search_button_class(&wide("SearchButton")[..12]));
        assert!(is_search_button_class(&wide("SearchBox")[..9]));
        assert!(is_search_button_class(&wide("UniversalSearchBand")[..19]));
        assert!(is_search_button_class(&wide("SearchControl")[..13]));
        assert!(!is_search_button_class(&wide("Button")[..6]));
        assert!(!is_search_button_class(&wide("Start")[..5]));
        assert!(!is_search_button_class(&wide("StartButton")[..11]));
    }

    #[test]
    fn shell_message_point_preserves_signed_coordinates() {
        let packed = ((-240i16 as u16 as u32) << 16) | (-1_920i16 as u16 as u32);
        let point = point_from_message(packed as isize);
        assert_eq!(point.x, -1_920);
        assert_eq!(point.y, -240);
    }

    #[test]
    fn raw_keyboard_packets_preserve_valid_transitions_only() {
        assert_eq!(
            classify_raw_key(VK_LWIN.0, 0x5b, 0, WM_KEYDOWN),
            Some((KeyKind::Win(WinSide::Left), true))
        );
        assert_eq!(
            classify_raw_key(VK_RWIN.0, 0x5c, RI_KEY_BREAK as u16, WM_KEYUP),
            Some((KeyKind::Win(WinSide::Right), false))
        );
        assert_eq!(
            classify_raw_key(0x41, 0x1e, 0, WM_KEYDOWN),
            Some((KeyKind::Other(0x41), true))
        );
        assert_eq!(
            classify_raw_key(VK_LWIN.0, KEYBOARD_OVERRUN_MAKE_CODE as u16, 0, WM_KEYDOWN),
            None
        );
        assert_eq!(
            classify_raw_key(VK_LWIN.0, 0x5b, RI_KEY_BREAK as u16, WM_KEYDOWN),
            None
        );
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
    fn held_win_repeats_pass_through_and_toggle_once() {
        assert_eq!(
            run(&[
                (win(WinSide::Left), true),
                (win(WinSide::Left), true), // auto-repeat
                (win(WinSide::Left), false),
            ]),
            vec![
                Decision::Mask,
                Decision::Pass,
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
    fn win_key_combos_pass_every_non_win_event_through() {
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
                    Decision::Pass,
                    Decision::Pass,
                    Decision::Pass,
                    Decision::Pass,
                ],
                "key {key:#x}"
            );
        }
    }

    #[test]
    fn win_r_combo_never_toggles() {
        assert_eq!(
            run(&[
                (win(WinSide::Left), true),
                (KeyKind::Other(0x52), true), // R
                (KeyKind::Other(0x52), false),
                (win(WinSide::Left), false),
            ]),
            vec![
                Decision::Mask,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn win_r_release_only_combo_never_toggles() {
        assert_eq!(
            run(&[
                (win(WinSide::Left), true),
                (KeyKind::Other(0x52), false), // R-up without a matching raw R-down
                (win(WinSide::Left), false),
            ]),
            vec![Decision::Mask, Decision::Pass, Decision::Pass]
        );
    }

    #[test]
    fn queued_r_release_cancels_a_deferred_win_toggle() {
        let mut machine = WinKeyMachine::default();
        let mut pending = PendingWinToggle::EMPTY;
        assert_eq!(machine.feed(win(WinSide::Left), true), Decision::Mask);
        let decision = machine.feed(win(WinSide::Left), false);
        let Decision::Toggle(side) = decision else {
            panic!("Win-up should create a deferred toggle candidate");
        };
        let now = Instant::now();
        pending.arm(side, [false; 256], now);

        assert_eq!(machine.feed(KeyKind::Other(0x52), false), Decision::Pass);
        pending.observe_key(0x52, false, now);

        assert_eq!(pending.take_if_ready(now + WIN_TOGGLE_RELEASE_GRACE), None);
    }

    #[test]
    fn held_r_snapshot_waits_for_its_trailing_release() {
        let mut blocked = [false; 256];
        blocked[0x52] = true;
        let mut pending = PendingWinToggle::EMPTY;
        let now = Instant::now();
        pending.arm(WinSide::Left, blocked, now);

        assert_eq!(pending.take_if_ready(now + WIN_TOGGLE_RELEASE_GRACE), None);
        pending.observe_key(0x52, false, now + WIN_TOGGLE_RELEASE_GRACE);
        assert_eq!(pending.take_if_ready(now + WIN_TOGGLE_RELEASE_GRACE), None);
    }

    #[test]
    fn typing_after_bare_win_does_not_cancel_the_deferred_toggle() {
        let mut pending = PendingWinToggle::EMPTY;
        let now = Instant::now();
        pending.arm(WinSide::Left, [false; 256], now);

        pending.observe_key(0x41, true, now);
        pending.observe_key(0x41, false, now);
        assert_eq!(
            pending.take_if_ready(now + WIN_TOGGLE_RELEASE_GRACE),
            Some(WinSide::Left)
        );
    }

    #[test]
    fn post_release_keydown_clears_a_stale_physical_snapshot() {
        let mut blocked = [false; 256];
        blocked[0x41] = true;
        let mut pending = PendingWinToggle::EMPTY;
        let now = Instant::now();
        pending.arm(WinSide::Left, blocked, now);

        assert_eq!(pending.take_if_ready(now + WIN_TOGGLE_RELEASE_GRACE), None);
        let retry_at = now + WIN_TOGGLE_RELEASE_GRACE;
        pending.observe_key(0x41, true, retry_at);
        assert_eq!(
            pending.take_if_ready(retry_at + WIN_TOGGLE_RELEASE_GRACE),
            Some(WinSide::Left)
        );
    }

    #[test]
    fn modifier_aliases_share_one_pending_key_slot() {
        for (specific, generic) in [
            (VK_LSHIFT.0, VK_SHIFT.0),
            (VK_RSHIFT.0, VK_SHIFT.0),
            (VK_LCONTROL.0, VK_CONTROL.0),
            (VK_RCONTROL.0, VK_CONTROL.0),
            (VK_LMENU.0, VK_MENU.0),
            (VK_RMENU.0, VK_MENU.0),
        ] {
            assert_eq!(canonical_non_win_key(specific), generic);
        }

        let now = Instant::now();
        let mut blocked = [false; 256];
        blocked[VK_SHIFT.0 as usize] = true;
        let mut pending = PendingWinToggle::EMPTY;
        pending.arm(WinSide::Left, blocked, now);
        pending.observe_key(VK_LSHIFT.0, true, now);
        pending.observe_key(VK_SHIFT.0, false, now);
        assert_eq!(
            pending.take_if_ready(now + WIN_TOGGLE_RELEASE_GRACE),
            Some(WinSide::Left)
        );
    }

    #[test]
    fn ctrl_esc_toggles_immediately_without_win_deferral() {
        assert!(!should_defer_toggle(KeyKind::Other(VK_ESCAPE_CODE)));
        assert!(should_defer_toggle(win(WinSide::Left)));
    }

    #[test]
    fn modifiers_while_win_held_pass_and_prevent_a_toggle() {
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
    fn shift_then_win_then_key_all_pass_through() {
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
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn preheld_keys_prevent_masking_and_standalone_toggle() {
        for key in [0x10, 0x11, 0x12, 0x41] {
            assert_eq!(
                run(&[
                    (KeyKind::Other(key), true),
                    (win(WinSide::Left), true),
                    (win(WinSide::Left), false),
                    (KeyKind::Other(key), false),
                ]),
                vec![
                    Decision::Pass,
                    Decision::Pass,
                    Decision::Pass,
                    Decision::Pass,
                ],
                "pre-held key {key:#x}"
            );
        }
    }

    #[test]
    fn both_win_keys_and_chord_releases_pass_through() {
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
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn both_win_keys_without_another_key_never_toggle() {
        assert_eq!(
            run(&[
                (win(WinSide::Left), true),
                (win(WinSide::Right), true),
                (win(WinSide::Right), false),
                (win(WinSide::Left), false),
            ]),
            vec![
                Decision::Mask,
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
        assert_eq!(m.feed(KeyKind::Other(0x45), true), Decision::Pass);
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
                Decision::Pass,
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

    #[test]
    fn standalone_ctrl_esc_left_ctrl_toggles() {
        assert_eq!(
            run(&[
                (KeyKind::Other(VK_CONTROL_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), false),
                (KeyKind::Other(VK_CONTROL_CODE), false),
            ]),
            vec![
                Decision::Pass,
                Decision::Mask,
                Decision::Toggle(WinSide::Left),
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn standalone_ctrl_esc_right_ctrl_toggles() {
        assert_eq!(
            run(&[
                (KeyKind::Other(VK_RCONTROL_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), false),
                (KeyKind::Other(VK_RCONTROL_CODE), false),
            ]),
            vec![
                Decision::Pass,
                Decision::Mask,
                Decision::Toggle(WinSide::Left),
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn held_ctrl_esc_repeats_pass_through_and_toggle_once() {
        assert_eq!(
            run(&[
                (KeyKind::Other(VK_CONTROL_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), true), // auto-repeat
                (KeyKind::Other(VK_ESCAPE_CODE), false),
                (KeyKind::Other(VK_CONTROL_CODE), false),
            ]),
            vec![
                Decision::Pass,
                Decision::Mask,
                Decision::Pass,
                Decision::Toggle(WinSide::Left),
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn rapid_ctrl_esc_taps_toggle_every_time() {
        let mut m = WinKeyMachine::default();
        assert_eq!(
            m.feed(KeyKind::Other(VK_CONTROL_CODE), true),
            Decision::Pass
        );
        for _ in 0..3 {
            assert_eq!(m.feed(KeyKind::Other(VK_ESCAPE_CODE), true), Decision::Mask);
            assert_eq!(
                m.feed(KeyKind::Other(VK_ESCAPE_CODE), false),
                Decision::Toggle(WinSide::Left)
            );
        }
        assert_eq!(
            m.feed(KeyKind::Other(VK_CONTROL_CODE), false),
            Decision::Pass
        );
        assert!(!m.left.down && !m.right.down && !m.ctrl_esc.down);
    }

    #[test]
    fn ctrl_shift_esc_passes_through_without_toggle() {
        assert_eq!(
            run(&[
                (KeyKind::Other(VK_CONTROL_CODE), true),
                (KeyKind::Other(0x10), true), // Shift
                (KeyKind::Other(VK_ESCAPE_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), false),
                (KeyKind::Other(0x10), false),
                (KeyKind::Other(VK_CONTROL_CODE), false),
            ]),
            vec![
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn ctrl_esc_combo_with_other_key_passes_through() {
        assert_eq!(
            run(&[
                (KeyKind::Other(VK_CONTROL_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), true),
                (KeyKind::Other(0x09), true), // Tab
                (KeyKind::Other(0x09), false),
                (KeyKind::Other(VK_ESCAPE_CODE), false),
                (KeyKind::Other(VK_CONTROL_CODE), false),
            ]),
            vec![
                Decision::Pass,
                Decision::Mask,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn ctrl_released_before_esc_still_toggles() {
        assert_eq!(
            run(&[
                (KeyKind::Other(VK_CONTROL_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), true),
                (KeyKind::Other(VK_CONTROL_CODE), false),
                (KeyKind::Other(VK_ESCAPE_CODE), false),
            ]),
            vec![
                Decision::Pass,
                Decision::Mask,
                Decision::Pass,
                Decision::Toggle(WinSide::Left),
            ]
        );
    }

    #[test]
    fn esc_without_ctrl_never_toggles() {
        assert_eq!(
            run(&[
                (KeyKind::Other(VK_ESCAPE_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), false),
            ]),
            vec![Decision::Pass, Decision::Pass,]
        );
    }

    #[test]
    fn esc_then_ctrl_never_toggles() {
        assert_eq!(
            run(&[
                (KeyKind::Other(VK_ESCAPE_CODE), true),
                (KeyKind::Other(VK_CONTROL_CODE), true),
                (KeyKind::Other(VK_ESCAPE_CODE), false),
                (KeyKind::Other(VK_CONTROL_CODE), false),
            ]),
            vec![
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
                Decision::Pass,
            ]
        );
    }

    #[test]
    fn ctrl_esc_reset_mid_press_never_toggles() {
        let mut m = WinKeyMachine::default();
        assert_eq!(
            m.feed(KeyKind::Other(VK_CONTROL_CODE), true),
            Decision::Pass
        );
        assert_eq!(m.feed(KeyKind::Other(VK_ESCAPE_CODE), true), Decision::Mask);
        m.reset();
        assert!(!m.left.down && !m.right.down && !m.ctrl_esc.down);
        assert_eq!(
            m.feed(KeyKind::Other(VK_ESCAPE_CODE), false),
            Decision::Pass
        );
        assert_eq!(
            m.feed(KeyKind::Other(VK_CONTROL_CODE), false),
            Decision::Pass
        );
    }

    fn m_feed_single(kind: KeyKind, is_down: bool) -> Decision {
        let mut m = WinKeyMachine::default();
        m.feed(kind, is_down)
    }
}
