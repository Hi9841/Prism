//! Win-key observation (disabled by default).
//!
//! Turns a standalone Win-key press into a Prism toggle while leaving
//! Win+* combos and other global keyboard tools intact. Design rules:
//!
//! - Normal mode works as Open-Shell does: Explorer's bare-Win and Ctrl+Esc
//!   hotkeys are released, so Windows itself decides what a standalone Win tap
//!   is and sends `SC_TASKLIST` to the shell. The Explorer hook forwards that
//!   command to Prism. Prism reads no keyboard input in this mode.
//! - StartAllBack integration disables the provider's Win action reversibly.
//!   The provider then consumes Win before Explorer sees it, so only in this
//!   mode a raw-input observer and the side-aware state machine
//!   (`WinKeyMachine`, unit-tested without Win32) detect standalone Win.
//!   Prism never installs a low-level keyboard hook or modifies physical input.
//! - A small Explorer message hook takes ownership of `SC_TASKLIST` before
//!   native Start is launched, and of Explorer `WM_HOTKEY` Win+S / Win+Q
//!   before native Search (opened via `activate_palette`). Without provider
//!   integration, it also releases Explorer's bare-Win hotkey. Search hotkeys
//!   fail open if the observer cannot be notified.
//! - The Start button is found by UI Automation ID (with a child-window class
//!   fallback), and an Explorer-thread mouse hook consumes clicks in its rect.
//!   The bridge client (install, requests, button lookup) is `shell_bridge`.
//! - Every Win32 result is checked. Registration failures disable observation.
//! - Disabling, quitting or failing mid-keypress resets all observation state.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Devices::HumanInterfaceDevice::{
    HID_USAGE_GENERIC_KEYBOARD, HID_USAGE_PAGE_GENERIC, KEYBOARD_OVERRUN_MAKE_CODE,
};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, GetCurrentThreadId, SetWaitableTimer, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
    TIMER_ALL_ACCESS,
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
    GetQueueStatus, MsgWaitForMultipleObjectsEx, PeekMessageW,
    PostThreadMessageW, RegisterClassW, RegisterWindowMessageW,
    TranslateMessage, MSG, MSGFLT_ALLOW, PM_REMOVE, QS_ALLINPUT, QS_RAWINPUT,
    RI_KEY_BREAK, WM_APP, WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::win32::wide;
use crate::shell_bridge::{self, BridgeEvent, ShellBridge};
use crate::shell_protocol::OBSERVER_CLASS_NAME;

const ACTION_MESSAGE: u32 = shell_bridge::WAKE_MESSAGE;
const TOGGLE_DEBOUNCE_MS: u64 = 80;
const WIN_TOGGLE_RELEASE_GRACE: Duration = Duration::from_millis(30);
/// A chord key released just before the Win-key snapshot already has its key-up queued as raw
/// input. With no raw input queued, a plain Win tap toggles after this short margin instead of
/// the full release grace (Win key to palette: about -27 ms, session H).
const WIN_TOGGLE_QUIET_MIN: Duration = Duration::from_millis(3);
/// A shell Start command toggles immediately. The normal case is already
/// de-duplicated: a raw Win-down sets `LAST_RAW_TOGGLE_MS` ~100 ms before the
/// Win-up shell command, and a late raw toggle is dropped by the debounce.
const SHELL_START_FALLBACK_GRACE: Duration = Duration::ZERO;
/// If the raw observer saw a Win/Ctrl+Esc this recently, a shell Start command
/// belongs to the same press and is ignored. Kept just above the physical
/// press duration so a later, separate press is never suppressed.
const SHELL_START_DEDUPE_MS: u64 = 250;
/// If a Search hotkey races ahead of raw Win-down, treat the coming Win press
/// as a combo only while Win is still physically held and only for this long.
const SEARCH_COMBO_GRACE: Duration = Duration::from_millis(250);


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
    search_combo_until: Option<Instant>,
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
        search_combo_until: None,
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
                let already_down = match side {
                    WinSide::Left => self.left.down,
                    WinSide::Right => self.right.down,
                };
                let search_combo = if is_down && !already_down {
                    self.take_search_combo(Instant::now())
                } else {
                    false
                };
                let press = self.press_mut(side);
                if is_down {
                    if press.down {
                        // Auto-repeat or extra press while already held.
                        Decision::Pass
                    } else {
                        press.down = true;
                        press.combo = preexisting_chord || search_combo;
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
                    if !other_win_down {
                        self.search_combo_until = None;
                    }
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
        self.search_combo_until = None;
    }

    /// Forgets held keys that are no longer physically down. Raw input loses a
    /// release that happens on another desktop (the lock screen after Win+L,
    /// UAC, Ctrl+Alt+Del); a key left "held" made every later Win press a
    /// chord until that key was typed again. Call it before each key-down: a
    /// key released after the press began still reports its release next,
    /// and any non-Win event while Win is held marks the press as a chord.
    pub fn forget_released(&mut self, physically_down: impl Fn(u16) -> bool) {
        for (key, down) in self.non_win_down.iter_mut().enumerate() {
            if *down && !physically_down(key as u16) {
                *down = false;
            }
        }
        if self.left.down && !physically_down(VK_LWIN.0) {
            self.left = Press::default();
        }
        if self.right.down && !physically_down(VK_RWIN.0) {
            self.right = Press::default();
        }
        if self.ctrl_esc.down && !physically_down(VK_ESCAPE_CODE) {
            self.ctrl_esc = Press::default();
        }
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

    /// Explorer forwarded a Win+S / Win+Q hotkey. Marks a currently held Win
    /// press as a combo so Win-up does not toggle, and arms a short grace only
    /// when Win is still physically down but raw input has not seen it yet.
    pub fn claim_search_hotkey(&mut self, now: Instant, win_physically_down: bool) -> WinSide {
        self.left.combo |= self.left.down;
        self.right.combo |= self.right.down;
        if self.left.down || self.right.down {
            self.search_combo_until = None;
        } else if win_physically_down {
            self.search_combo_until = Some(now + SEARCH_COMBO_GRACE);
        } else {
            self.search_combo_until = None;
        }
        if self.right.down && !self.left.down {
            WinSide::Right
        } else {
            WinSide::Left
        }
    }

    fn take_search_combo(&mut self, now: Instant) -> bool {
        matches!(self.search_combo_until.take(), Some(until) if now < until)
    }
}

/* ------------------------------------------------------------------ */
/*  Hook plumbing                                                       */
/* ------------------------------------------------------------------ */

static ACTIVE: AtomicBool = AtomicBool::new(false);
static PROVIDER_SUPPRESSES_START: AtomicBool = AtomicBool::new(false);
static OBSERVER_ACTIVE: AtomicBool = AtomicBool::new(false);
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
type HookReady = mpsc::SyncSender<Result<(), String>>;
type StopReady = mpsc::SyncSender<()>;

static START_TX: OnceLock<mpsc::Sender<HookReady>> = OnceLock::new();
static RAW_MACHINE: Mutex<WinKeyMachine> = Mutex::new(WinKeyMachine::EMPTY);
static LAST_RAW_TOGGLE_MS: AtomicU64 = AtomicU64::new(0);
static SHELL_START_FALLBACK: Mutex<ShellStartFallback> = Mutex::new(ShellStartFallback::EMPTY);
static LAST_TOGGLE_MS: AtomicU64 = AtomicU64::new(0);
static TOGGLE_CLOCK: OnceLock<Instant> = OnceLock::new();
static PENDING_WIN_TOGGLE: Mutex<PendingWinToggle> = Mutex::new(PendingWinToggle::EMPTY);

enum Action {
    ToggleWin(WinSide),
    ToggleTaskbar(POINT),
    ActivateSearch,
}

struct PendingWinToggle {
    side: Option<WinSide>,
    blocked_keys: [bool; 256],
    post_release_keys: [bool; 256],
    deadline: Option<Instant>,
    /// Earliest toggle when no raw input is queued (`WIN_TOGGLE_QUIET_MIN` after the arm).
    quiet_at: Option<Instant>,
}

impl PendingWinToggle {
    const EMPTY: Self = Self {
        side: None,
        blocked_keys: [false; 256],
        post_release_keys: [false; 256],
        deadline: None,
        quiet_at: None,
    };

    fn arm(&mut self, side: WinSide, blocked_keys: [bool; 256], now: Instant) {
        self.side = Some(side);
        self.blocked_keys = blocked_keys;
        self.post_release_keys.fill(false);
        self.deadline = Some(now + WIN_TOGGLE_RELEASE_GRACE);
        self.quiet_at = Some(now + WIN_TOGGLE_QUIET_MIN);
    }

    fn cancel(&mut self) {
        self.side = None;
        self.blocked_keys.fill(false);
        self.post_release_keys.fill(false);
        self.deadline = None;
        self.quiet_at = None;
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
            // New typing after Win-up: wait for its key-up order, never the quiet shortcut.
            self.quiet_at = None;
            return;
        }
        if self.post_release_keys[key] {
            self.post_release_keys[key] = false;
            return;
        }
        // A key-up without a post-release key-down belongs to the Win chord.
        self.cancel();
    }

    /// `input_queued`: raw input waits in this thread's queue (it may hold a chord key-up).
    fn take_if_ready(&mut self, now: Instant, input_queued: bool) -> Option<WinSide> {
        let deadline = self.deadline?;
        let quiet = !input_queued && self.quiet_at.is_some_and(|at| now >= at);
        if now < deadline && !quiet {
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
        self.quiet_at = None;
        side
    }

    fn wait_duration(&self, now: Instant) -> Option<Duration> {
        let next = match (self.deadline, self.quiet_at) {
            (Some(deadline), Some(quiet)) => Some(deadline.min(quiet)),
            (deadline, _) => deadline,
        };
        self.side.and(next).map(|at| at.saturating_duration_since(now))
    }
}

static ACTION_TX: OnceLock<mpsc::Sender<Action>> = OnceLock::new();
static ACTION_RX: Mutex<Option<mpsc::Receiver<Action>>> = Mutex::new(None);
static STOP_READY: Mutex<Option<StopReady>> = Mutex::new(None);
static RAW_INPUT_CLASS: OnceLock<Result<(), String>> = OnceLock::new();

pub fn set_provider_suppression(active: bool) {
    PROVIDER_SUPPRESSES_START.store(active, Ordering::Release);
    RAW_MACHINE.lock().map(|mut machine| machine.reset()).ok();
    cancel_pending_win_toggle();
    clear_queued_actions();
}

/// Turns Win-key observation on or off. Disabling resets the machine and
/// tears down native observers immediately.
pub fn set_enabled(on: bool) -> Result<(), String> {
    if on == ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }
    ACTIVE.store(on, Ordering::SeqCst);
    if !on {
        shell_bridge::deactivate();
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
    let thread = unsafe { GetCurrentThreadId() };
    THREAD_ID.store(thread, Ordering::SeqCst);
    shell_bridge::attach_pump(thread);
    while let Ok(ready) = rx.recv() {
        if ACTIVE.load(Ordering::SeqCst) {
            unsafe { run_pump(ready) };
        } else {
            let _ = ready.send(Err("Windows-key observation was cancelled".to_string()));
        }
    }
    shell_bridge::attach_pump(0);
    THREAD_ID.store(0, Ordering::SeqCst);
}

/// Creates the observer window (raw keyboard input only in provider mode) and
/// installs the Explorer Start-command bridge.
unsafe fn run_pump(ready: HookReady) {
    let provider_mode = PROVIDER_SUPPRESSES_START.load(Ordering::Acquire);
    let _ = taskbar_created_message();
    let observer_window = match create_observer_window(provider_mode) {
        Ok(window) => window,
        Err(error) => {
            let detail = error.clone();
            let _ = ready.send(Err(error));
            disable_observation("Win-key observer registration failed", &detail);
            return;
        }
    };
    OBSERVER_ACTIVE.store(true, Ordering::Release);
    let com_initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
    let mut bridge = match ShellBridge::install(!provider_mode) {
        Ok(bridge) => bridge,
        Err(error) => {
            crate::logs::debug_trace(&format!("bridge-install-error {error}"));
            OBSERVER_ACTIVE.store(false, Ordering::Release);
            destroy_observer_window(observer_window);
            let detail = error.clone();
            let _ = ready.send(Err(error));
            disable_observation("Explorer Start-command bridge installation failed", &detail);
            if com_initialized {
                CoUninitialize();
            }
            return;
        }
    };
    shell_bridge::connect(&bridge);
    shell_bridge::notify_start_icon_changed();
    let _ = ready.send(Ok(()));
    crate::logs::debug_trace("bridge-install-ok");
    /// Bridge reinstall retries: the first after half a second, then doubling.
    const REINSTALL_FIRST_DELAY: Duration = Duration::from_millis(500);
    const REINSTALL_MAX_DELAY: Duration = Duration::from_secs(30);
    let mut reinstall_delay = REINSTALL_FIRST_DELAY;
    let mut reinstall_at: Option<Instant> = None;
    // A wait timeout ends on the system clock tick (15.6 ms by default), and the pending Win
    // toggle is due 3 ms after Win-up: short waits use a high-resolution timer instead.
    let precise_timer = CreateWaitableTimerExW(
        None,
        PCWSTR::null(),
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
        TIMER_ALL_ACCESS.0,
    )
    .ok();
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
        flush_shell_start_fallback();
        // Drain native actions outside the callback.
        if let Ok(mut rx_slot) = ACTION_RX.lock() {
            if let Some(rx) = rx_slot.as_mut() {
                while let Ok(action) = rx.try_recv() {
                    if !ACTIVE.load(Ordering::Acquire) {
                        break 'pump;
                    }
                    match action {
                        Action::ToggleWin(_side) => {
                            crate::logs::debug_trace("action-toggle-win");
                            if let Some(host) = crate::host::get() {
                                host.toggle_from_win(bridge.start_rect());
                            }
                        }
                        Action::ToggleTaskbar(click) => {
                            if let Some(host) = crate::host::get() {
                                host.toggle_from_taskbar(click, bridge.start_rect());
                            }
                        }
                        Action::ActivateSearch => {
                            crate::logs::debug_trace("action-activate-search");
                            if let Some(host) = crate::host::get() {
                                host.activate();
                            }
                        }
                    }
                }
            }
        }
        if reinstall_at.is_none_or(|at| Instant::now() >= at) && TASKBAR_RESTARTED.swap(false, Ordering::AcqRel) {
            // A new Explorer: re-inject the hooks and release its Start hotkeys
            // again. Release the old bridge first: when the same Explorer
            // still runs, its own hook must handle its icon shutdown before a
            // newer hook chains in front of it.
            bridge.release();
            let provider_mode = PROVIDER_SUPPRESSES_START.load(Ordering::Acquire);
            match ShellBridge::install(!provider_mode) {
                Ok(new_bridge) => {
                    bridge = new_bridge;
                    reinstall_delay = REINSTALL_FIRST_DELAY;
                    reinstall_at = None;
                    shell_bridge::connect(&bridge);
                    shell_bridge::notify_start_icon_changed();
                    crate::logs::debug_trace("bridge-reinstalled after taskbar restart");
                }
                Err(error) => {
                    // Explorer may still be starting, or may be gone for good
                    // (a shell replacement): retry, less and less often.
                    crate::logs::debug_trace(&format!("bridge-reinstall-error {error}"));
                    TASKBAR_RESTARTED.store(true, Ordering::Release);
                    reinstall_at = Some(Instant::now() + reinstall_delay);
                    reinstall_delay = (reinstall_delay * 2).min(REINSTALL_MAX_DELAY);
                }
            }
        }
        bridge.refresh_start_rect();
        // The pump has no timer work faster than the Start-rect refresh
        // interval; input already wakes the loop through the message queue.
        // A long timeout removes a permanent 1 Hz wakeup from the hot path.
        let wait = pending_win_toggle_wait()
            .into_iter()
            .chain(shell_start_fallback_wait())
            .chain(reinstall_at.map(|at| at.saturating_duration_since(Instant::now())))
            .min()
            .unwrap_or(shell_bridge::START_RECT_REFRESH_INTERVAL)
            .min(shell_bridge::START_RECT_REFRESH_INTERVAL);
        let timer_armed = precise_timer.filter(|_| wait < Duration::from_secs(1)).is_some_and(|timer| {
            // A negative due time is relative, in 100 ns units.
            let due = -((wait.as_nanos() / 100).max(1) as i64);
            SetWaitableTimer(timer, &due, 0, None, None, false).is_ok()
        });
        let timer_handles: [HANDLE; 1] = [precise_timer.unwrap_or_default()];
        let _ = MsgWaitForMultipleObjectsEx(
            timer_armed.then_some(&timer_handles[..]),
            wait.as_micros().div_ceil(1000).min(u128::from(u32::MAX)) as u32,
            QS_ALLINPUT,
            Default::default(),
        );
    }
    if let Some(timer) = precise_timer {
        let _ = CloseHandle(timer);
    }
    shell_bridge::disconnect();
    OBSERVER_ACTIVE.store(false, Ordering::Release);
    RAW_MACHINE.lock().map(|mut machine| machine.reset()).ok();
    cancel_pending_win_toggle();
    // Ask the Explorer-thread renderer to tear down its window while the hook
    // and observer are still alive, then release the bridge.
    drop(bridge);
    destroy_observer_window(observer_window);
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
/// `detail` is the underlying error; release builds have no `debug_trace`, so it goes to the
/// always-on `%LOCALAPPDATA%\Prism\logs\win-key.log` too (this runs on the hook thread).
fn disable_observation(reason: &str, detail: &str) {
    crate::logs::debug_trace(&format!("observation-disabled {reason}: {detail}"));
    // Startup retries the install every 250 ms for up to 2 minutes: log each cause once.
    static LAST_LOGGED: Mutex<String> = Mutex::new(String::new());
    let key = format!("{reason}: {detail}");
    let fresh = LAST_LOGGED.lock().map(|mut last| std::mem::replace(&mut *last, key.clone()) != key).unwrap_or(true);
    if let Some(dir) = crate::logs::dir().filter(|_| fresh) {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Disabled<'a> {
            unix_ms: u128,
            version: &'a str,
            reason: &'a str,
            detail: &'a str,
        }
        let record = Disabled {
            unix_ms: crate::logs::unix_ms(),
            version: env!("CARGO_PKG_VERSION"),
            reason,
            detail,
        };
        crate::logs::append(&dir, "win-key", &record);
    }
    ACTIVE.store(false, Ordering::SeqCst);
    shell_bridge::deactivate();
    RAW_MACHINE.lock().map(|mut m| m.reset()).ok();
    cancel_pending_win_toggle();
    clear_queued_actions();
    LAST_TOGGLE_MS.store(0, Ordering::Release);
    crate::host::emit(crate::host::CoreEvent::WinModeFailed(reason.to_string()));
}

fn clear_queued_actions() {
    if let Ok(mut rx_slot) = ACTION_RX.lock() {
        if let Some(rx) = rx_slot.as_mut() {
            while rx.try_recv().is_ok() {}
        }
    }
}

unsafe fn ensure_raw_input_class() -> Result<(), String> {
    RAW_INPUT_CLASS
        .get_or_init(|| {
            let module =
                GetModuleHandleW(None).map_err(|error| format!("get Prism module: {error}"))?;
            let class_name = wide(OBSERVER_CLASS_NAME);
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

/// The window that receives the Explorer bridge's events. With `watch_keyboard`
/// (provider mode only) it also receives raw keyboard input.
unsafe fn create_observer_window(watch_keyboard: bool) -> Result<HWND, String> {
    ensure_raw_input_class()?;
    let module = GetModuleHandleW(None).map_err(|error| format!("get Prism module: {error}"))?;
    let instance = HINSTANCE(module.0);
    let class_name = wide(OBSERVER_CLASS_NAME);
    // Message-only windows stop receiving keyboard INPUTSINK reports while an
    // elevated app (for example Task Manager) holds focus, so the Win key never
    // reaches Prism until focus leaves that window. A hidden top-level tool
    // window keeps receiving background keyboard reports.
    let window = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        PCWSTR(class_name.as_ptr()),
        PCWSTR::null(),
        WS_POPUP,
        0,
        0,
        0,
        0,
        None,
        None,
        Some(instance),
        None,
    )
    .map_err(|error| format!("create raw keyboard observer window: {error}"))?;
    let bridge_message = match shell_bridge::message_id() {
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
    // Also Explorer's TaskbarCreated broadcast, or an elevated Prism never
    // learns that Explorer restarted and never re-injects the bridge.
    let taskbar_created = taskbar_created_message();
    if taskbar_created != 0 {
        let _ = ChangeWindowMessageFilterEx(window, taskbar_created, MSGFLT_ALLOW, None);
    }
    // INPUTSINK only observes input while Prism is in the background. It does
    // not set NOLEGACY, so normal key messages and third-party tools continue
    // to receive the original keyboard stream.
    if watch_keyboard {
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
    }
    // Lock and unlock reset the press state (`WM_WTSSESSION_CHANGE`). Without
    // it, reconciling at the next key-down still recovers, so a failure here
    // is not fatal.
    if let Err(error) = WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) {
        crate::logs::debug_trace(&format!("session-notification-error {error}"));
    }
    Ok(window)
}

/// Removes a raw keyboard registration if there is one (no effect without it).
unsafe fn destroy_observer_window(window: HWND) {
    let remove = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_KEYBOARD,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: HWND::default(),
    };
    let _ = RegisterRawInputDevices(&[remove], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    let _ = WTSUnRegisterSessionNotification(window);
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
    cancel_shell_start_fallback();
}

fn schedule_win_toggle(side: WinSide, blocked_keys: [bool; 256]) {
    if let Ok(mut pending) = PENDING_WIN_TOGGLE.lock() {
        pending.arm(side, blocked_keys, Instant::now());
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ShellStartFallback {
    armed: bool,
    deadline: Option<Instant>,
}

impl ShellStartFallback {
    const EMPTY: Self = Self {
        armed: false,
        deadline: None,
    };

    fn arm(&mut self, now: Instant) {
        self.armed = true;
        self.deadline = Some(now + SHELL_START_FALLBACK_GRACE);
    }

    fn cancel(&mut self) {
        self.armed = false;
        self.deadline = None;
    }

    fn take_if_ready(&mut self, now: Instant) -> bool {
        if !self.armed {
            return false;
        }
        match self.deadline {
            Some(deadline) if now >= deadline => {
                self.armed = false;
                self.deadline = None;
                true
            }
            _ => false,
        }
    }

    fn wait_duration(&self, now: Instant) -> Option<Duration> {
        if !self.armed {
            return None;
        }
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(now))
    }
}

/// Explorer saw Win+S / Win+Q (`WM_HOTKEY`) and consumed native Search.
/// Open Prism (idempotent). Extra taps cannot close it or reopen Search.
fn note_shell_search_command() {
    if !ACTIVE.load(Ordering::Acquire)
        || !OBSERVER_ACTIVE.load(Ordering::Acquire)
        || !shell_bridge::is_active()
    {
        return;
    }
    let win_physically_down = win_keys_physically_down();
    if let Ok(mut machine) = RAW_MACHINE.lock() {
        let _ = machine.claim_search_hotkey(Instant::now(), win_physically_down);
    }
    cancel_pending_win_toggle();
    LAST_RAW_TOGGLE_MS.store(toggle_clock_ms(), Ordering::Release);
    crate::logs::debug_trace("shell-search-activate");
    queue_action(Action::ActivateSearch);
}

fn win_keys_physically_down() -> bool {
    key_physically_down(VK_LWIN.0) || key_physically_down(VK_RWIN.0)
}

/// Explorer saw a bare Win / Ctrl+Esc (`SC_TASKLIST`). In normal mode this is
/// the only Win signal, as in Open-Shell: Windows decides that the tap was
/// standalone, and an elevated foreground cannot block it. In provider mode the
/// raw observer may also report the press; a recent raw toggle skips this one.
fn note_shell_start_command() {
    if !ACTIVE.load(Ordering::Acquire)
        || !OBSERVER_ACTIVE.load(Ordering::Acquire)
        || !shell_bridge::is_active()
    {
        return;
    }
    let now = toggle_clock_ms();
    // Consume the marker: one raw press may suppress exactly one shell command.
    // Leaving it set let a later, separate Win press that only reached the shell
    // be dropped as if it were the earlier raw press.
    let last_raw = LAST_RAW_TOGGLE_MS.swap(0, Ordering::AcqRel);
    if last_raw != 0 && now.saturating_sub(last_raw) < SHELL_START_DEDUPE_MS {
        crate::logs::debug_trace("shell-start-skipped (raw recent)");
        return;
    }
    crate::logs::debug_trace("shell-start-armed");
    SHELL_START_FALLBACK
        .lock()
        .map(|mut pending| pending.arm(Instant::now()))
        .ok();
}

fn cancel_shell_start_fallback() {
    SHELL_START_FALLBACK
        .lock()
        .map(|mut pending| pending.cancel())
        .ok();
}

fn flush_shell_start_fallback() {
    let ready = SHELL_START_FALLBACK
        .lock()
        .ok()
        .is_some_and(|mut pending| pending.take_if_ready(Instant::now()));
    if ready
        && ACTIVE.load(Ordering::Acquire)
        && OBSERVER_ACTIVE.load(Ordering::Acquire)
        && shell_bridge::is_active()
    {
        crate::logs::debug_trace("shell-start-fallback-toggle");
        queue_action(Action::ToggleWin(WinSide::Left));
    }
}

fn shell_start_fallback_wait() -> Option<Duration> {
    SHELL_START_FALLBACK
        .lock()
        .ok()
        .and_then(|pending| pending.wait_duration(Instant::now()))
}

fn flush_pending_win_toggle() {
    if let Ok(mut pending) = PENDING_WIN_TOGGLE.lock() {
        let input_queued = unsafe { GetQueueStatus(QS_RAWINPUT) } >> 16 != 0;
        let side = pending.take_if_ready(Instant::now(), input_queued);
        if let Some(side) = side {
            // Keep the pending lock through enqueueing. Disable/reset paths
            // take this same lock after clearing ACTIVE, so a candidate either
            // queues before their final drain or observes inactive.
            if ACTIVE.load(Ordering::Acquire)
                && OBSERVER_ACTIVE.load(Ordering::Acquire)
                && shell_bridge::is_active()
            {
                queue_action(Action::ToggleWin(side));
                crate::logs::debug_trace("pending-toggle-queued");
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

fn key_physically_down(key: u16) -> bool {
    unsafe { GetAsyncKeyState(i32::from(key)) as u16 & 0x8000 != 0 }
}

fn non_win_keys_down() -> [bool; 256] {
    let mut down = [false; 256];
    for key in 0x08u16..=0xfe {
        if key != VK_LWIN.0 && key != VK_RWIN.0 {
            down[canonical_non_win_key(key) as usize] |= key_physically_down(key);
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

/// Explorer broadcasts this after it (re)creates the taskbar: a restarted
/// Explorer has lost Prism's hooks and registered its Start hotkeys again.
static TASKBAR_RESTARTED: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED_MESSAGE: OnceLock<u32> = OnceLock::new();

fn taskbar_created_message() -> u32 {
    *TASKBAR_CREATED_MESSAGE.get_or_init(|| {
        let name = wide("TaskbarCreated");
        unsafe { RegisterWindowMessageW(PCWSTR(name.as_ptr())) }
    })
}

unsafe extern "system" fn raw_input_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message != 0 && message == taskbar_created_message() {
        crate::logs::debug_trace("taskbar-created");
        TASKBAR_RESTARTED.store(true, Ordering::Release);
        return LRESULT(0);
    }
    if message == WM_WTSSESSION_CHANGE {
        // A lock, unlock or session switch: the releases of Win+L and every key
        // typed on the lock screen never reach this observer. Start clean, so
        // the first Win press after unlock is not taken as a held key.
        crate::logs::debug_trace(&format!("session-change {}", wparam.0));
        RAW_MACHINE.lock().map(|mut machine| machine.reset()).ok();
        cancel_pending_win_toggle();
        return LRESULT(0);
    }
    if shell_bridge::message_id().is_ok_and(|bridge_message| message == bridge_message) {
        match shell_bridge::handle_event(wparam.0, lparam.0) {
            BridgeEvent::StartCommand => note_shell_start_command(),
            BridgeEvent::SearchCommand => note_shell_search_command(),
            BridgeEvent::TaskbarPointer => shell_bridge::request_start_rect_refresh(),
            BridgeEvent::StartClick(click)
                if ACTIVE.load(Ordering::Acquire)
                    && OBSERVER_ACTIVE.load(Ordering::Acquire)
                    && shell_bridge::is_active() =>
            {
                queue_action(Action::ToggleTaskbar(click));
            }
            BridgeEvent::StartClick(_) | BridgeEvent::None => {}
        }
        return LRESULT(0);
    }
    if message == WM_INPUT
        && ACTIVE.load(Ordering::Acquire)
        && OBSERVER_ACTIVE.load(Ordering::Acquire)
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
                    .map(|mut machine| {
                        if is_down {
                            machine.forget_released(key_physically_down);
                        }
                        machine.feed(kind, is_down)
                    })
                    .unwrap_or(Decision::Pass);
                if matches!(kind, KeyKind::Win(_)) || matches!(decision, Decision::Toggle(_)) {
                    crate::logs::debug_trace(&format!("raw-key {kind:?} down={is_down} -> {decision:?}"));
                    LAST_RAW_TOGGLE_MS.store(toggle_clock_ms(), Ordering::Release);
                    cancel_shell_start_fallback();
                }
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

/* ------------------------------------------------------------------ */
/*  Unit tests - the full shortcut matrix                              */
/* ------------------------------------------------------------------ */

