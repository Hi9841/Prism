//! The palette window lifecycle (Prism `src-tauri/src/lib.rs`, the window
//! half): toggles, presentation, raise retries, click-away, deferred hide, and
//! the window commands the UI calls through [`prism_ui::lib::bridge::Shell`].

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{App, AppContext, AsyncApp, Task, Timer};
use prism_core::backend::backend;
use prism_core::palette_window::{
    self, DEFAULT_LOGICAL_WIDTH, PresentationAnchor, PresentationSource, is_prism_foreground, log_foreground,
    presentation_anchor,
};
use prism_core::{drag, taskbar, taskbar_alignment, win_key};
use prism_ui::lib::bridge::{self, BackendEvent, BridgeResult};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, HWND_NOTOPMOST, HWND_TOPMOST, IsWindowVisible, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, ShowWindow,
};

/// Keep this >= the UI's LAUNCHER_MOTION_MS (110 ms) so the view can fade out.
const PALETTE_HIDE_DELAY: Duration = Duration::from_millis(125);
/// Reassert over ~500 ms, not once: the Explorer bridge grants foreground
/// asynchronously and an elevated or fullscreen owner can deny the first
/// SetForegroundWindow.
const PALETTE_RAISE_RETRY_DELAYS: &[u64] = &[40, 120, 250, 500];
/// A focus loss right after opening is part of the activation burst, not a click-away.
const ACTIVATION_FOCUS_GRACE: Duration = Duration::from_millis(600);

static PALETTE_OPEN: AtomicBool = AtomicBool::new(false);
static PALETTE_TRANSITION: AtomicU64 = AtomicU64::new(0);
static ACTIVATION_FOCUS_PENDING: AtomicBool = AtomicBool::new(false);
/// This open has already put Prism in the foreground; a later raise retry
/// would steal Alt-Tab and launches back.
static PALETTE_TOOK_FOREGROUND: AtomicBool = AtomicBool::new(false);
static PRESENTATION_ANCHOR: Mutex<Option<PresentationAnchor>> = Mutex::new(None);
/// When the last open started (`now_ms`).
static LAST_OPEN_MS: AtomicU64 = AtomicU64::new(0);
/// An open shows the window within a frame; past this, an "open" state with
/// a hidden window is stale.
const STALE_OPEN_MS: u64 = 500;

/// Milliseconds on the monotonic clock: a wall-clock change must not make an
/// open look stale.
fn now_ms() -> u64 {
    static START: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);
    START.elapsed().as_millis() as u64
}
pub static LOGICAL_WIDTH: AtomicU32 = AtomicU32::new(DEFAULT_LOGICAL_WIDTH);

/// GPUI gives popup windows the resize-border insets of a framed window
/// (8 px left, right, and bottom). The palette has no frame: the client area
/// must be the whole window, as for the Tauri window.
pub fn remove_frame(hwnd: HWND) {
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{SWP_FRAMECHANGED, WM_NCCALCSIZE};

    unsafe extern "system" fn frameless(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if msg == WM_NCCALCSIZE && wparam.0 != 0 {
            return LRESULT(0);
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(frameless), 1, 0);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Windows sends WM_SIZE synchronously inside SetWindowPos. The shell resizes
/// from GPUI updates, where GPUI cannot run its resize callback: the renderer
/// grew but the layout kept the old width (the width setting looked dead).
/// A posted WM_SIZE arrives after the update and relays the new client size.
fn repost_size(hwnd: HWND) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, PostMessageW, SIZE_RESTORED, WM_SIZE};
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        return;
    }
    let width = (client.right - client.left).clamp(0, 0xFFFF) as isize;
    let height = (client.bottom - client.top).clamp(0, 0xFFFF) as isize;
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_SIZE, WPARAM(SIZE_RESTORED as usize), LPARAM(width | (height << 16)));
    }
}

thread_local! {
    static INSTANCE: std::cell::RefCell<Option<std::rc::Rc<GpuiShell>>> = const { std::cell::RefCell::new(None) };
}

/// The shell for deferred native work.
pub fn instance() -> Option<std::rc::Rc<GpuiShell>> {
    INSTANCE.with(|i| i.borrow().clone())
}

pub fn set_instance(shell: std::rc::Rc<GpuiShell>) {
    INSTANCE.with(|i| *i.borrow_mut() = Some(shell));
}

pub struct GpuiShell {
    hwnd: Cell<isize>,
    pub palette_visible: Arc<AtomicBool>,
}

impl GpuiShell {
    pub fn new(palette_visible: Arc<AtomicBool>) -> Self {
        Self { hwnd: Cell::new(0), palette_visible }
    }

    pub fn set_hwnd(&self, hwnd: HWND) {
        self.hwnd.set(hwnd.0 as isize);
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd.get() as *mut _)
    }

    pub fn hwnd_raw(&self) -> isize {
        self.hwnd.get()
    }

    /// Timers check the transition counter when they fire, so they can run detached.
    fn keep(&self, task: Task<()>) {
        task.detach();
    }

    fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd()) }.as_bool()
    }

    fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd(), SW_SHOW);
        }
        self.palette_visible.store(true, Ordering::Release);
    }

    fn hide_window(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd(), SW_HIDE);
        }
        self.palette_visible.store(false, Ordering::Release);
    }

    /// The physical frame for the preferred logical size; resizes the window when needed.
    fn palette_target(&self, anchor: Option<PresentationAnchor>, alignment: taskbar_alignment::Alignment) -> Result<(i32, i32), String> {
        let hwnd = self.hwnd();
        let frame = palette_window::palette_frame(hwnd, anchor, alignment, LOGICAL_WIDTH.load(Ordering::Acquire))?;
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|e| e.to_string())?;
        if rect.right - rect.left != frame.width || rect.bottom - rect.top != frame.height {
            unsafe {
                SetWindowPos(hwnd, None, 0, 0, frame.width, frame.height, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE)
            }
            .map_err(|e| e.to_string())?;
            repost_size(hwnd);
        }
        Ok((frame.x, frame.y))
    }

    fn position_palette(&self, anchor: Option<PresentationAnchor>) {
        let Ok((x, y)) = self.palette_target(anchor, taskbar_alignment::current()) else { return };
        unsafe {
            let _ = SetWindowPos(self.hwnd(), None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    fn reconcile_palette_position(&self, anchor: Option<PresentationAnchor>) -> Result<(), String> {
        let (x, y) = self.palette_target(anchor, taskbar_alignment::current())?;
        taskbar_alignment::reapply_with_companion(taskbar_alignment::CompanionMove { window: self.hwnd(), x, y })
    }

    fn raise_palette(&self) -> Result<(), String> {
        // Never Window::activate_window: it injects a synthetic Alt key (A8).
        palette_window::raise(self.hwnd())
    }

    fn schedule_palette_raise_retry(&self, transition: u64, cx: &mut App) {
        for &delay_ms in PALETTE_RAISE_RETRY_DELAYS {
            let hwnd = self.hwnd.get();
            let task = cx.spawn(async move |_cx: &mut AsyncApp| {
                Timer::after(Duration::from_millis(delay_ms)).await;
                if !palette_raise_still_wanted(transition) {
                    return;
                }
                if is_prism_foreground() {
                    PALETTE_TOOK_FOREGROUND.store(true, Ordering::Release);
                    return;
                }
                let _ = palette_window::raise(HWND(hwnd as *mut _));
                if is_prism_foreground() {
                    PALETTE_TOOK_FOREGROUND.store(true, Ordering::Release);
                }
            });
            self.keep(task);
        }
    }

    fn schedule_palette_hide(&self, transition: u64, cx: &mut App) {
        let hwnd = self.hwnd.get();
        let visible = self.palette_visible.clone();
        let task = cx.spawn(async move |_cx: &mut AsyncApp| {
            Timer::after(PALETTE_HIDE_DELAY).await;
            if PALETTE_TRANSITION.load(Ordering::Acquire) == transition && !PALETTE_OPEN.load(Ordering::Acquire) {
                unsafe {
                    let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
                }
                visible.store(false, Ordering::Release);
                taskbar::release();
            }
        });
        self.keep(task);
    }

    pub fn toggle_palette(&self, cx: &mut App) {
        self.toggle_palette_with_presentation(PresentationSource::ConfiguredShortcut, None, cx);
    }

    pub fn toggle_palette_from_win(&self, start_button: Option<RECT>, cx: &mut App) {
        let anchor = start_button.map(|rect| presentation_anchor(Some(rect), None));
        self.toggle_palette_with_presentation(PresentationSource::WinKey, anchor, cx);
    }

    pub fn toggle_palette_from_taskbar(&self, click: POINT, start_button: Option<RECT>, cx: &mut App) {
        let anchor = Some(presentation_anchor(start_button, Some(click)));
        self.toggle_palette_with_presentation(PresentationSource::TaskbarStartClick, anchor, cx);
    }

    fn toggle_palette_with_presentation(
        &self,
        source: PresentationSource,
        anchor: Option<PresentationAnchor>,
        cx: &mut App,
    ) {
        let timer = prism_core::perf::start();
        // A press must never be spent on a stale state: "open" with no visible
        // window (and no open in progress) means the palette is closed.
        let now = now_ms();
        if PALETTE_OPEN.load(Ordering::Acquire)
            && !self.visible()
            && now.saturating_sub(LAST_OPEN_MS.load(Ordering::Acquire)) > STALE_OPEN_MS
        {
            win_key::debug_trace("palette-toggle resync stale open");
            PALETTE_OPEN.store(false, Ordering::Release);
        }
        let opening = !PALETTE_OPEN.fetch_xor(true, Ordering::AcqRel);
        if opening {
            LAST_OPEN_MS.store(now, Ordering::Release);
        }
        let transition = PALETTE_TRANSITION.fetch_add(1, Ordering::AcqRel) + 1;
        log_foreground(if opening { "toggle-open-before" } else { "toggle-close-before" });
        win_key::debug_trace(&format!("palette-toggle open={opening}"));
        let _ = source;
        if opening {
            PALETTE_TOOK_FOREGROUND.store(false, Ordering::Release);
            backend().schedule_installed_apps_refresh();
            // Windows can report the palette as unfocused once while it is still
            // acquiring foreground from an elevated window.
            ACTIVATION_FOCUS_PENDING.store(true, Ordering::Release);
            let task = cx.spawn(async move |cx: &mut AsyncApp| {
                Timer::after(ACTIVATION_FOCUS_GRACE).await;
                if PALETTE_TRANSITION.load(Ordering::Acquire) != transition {
                    return;
                }
                ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
                // The grace can swallow a real click on another window, and
                // Windows sends no second blur. A palette that held the
                // foreground and lost it during the grace was clicked away.
                if PALETTE_TOOK_FOREGROUND.load(Ordering::Acquire) {
                    let _ = cx.update(|cx| {
                        if let Some(shell) = instance() {
                            shell.dismiss_on_blur("palette-hide-blur-after-grace", cx);
                        }
                    });
                }
            });
            self.keep(task);
            PRESENTATION_ANCHOR.lock().map(|mut value| *value = anchor).ok();
        } else {
            ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
            PRESENTATION_ANCHOR.lock().map(|mut value| *value = None).ok();
            self.schedule_palette_hide(transition, cx);
        }
        // Send the desired state, not an ambiguous toggle.
        bridge::emit_event(BackendEvent::Toggle { open: opening }, cx);
        prism_core::perf::finish(timer, "palette_toggle", || format!("open={opening}"));
    }

    /// A launch or second instance: open the palette and present it.
    pub fn activate_palette(&self, cx: &mut App) {
        ACTIVATION_FOCUS_PENDING.store(true, Ordering::Release);
        if !PALETTE_OPEN.load(Ordering::Acquire) {
            self.toggle_palette(cx);
        }
        let _ = self.present(cx);
    }

    /// Moves, sizes, and shows the window from a foreground task: a synchronous
    /// resize inside a GPUI update would lose the window's resize notification.
    fn present(&self, cx: &mut App) -> BridgeResult<bool> {
        if !PALETTE_OPEN.load(Ordering::Acquire) {
            return Ok(false);
        }
        cx.spawn(async move |cx: &mut AsyncApp| {
            let _ = cx.update(|cx| {
                if let Some(shell) = instance() {
                    shell.present_native(cx);
                }
            });
        })
        .detach();
        Ok(true)
    }

    fn present_native(&self, cx: &mut App) {
        if !PALETTE_OPEN.load(Ordering::Acquire) {
            return;
        }
        let timer = prism_core::perf::start();
        taskbar::present();
        let anchor = PRESENTATION_ANCHOR.lock().ok().and_then(|value| *value);
        // Reconcile the taskbar and Prism together on every presentation.
        if self.reconcile_palette_position(anchor).is_err() {
            self.position_palette(anchor);
        }
        self.show();
        // A transient focus denial must not fail the presentation after the
        // window is already shown; the bridge and the retries finish the job.
        let _ = self.raise_palette();
        if !is_prism_foreground() {
            let granted = win_key::request_foreground(self.hwnd.get());
            win_key::debug_trace(&format!("explorer-fg granted={granted} self={}", is_prism_foreground()));
        }
        if is_prism_foreground() {
            PALETTE_TOOK_FOREGROUND.store(true, Ordering::Release);
        }
        self.schedule_palette_raise_retry(PALETTE_TRANSITION.load(Ordering::Acquire), cx);
        log_foreground("present-after");
        prism_core::perf::finish(timer, "palette_present", || "window=main".to_string());
    }
}

fn palette_raise_still_wanted(transition: u64) -> bool {
    PALETTE_OPEN.load(Ordering::Acquire)
        && PALETTE_TRANSITION.load(Ordering::Acquire) == transition
        && !PALETTE_TOOK_FOREGROUND.load(Ordering::Acquire)
}

fn begin_palette_close() -> u64 {
    PALETTE_OPEN.store(false, Ordering::Release);
    ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
    let transition = PALETTE_TRANSITION.fetch_add(1, Ordering::AcqRel) + 1;
    PRESENTATION_ANCHOR.lock().map(|mut value| *value = None).ok();
    transition
}

fn is_animatable_window_width(width: u32) -> bool {
    (560..=720).contains(&width)
}

impl bridge::Shell for GpuiShell {
    fn present_palette(&self, cx: &mut App) -> BridgeResult<bool> {
        self.present(cx)
    }

    fn hide_palette(&self, defer: bool, cx: &mut App) -> BridgeResult<()> {
        win_key::debug_trace(&format!("palette-hide-command defer={defer}"));
        let transition = begin_palette_close();
        if defer {
            self.schedule_palette_hide(transition, cx);
            return Ok(());
        }
        self.hide_window();
        taskbar::release();
        Ok(())
    }

    fn is_window_visible(&self, _cx: &mut App) -> bool {
        self.visible()
    }

    fn set_always_on_top(&self, on: bool, _cx: &mut App) -> BridgeResult<()> {
        unsafe {
            SetWindowPos(
                self.hwnd(),
                Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        }
        .map_err(|e| e.to_string())
    }

    /// The Tauri app set the native window theme and cleared DWM materials.
    /// The GPUI window has no native material or frame to theme.
    fn set_window_style(&self, theme: &str, _cx: &mut App) -> BridgeResult<()> {
        match theme {
            "light" | "dark" => Ok(()),
            _ => Err(format!("unknown theme '{theme}'")),
        }
    }

    fn set_window_width(&self, width: u32, cx: &mut App) -> BridgeResult<()> {
        // Persisted choices remain the three presets; intermediate values are
        // accepted only inside their bounds so the UI can animate the width.
        if !is_animatable_window_width(width) {
            return Err(format!("unsupported width '{width}'"));
        }
        LOGICAL_WIDTH.store(width, Ordering::Release);
        cx.spawn(async move |cx: &mut AsyncApp| {
            let _ = cx.update(|_| {
                let Some(shell) = instance() else { return };
                let anchor = PRESENTATION_ANCHOR.lock().ok().and_then(|value| *value);
                if shell.reconcile_palette_position(anchor).is_err() {
                    shell.position_palette(anchor);
                }
            });
        })
        .detach();
        Ok(())
    }

    fn set_taskbar_alignment(&self, alignment: &str, _cx: &mut App) -> BridgeResult<()> {
        let alignment = taskbar_alignment::Alignment::parse(alignment)?;
        let anchor = PRESENTATION_ANCHOR.lock().ok().and_then(|value| *value);
        let (x, y) = self.palette_target(anchor, alignment)?;
        taskbar_alignment::set_with_companion(alignment, taskbar_alignment::CompanionMove { window: self.hwnd(), x, y })
    }

    fn start_file_drag(&self, paths: Vec<String>, cx: &mut App) -> Task<BridgeResult<bool>> {
        let path_bufs = match backend().drag_paths(paths) {
            Ok(paths) => paths,
            Err(error) => return Task::ready(Err(error)),
        };
        // From a foreground task: DoDragDrop runs a modal loop (A7).
        cx.spawn(async move |cx: &mut AsyncApp| {
            let result = drag::drag_files(&path_bufs)?;
            let dropped = result == drag::DragResult::Dropped;
            if dropped {
                let _ = cx.update(|cx| {
                    let shell = bridge::shell(cx);
                    let _ = shell.hide_palette(false, cx);
                });
            }
            Ok(dropped)
        })
    }

    fn quit(&self, cx: &mut App) -> Task<BridgeResult<()>> {
        // The Win-key bridge teardown can take up to two seconds; keep it off the UI thread.
        let teardown = cx.background_spawn(async move { backend().teardown_for_exit() });
        cx.spawn(async move |cx: &mut AsyncApp| {
            teardown.await;
            cx.update(|cx| cx.quit()).map_err(|e| e.to_string())
        })
    }

    fn app_version(&self) -> String {
        env!("PRISM_VERSION").to_owned()
    }

    fn owns_foreground(&self) -> bool {
        is_prism_foreground()
    }

    fn palette_held_open(&self) -> bool {
        PALETTE_OPEN.load(Ordering::Acquire)
    }

    fn window_activation_changed(&self, active: bool, cx: &mut App) {
        if active {
            // GPUI reports WM_ACTIVATE, which Windows also sends when it
            // activates the window inside its own thread but denies the
            // foreground. Only a real foreground stops the raise retries.
            if !is_prism_foreground() {
                win_key::debug_trace("activate-without-foreground");
                return;
            }
            PALETTE_TOOK_FOREGROUND.store(true, Ordering::Release);
            // Windows ignores z-order requests from a process without
            // foreground rights; an open without them could leave the palette
            // under other windows. Now that Prism holds the foreground, take
            // the topmost band (the presentation always does).
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd(),
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            ACTIVATION_FOCUS_PENDING.store(false, Ordering::Release);
            return;
        }
        if !self.visible() || drag::is_dragging() || is_prism_foreground() {
            return;
        }
        if ACTIVATION_FOCUS_PENDING.swap(false, Ordering::AcqRel) {
            win_key::debug_trace("palette-blur-in-grace");
            return;
        }
        self.dismiss_on_blur("palette-hide-blur", cx);
    }
}

impl GpuiShell {
    /// Clicking away dismisses the launcher; the window stays up for the hide
    /// delay so the view plays the same exit as a Win-key close.
    fn dismiss_on_blur(&self, trace: &str, cx: &mut App) {
        if !self.visible() || drag::is_dragging() || is_prism_foreground() || !PALETTE_OPEN.load(Ordering::Acquire) {
            return;
        }
        let transition = begin_palette_close();
        win_key::debug_trace(trace);
        bridge::emit_event(BackendEvent::Toggle { open: false }, cx);
        self.schedule_palette_hide(transition, cx);
    }
}
