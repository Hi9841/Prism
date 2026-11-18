//! The client side of the Explorer bridge (`shell-hook`, protocol in
//! `shell_protocol`): it installs the bridge DLL into Explorer and removes it,
//! finds the Start and Search buttons (UI Automation, with a child-window
//! fallback) and sends their rects, and makes the pin, foreground and Start
//! icon requests. `win_key` runs it on its observer thread and passes it the
//! bridge's events.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{BOOL, PCSTR, PCWSTR};
use windows::Win32::Foundation::{
    FreeLibrary, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CLSCTX_INPROC_SERVER,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElement,
    TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_ProcessIdPropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW,
    EnumChildWindows, FindWindowW, GetClassNameW, GetShellWindow, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, MsgWaitForMultipleObjectsEx, PeekMessageW,
    PostThreadMessageW, RegisterWindowMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, MSG, PM_REMOVE, QS_ALLINPUT, WH_GETMESSAGE, WH_MOUSE, WM_APP,
};

use crate::shell_protocol::*;
use crate::win32::wide;

/// The message that wakes the observer thread's loop (it carries no work).
pub(crate) const WAKE_MESSAGE: u32 = WM_APP + 1;
/// How often the observer thread reads the Start and Search button rects: the
/// UIA query is expensive and runs on Explorer's side.
pub(crate) const START_RECT_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// The bridge is installed and Explorer answers it.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Explorer's taskbar thread, where the requests go; 0 without a bridge.
static TASKBAR_THREAD: AtomicU32 = AtomicU32::new(0);
/// The thread of the observer window, where Explorer's answers arrive.
static PUMP_THREAD: AtomicU32 = AtomicU32::new(0);
/// Registered once; calling RegisterWindowMessageW on every raw-input message
/// (the window proc path) is wasteful.
static BRIDGE_MESSAGE_ID: OnceLock<Result<u32, String>> = OnceLock::new();
static START_CLICK_X: AtomicI32 = AtomicI32::new(0);
static PIN_REQUEST: Mutex<()> = Mutex::new(());
/// Set when taskbar geometry changes (alignment moves, resizes) so the pump
/// refreshes the Start rect immediately instead of up to 5 seconds later.
static REFRESH_REQUEST: AtomicBool = AtomicBool::new(false);

static HOTKEY_ACK: Ack = Ack::new();
static START_RECT_ACK: Ack = Ack::new();
static SEARCH_RECT_ACK: Ack = Ack::new();
static ICON_SHUTDOWN_ACK: Ack = Ack::new();
static PIN_ACK: Ack = Ack::new();
static FOREGROUND_ACK: Ack = Ack::new();

/// Explorer's answer to one request: none yet, or success or failure.
struct Ack {
    answer: Mutex<Option<bool>>,
    answered: Condvar,
}

impl Ack {
    const fn new() -> Self {
        Self { answer: Mutex::new(None), answered: Condvar::new() }
    }

    fn reset(&self) {
        *self.answer.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn set(&self, ok: bool) {
        *self.answer.lock().unwrap_or_else(|e| e.into_inner()) = Some(ok);
        self.answered.notify_all();
    }

    fn get(&self) -> Option<bool> {
        *self.answer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits at most `timeout` for the answer. The answer arrives through the
    /// observer window: on its thread the wait dispatches messages, on any
    /// other thread it sleeps until `set`.
    fn wait(&self, timeout: Duration) -> Option<bool> {
        let started = Instant::now();
        if unsafe { GetCurrentThreadId() } != PUMP_THREAD.load(Ordering::Acquire) {
            let answer = self.answer.lock().unwrap_or_else(|e| e.into_inner());
            let (answer, _) = self
                .answered
                .wait_timeout_while(answer, timeout, |answer| answer.is_none())
                .unwrap_or_else(|e| e.into_inner());
            return *answer;
        }
        let mut msg = MSG::default();
        while started.elapsed() < timeout {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                }
            }
            if let Some(answer) = self.get() {
                return Some(answer);
            }
            unsafe {
                let _ = MsgWaitForMultipleObjectsEx(None, 25, QS_ALLINPUT, Default::default());
            }
        }
        self.get()
    }
}

/// The observer thread of `win_key`: answers arrive there, and a Start rect
/// refresh wakes it. 0 when it ends.
pub(crate) fn attach_pump(thread: u32) {
    PUMP_THREAD.store(thread, Ordering::Release);
}

/// The installed bridge now serves requests.
pub(crate) fn connect(bridge: &ShellBridge) {
    ACTIVE.store(true, Ordering::Release);
    TASKBAR_THREAD.store(bridge.taskbar_thread, Ordering::Release);
}

/// The bridge stops serving requests and events (observation stops).
pub(crate) fn deactivate() {
    ACTIVE.store(false, Ordering::Release);
}

/// The bridge is gone (the observer thread ends).
pub(crate) fn disconnect() {
    ACTIVE.store(false, Ordering::Release);
    TASKBAR_THREAD.store(0, Ordering::Release);
}

pub(crate) fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// What `win_key` does with an event of the bridge; the answers to requests
/// are handled here.
pub(crate) enum BridgeEvent {
    None,
    StartCommand,
    SearchCommand,
    TaskbarPointer,
    StartClick(POINT),
}

/// Handles one bridge message (`wparam` is the code, `lparam` the detail).
pub(crate) fn handle_event(code: usize, detail: isize) -> BridgeEvent {
    match code {
        EVENT_HOTKEY_DISABLED => HOTKEY_ACK.set(detail != 0),
        EVENT_START_RECT_CONFIGURED => START_RECT_ACK.set(detail != 0),
        EVENT_SEARCH_RECT_CONFIGURED => SEARCH_RECT_ACK.set(detail != 0),
        EVENT_START_ICON_SHUTDOWN => ICON_SHUTDOWN_ACK.set(detail != 0),
        EVENT_START_ICON_REFRESHED => crate::logs::debug_trace(&format!("start-icon-refresh {detail}")),
        EVENT_TASKBAR_PIN_COMPLETED => PIN_ACK.set(detail != 0),
        EVENT_FOREGROUND_RESULT => FOREGROUND_ACK.set(detail != 0),
        EVENT_SHELL_START_COMMAND => return BridgeEvent::StartCommand,
        EVENT_SHELL_SEARCH_COMMAND => return BridgeEvent::SearchCommand,
        EVENT_TASKBAR_POINTER => return BridgeEvent::TaskbarPointer,
        EVENT_TASKBAR_START_CLICK_X => START_CLICK_X.store(detail as i32, Ordering::Release),
        EVENT_TASKBAR_START_CLICK_Y => {
            return BridgeEvent::StartClick(POINT { x: START_CLICK_X.load(Ordering::Acquire), y: detail as i32 });
        }
        _ => {}
    }
    BridgeEvent::None
}

/// Asks the observer thread to read the Start button rectangle now. Taskbar
/// moves (alignment repair, density changes) should reposition the glyph
/// overlay at once rather than on the next interval tick.
pub fn request_start_rect_refresh() {
    if !is_active() {
        return;
    }
    REFRESH_REQUEST.store(true, Ordering::Release);
    let thread = PUMP_THREAD.load(Ordering::Acquire);
    if thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(thread, WAKE_MESSAGE, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn notify_start_icon_changed() {
    let taskbar_thread = TASKBAR_THREAD.load(Ordering::Acquire);
    let Ok(message) = message_id() else {
        return;
    };
    if taskbar_thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(
                taskbar_thread,
                message,
                WPARAM(CONTROL_START_ICON_REFRESH),
                LPARAM(0),
            );
        }
    }
}

pub fn taskbar_pin(path: &Path, pinned: bool) -> Result<(), String> {
    if !ACTIVE.load(Ordering::Acquire) {
        return Err("shell bridge not active".into());
    }
    let _request = PIN_REQUEST
        .lock()
        .map_err(|_| "taskbar pin request lock is poisoned".to_string())?;
    let taskbar_thread = TASKBAR_THREAD.load(Ordering::Acquire);
    if taskbar_thread == 0 {
        return Err("shell taskbar thread not available".into());
    }

    let target_file = pin_target_path().ok_or("no folder for the pin request")?;
    if let Some(parent) = target_file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create pin request directory: {error}"))?;
    }
    std::fs::write(&target_file, path.to_string_lossy().as_bytes())
        .map_err(|error| format!("failed to write pin target file: {error}"))?;

    PIN_ACK.reset();
    let control = if pinned {
        CONTROL_TASKBAR_PIN
    } else {
        CONTROL_TASKBAR_UNPIN
    };
    unsafe {
        PostThreadMessageW(
            taskbar_thread,
            message_id()?,
            WPARAM(control),
            LPARAM(0),
        )
        .map_err(|error| {
            let _ = std::fs::remove_file(&target_file);
            format!("failed to post pin request to Explorer: {error}")
        })?;
    }

    let outcome = match PIN_ACK.wait(Duration::from_millis(1500)) {
        Some(true) => Ok(()),
        Some(false) => Err("Explorer rejected the taskbar pin request".into()),
        None => Err("Explorer taskbar pin request timed out".into()),
    };
    let _ = std::fs::remove_file(&target_file);
    outcome
}

/// Asks the injected Explorer bridge to hand Prism the foreground. Explorer is
/// the shell that handled the Win key, so it may still hold activation
/// permission that a medium-integrity Prism process cannot claim directly.
/// This is the closest cheap analogue to Open-Shell hosting its Start menu as
/// an Explorer-owned window.
pub fn request_foreground(hwnd: isize) -> bool {
    if !ACTIVE.load(Ordering::Acquire) || hwnd == 0 {
        return false;
    }
    let Ok(message) = message_id() else {
        return false;
    };
    let taskbar_thread = TASKBAR_THREAD.load(Ordering::Acquire);
    if taskbar_thread == 0 {
        return false;
    }
    FOREGROUND_ACK.reset();
    unsafe {
        PostThreadMessageW(
            taskbar_thread,
            message,
            WPARAM(CONTROL_FOREGROUND_WINDOW),
            LPARAM(hwnd),
        )
        .is_ok()
    }
}

/// Explorer's answer to the last [`request_foreground`]: `None` until it
/// replies, then whether its `SetForegroundWindow` succeeded.
pub fn foreground_ack() -> Option<bool> {
    FOREGROUND_ACK.get()
}

type ShellHookProc = unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT;

pub(crate) struct ShellBridge {
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
    /// The hooks are gone (`release`); the struct only keeps the last rects.
    released: bool,
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
    pub(crate) unsafe fn install(release_win_hotkey: bool) -> Result<Self, String> {
        let library_path = shell_hook_library()?;
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
                    return Err("Explorer mouse bridge export is missing".to_string());
                }
            };

        let progman = match find_shell_window("Progman") {
            Ok(window) => window,
            Err(error) => {
                let _ = FreeLibrary(module);
                return Err(error);
            }
        };
        let progman_thread = GetWindowThreadProcessId(progman, None);
        if progman_thread == 0 {
            let _ = FreeLibrary(module);
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
                return Err(format!("hook Explorer Start command: {error}"));
            }
        };

        let taskbar = match find_shell_window("Shell_TrayWnd") {
            Ok(window) => window,
            Err(error) => {
                cleanup_shell_hook(progman_hook, module);
                return Err(error);
            }
        };
        let taskbar_thread = GetWindowThreadProcessId(taskbar, None);
        if taskbar_thread == 0 {
            cleanup_shell_hook(progman_hook, module);
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
                    cleanup_shell_hook(progman_hook, module);
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
            cleanup_shell_hook(progman_hook, module);
            return Err("Explorer taskbar process is unavailable".to_string());
        }
        let (start_button_locator, start_rect) =
            match StartButtonLocator::new(taskbar, taskbar_process_id) {
                Ok(locator) => locator,
                Err(error) => {
                    if let Some(hook) = taskbar_message_hook {
                        let _ = UnhookWindowsHookEx(hook);
                    }
                    cleanup_shell_hook(progman_hook, module);
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
                cleanup_shell_hook(progman_hook, module);
                return Err(format!("hook Explorer Start-button mouse path: {error}"));
            }
        };
        let bridge_message = match message_id() {
            Ok(message) => message,
            Err(error) => {
                let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
                if let Some(hook) = taskbar_message_hook {
                    let _ = UnhookWindowsHookEx(hook);
                }
                cleanup_shell_hook(progman_hook, module);
                return Err(error);
            }
        };
        START_RECT_ACK.reset();
        if let Err(error) = post_start_button_rect(taskbar_thread, bridge_message, start_rect) {
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module);
            return Err(error);
        }
        if START_RECT_ACK.wait(Duration::from_secs(1)) != Some(true) {
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module);
            return Err("Explorer did not accept the Start-button rectangle".to_string());
        }

        SEARCH_RECT_ACK.reset();
        let _ = post_search_button_rect(taskbar_thread, bridge_message, search_rect);
        let _ = SEARCH_RECT_ACK.wait(Duration::from_millis(250));

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
                released: false,
            });
        }

        let app_manager = match find_shell_window("ApplicationManager_ImmersiveShellWindow") {
            Ok(window) => window,
            Err(error) => {
                let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
                if let Some(hook) = taskbar_message_hook {
                    let _ = UnhookWindowsHookEx(hook);
                }
                cleanup_shell_hook(progman_hook, module);
                return Err(error);
            }
        };
        let app_manager_thread = GetWindowThreadProcessId(app_manager, None);
        if app_manager_thread == 0 {
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module);
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
                    cleanup_shell_hook(progman_hook, module);
                    return Err(format!("enter Explorer hotkey thread: {error}"));
                }
            }
        };

        HOTKEY_ACK.reset();
        let message = match message_id() {
            Ok(message) => message,
            Err(error) => {
                if let Some(hook) = app_manager_hook {
                    let _ = UnhookWindowsHookEx(hook);
                }
                let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
                if let Some(hook) = taskbar_message_hook {
                    let _ = UnhookWindowsHookEx(hook);
                }
                cleanup_shell_hook(progman_hook, module);
                return Err(error);
            }
        };
        if let Err(error) = PostThreadMessageW(
            app_manager_thread,
            message,
            WPARAM(CONTROL_DISABLE_WIN_HOTKEY),
            LPARAM(0),
        ) {
            if let Some(hook) = app_manager_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module);
            return Err(format!("ask Explorer to release bare Win: {error}"));
        }

        let acknowledged = HOTKEY_ACK.wait(Duration::from_secs(1));
        if acknowledged.is_none() {
            if let Some(hook) = app_manager_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            let _ = UnhookWindowsHookEx(taskbar_mouse_hook);
            if let Some(hook) = taskbar_message_hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            cleanup_shell_hook(progman_hook, module);
            return Err("Explorer did not acknowledge the bare-Win handoff".to_string());
        }

        #[cfg(debug_assertions)]
        eprintln!(
            "[win-key] Explorer bridge active (Progman thread {progman_thread}, hotkey thread {app_manager_thread}, hotkey {})",
            if acknowledged == Some(true) {
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
            released: false,
        })
    }

    pub(crate) fn start_rect(&self) -> Option<RECT> {
        valid_rect(self.start_rect).then_some(self.start_rect)
    }

    pub(crate) fn refresh_start_rect(&mut self) {
        let requested = REFRESH_REQUEST.swap(false, Ordering::AcqRel);
        let now = Instant::now();
        if !requested
            && self
                .last_rect_refresh
                .is_some_and(|last| now.duration_since(last) < START_RECT_REFRESH_INTERVAL)
        {
            return;
        }
        self.last_rect_refresh = Some(now);

        let Ok(message) = message_id() else {
            return;
        };

        if let Some(rect) = self.start_button_locator.rect() {
            if !same_rect(rect, self.start_rect)
                && post_start_button_rect(self.taskbar_thread, message, rect).is_ok()
            {
                crate::logs::debug_trace(&format!(
                    "start-rect {},{},{},{} requested={requested}",
                    rect.left, rect.top, rect.right, rect.bottom
                ));
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

/// Elements of Explorer's taskbar: its process id, or 0. In a Remote Desktop session (Windows
/// Sandbox is one) the taskbar's XAML elements report process id 0; every search starts at
/// Explorer's own taskbar window, so 0 there is Explorer too.
unsafe fn explorer_process_condition(
    uia: &IUIAutomation,
    taskbar_process_id: u32,
) -> Result<IUIAutomationCondition, String> {
    let process_id: VARIANT = (taskbar_process_id as i32).into();
    let explorer = uia
        .CreatePropertyCondition(UIA_ProcessIdPropertyId, &process_id)
        .map_err(|error| format!("match Explorer process: {error}"))?;
    let unreported: VARIANT = 0i32.into();
    let unreported = uia
        .CreatePropertyCondition(UIA_ProcessIdPropertyId, &unreported)
        .map_err(|error| format!("match unreported process: {error}"))?;
    uia.CreateOrCondition(&explorer, &unreported)
        .map_err(|error| format!("combine Explorer process conditions: {error}"))
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
    let process_condition = explorer_process_condition(&uia, taskbar_process_id)?;
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
        (CONTROL_START_RECT_LEFT, rect.left),
        (CONTROL_START_RECT_TOP, rect.top),
        (CONTROL_START_RECT_RIGHT, rect.right),
        (CONTROL_START_RECT_BOTTOM, rect.bottom),
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
        (CONTROL_SEARCH_RECT_LEFT, rect.left),
        (CONTROL_SEARCH_RECT_TOP, rect.top),
        (CONTROL_SEARCH_RECT_RIGHT, rect.right),
        (CONTROL_SEARCH_RECT_BOTTOM, rect.bottom),
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

    let process_condition = explorer_process_condition(&uia, taskbar_process_id)?;
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
        self.release();
    }
}

impl ShellBridge {
    /// Removes the hooks from Explorer. Safe to call more than once.
    pub(crate) fn release(&mut self) {
        if std::mem::replace(&mut self.released, true) {
            return;
        }
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
        #[cfg(debug_assertions)]
        eprintln!("[win-key] Explorer bridge released");
    }
}

fn request_start_icon_shutdown(thread: u32) {
    let Ok(message) = message_id() else {
        return;
    };
    ICON_SHUTDOWN_ACK.reset();
    unsafe {
        let _ = PostThreadMessageW(
            thread,
            message,
            WPARAM(CONTROL_START_ICON_SHUTDOWN),
            LPARAM(0),
        );
    }
    let _ = ICON_SHUTDOWN_ACK.wait(Duration::from_millis(250));
}

unsafe fn cleanup_shell_hook(hook: HHOOK, module: HMODULE) {
    let _ = UnhookWindowsHookEx(hook);
    let _ = FreeLibrary(module);
}

pub(crate) fn message_id() -> Result<u32, String> {
    BRIDGE_MESSAGE_ID
        .get_or_init(|| {
            let name = wide(BRIDGE_MESSAGE_NAME);
            let message = unsafe { RegisterWindowMessageW(PCWSTR(name.as_ptr())) };
            if message == 0 {
                Err("register Explorer bridge message failed".to_string())
            } else {
                Ok(message)
            }
        })
        .clone()
}

/// Explorer's desktop and taskbar windows exist (the bridge can install).
/// A cheap check for startup retries: a sign-in launch can run before
/// Explorer has created its taskbar.
pub fn shell_windows_ready() -> bool {
    unsafe {
        let taskbar = wide("Shell_TrayWnd");
        !GetShellWindow().0.is_null()
            && FindWindowW(PCWSTR(taskbar.as_ptr()), PCWSTR::null()).is_ok_and(|window| !window.0.is_null())
    }
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

/// The Explorer bridge DLL: one fixed file for each build of it, at
/// `%LOCALAPPDATA%\app.prism.launcher\shell-hook\prism-shell-hook-<hash>.dll`.
/// Before 1.0.30, each start wrote a new file with a random name to `%TEMP%` and loaded it into
/// Explorer. Antivirus tools flag that pattern. The uninstaller removes this folder.
fn shell_hook_library() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?;
    let directory = PathBuf::from(local).join(DATA_DIR_NAME).join("shell-hook");
    // Files that versions before 1.0.30 left in %TEMP%.
    remove_stale_shell_hooks(&std::env::temp_dir().join("Prism").join("shell-hooks"), "");
    install_shell_hook_library(&directory, include_bytes!(env!("PRISM_SHELL_HOOK_DLL")))
}

/// Puts `bytes` at `<directory>\prism-shell-hook-<hash>.dll` and returns the path. A file with the
/// same bytes stays as it is. A new build has a new name, so an update never has to replace a file
/// that Explorer holds locked.
fn install_shell_hook_library(directory: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory).map_err(|error| format!("create Explorer bridge directory: {error}"))?;
    let name = format!("prism-shell-hook-{:016x}.dll", crate::fnv1a_64(bytes));
    remove_stale_shell_hooks(directory, &name);
    let path = directory.join(&name);
    if std::fs::read(&path).is_ok_and(|existing| existing == bytes) {
        return Ok(path);
    }
    // Write a private file, then rename it: a second Prism that starts at the same time never
    // loads a DLL that is not fully written.
    let staging = directory.join(format!("{name}.{}.tmp", std::process::id()));
    let written = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&staging)
        .and_then(|mut file| file.write_all(bytes).and_then(|_| file.sync_all()));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staging);
        return Err(format!("write Explorer bridge DLL: {error}"));
    }
    match std::fs::rename(&staging, &path) {
        Ok(()) => Ok(path),
        Err(error) => {
            let _ = std::fs::remove_file(&staging);
            // Another Prism can have put the same file there first.
            if std::fs::read(&path).is_ok_and(|existing| existing == bytes) {
                Ok(path)
            } else {
                Err(format!("install Explorer bridge DLL: {error}"))
            }
        }
    }
}

/// Removes bridge DLLs other than `keep`. Explorer holds a loaded DLL locked, so this removes
/// only files that no Explorer uses now.
fn remove_stale_shell_hooks(directory: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if name != keep && name.starts_with("prism-shell-hook-") && name.ends_with(".dll") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::install_shell_hook_library;
    use std::path::{Path, PathBuf};

    fn scratch(label: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("shell-hook-test-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_same_build_keeps_one_file_with_a_stable_name() {
        let dir = scratch("stable");
        let first = install_shell_hook_library(&dir, b"build one").unwrap();
        let second = install_shell_hook_library(&dir, b"build one").unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"build one");
        assert_eq!(file_names(&dir), [first.file_name().unwrap().to_str().unwrap()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_build_gets_a_new_name_and_the_old_file_goes() {
        let dir = scratch("update");
        let old = install_shell_hook_library(&dir, b"build one").unwrap();
        let new = install_shell_hook_library(&dir, b"build two").unwrap();
        assert_ne!(old, new);
        assert!(!old.exists());
        assert_eq!(file_names(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_is_written_again() {
        let dir = scratch("damaged");
        let path = install_shell_hook_library(&dir, b"build one").unwrap();
        std::fs::write(&path, b"half").unwrap();
        assert_eq!(install_shell_hook_library(&dir, b"build one").unwrap(), path);
        assert_eq!(std::fs::read(&path).unwrap(), b"build one");
        assert_eq!(file_names(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
