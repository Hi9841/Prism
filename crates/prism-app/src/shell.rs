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
const PALETTE_HIDE_DELAY: Duration = Duration::ZERO;
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
/// When this open first held the foreground (`now_ms`); 0 while it has not.
static FOREGROUND_AT_MS: AtomicU64 = AtomicU64::new(0);
/// Explorer bridge requests posted in this open.
static EXPLORER_REQUESTS: AtomicU32 = AtomicU32::new(0);
/// After the last raise retry: an open still without the foreground here is
/// the "opened but takes no keys" state, and the focus log records it.
const FOCUS_VERDICT_MS: u64 = 800;
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
    /// The Settings value. Default on. `raise` always asks for the topmost band
    /// so a denied foreground does not drop the request. This flag puts the
    /// window back when the setting is off.
    always_on_top: Cell<bool>,
}

impl GpuiShell {
    pub fn new(palette_visible: Arc<AtomicBool>) -> Self {
        Self { hwnd: Cell::new(0), palette_visible, always_on_top: Cell::new(true) }
    }

    fn apply_topmost(&self) -> Result<(), String> {
        let band = if self.always_on_top.get() { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            SetWindowPos(
                self.hwnd(),
                Some(band),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        }
        .map_err(|error| error.to_string())
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
                    mark_took_foreground();
                    return;
                }
                let _ = palette_window::raise(HWND(hwnd as *mut _));
                if is_prism_foreground() {
                    mark_took_foreground();
                    return;
                }
                // Prism's own request is refused in the background. Explorer's
                // first answer can come too late; ask it again.
                ask_explorer_for_foreground(hwnd);
            });
            self.keep(task);
        }
        let verdict = cx.spawn(async move |cx: &mut AsyncApp| {
            Timer::after(Duration::from_millis(FOCUS_VERDICT_MS)).await;
            if !PALETTE_OPEN.load(Ordering::Acquire) || PALETTE_TRANSITION.load(Ordering::Acquire) != transition {
                return;
            }
            if is_prism_foreground() {
                mark_took_foreground();
            }
            let at = FOREGROUND_AT_MS.load(Ordering::Acquire);
            let open = prism_core::focus_log::OpenFocus {
                foreground_after_ms: (at != 0).then(|| at.saturating_sub(LAST_OPEN_MS.load(Ordering::Acquire))),
                explorer_requests: EXPLORER_REQUESTS.load(Ordering::Acquire),
                explorer_granted: win_key::foreground_ack(),
            };
            let owner = prism_core::focus_log::foreground_owner();
            cx.background_spawn(async move {
                prism_core::focus_log::record(open, owner, env!("PRISM_VERSION"));
            })
            .detach();
        });
        self.keep(verdict);
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
            FOREGROUND_AT_MS.store(0, Ordering::Release);
            EXPLORER_REQUESTS.store(0, Ordering::Release);
            backend().schedule_installed_apps_refresh();
            self.arm_focus_grace(transition, cx);
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
        if PALETTE_OPEN.load(Ordering::Acquire) {
            // The open path already armed a timer. A second launch must arm
            // one too, or the grace flag stays set and the first click-away
            // does not close the palette.
            self.arm_focus_grace(PALETTE_TRANSITION.load(Ordering::Acquire), cx);
        } else {
            self.toggle_palette(cx);
        }
        let _ = self.present(cx);
    }

    /// Windows can report the palette as unfocused once while it is still
    /// acquiring foreground. The flag and the timer are one state.
    fn arm_focus_grace(&self, transition: u64, cx: &mut App) {
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
        if !self.always_on_top.get() {
            let _ = self.apply_topmost();
        }
        if !is_prism_foreground() {
            ask_explorer_for_foreground(self.hwnd.get());
            win_key::debug_trace(&format!("explorer-fg self={}", is_prism_foreground()));
        }
        if is_prism_foreground() {
            mark_took_foreground();
        }
        self.schedule_palette_raise_retry(PALETTE_TRANSITION.load(Ordering::Acquire), cx);
        log_foreground("present-after");
        prism_core::perf::finish(timer, "palette_present", || "window=main".to_string());
    }
}

fn mark_took_foreground() {
    if !PALETTE_TOOK_FOREGROUND.swap(true, Ordering::AcqRel) {
        FOREGROUND_AT_MS.store(now_ms(), Ordering::Release);
    }
}

/// Explorer holds the foreground permission of the Win key only until the
/// next input. Under load it can handle the request after the user's first
/// typed key went to the old window, and Windows then refuses it.
fn ask_explorer_for_foreground(hwnd: isize) {
    if win_key::request_foreground(hwnd) {
        EXPLORER_REQUESTS.fetch_add(1, Ordering::AcqRel);
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
        self.always_on_top.set(on);
        self.apply_topmost()
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
            mark_took_foreground();
            // Windows ignores z-order requests from a process without
            // foreground rights. Apply the stored band now that Prism holds
            // the foreground. `raise` asked for the topmost band first.
            let _ = self.apply_topmost();
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
