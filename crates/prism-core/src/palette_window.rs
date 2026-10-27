//! Palette window placement and foreground handling, on a raw HWND so the
//! Tauri app and the GPUI app share one implementation.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, BringWindowToTop, GetCursorPos, GetForegroundWindow,
    GetWindowThreadProcessId, LockSetForegroundWindow, SetForegroundWindow, SetWindowPos,
    HWND_TOPMOST, LSFW_UNLOCK, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
};

use crate::taskbar_alignment::Alignment;

/// Preferred palette dimensions in logical (100% DPI) pixels. They stay
/// fixed regardless of the monitor the palette lands on; the physical
/// window size is derived per presentation from the target monitor's
/// scale factor and work area.
pub const DEFAULT_LOGICAL_WIDTH: u32 = 640;
pub const DEFAULT_LOGICAL_HEIGHT: u32 = 620;
pub const MIN_LOGICAL_WIDTH: u32 = 480;
pub const MIN_LOGICAL_HEIGHT: u32 = 400;

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PresentationSource {
    WinKey,
    TaskbarStartClick,
    ConfiguredShortcut,
    FocusLoss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskbarEdge {
    Bottom,
    Top,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationAnchor {
    pub start_button: Option<PhysicalRect>,
    pub click_point: Option<PhysicalPoint>,
    pub taskbar_edge: Option<TaskbarEdge>,
    pub monitor: Option<PhysicalRect>,
    pub work_area: Option<PhysicalRect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_factor: Option<f64>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationEvent {
    pub open: bool,
    pub source: PresentationSource,
    pub anchor: Option<PresentationAnchor>,
    pub generation: u64,
}

/// Where the palette goes and how large it is, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteFrame {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// The palette frame against the Start button's taskbar edge, clamped to the
/// physical-pixel work area. Keyboard and shortcut fallback placement is
/// bottom-centered on the active monitor. The caller resizes the window.
pub fn palette_frame(
    hwnd: HWND,
    anchor: Option<PresentationAnchor>,
    alignment: Alignment,
    logical_width: u32,
) -> Result<PaletteFrame, String> {
    let info = anchor
        .and_then(|value| {
            let monitor = value.monitor?;
            let work = value.work_area?;
            let scale = value
                .scale_factor
                .unwrap_or_else(|| window_scale_factor(hwnd));
            Some((monitor, work, scale))
        })
        .or_else(|| monitor_geometry_for_window(hwnd));
    let (monitor, work, scale_factor) =
        info.ok_or_else(|| "cannot resolve monitor geometry".to_string())?;
    // Recalculate the physical size from the preferred logical dimensions
    // and the target monitor's DPI every time: presenting on a smaller or
    // denser display must not permanently shrink the palette, and moving
    // back to a larger display restores the preferred size.
    let (width, height) =
        calculate_clamped_window_size(logical_width, DEFAULT_LOGICAL_HEIGHT, work, scale_factor);
    let edge = anchor
        .and_then(|value| value.taskbar_edge)
        .or_else(|| taskbar_edge(monitor, work))
        .unwrap_or(TaskbarEdge::Bottom);
    let (x, y) = palette_position(work, edge, alignment, width, height);
    Ok(PaletteFrame {
        x,
        y,
        width,
        height,
    })
}

pub fn presentation_anchor(
    start_button: Option<RECT>,
    click_point: Option<POINT>,
) -> PresentationAnchor {
    let start_button = start_button.map(PhysicalRect::from);
    let click_point = click_point.map(PhysicalPoint::from);
    let monitor_point = click_point.or_else(|| start_button.map(PhysicalRect::center));
    let geometry = monitor_point.and_then(monitor_geometry_for_point);
    PresentationAnchor {
        start_button,
        click_point,
        taskbar_edge: geometry.and_then(|(monitor, work, _)| taskbar_edge(monitor, work)),
        monitor: geometry.map(|(monitor, _, _)| monitor),
        work_area: geometry.map(|(_, work, _)| work),
        scale_factor: geometry.map(|(_, _, scale)| scale),
    }
}

/// Preferred logical size converted to physical pixels for the target
/// monitor's scale factor, clamped to the work area and the configured
/// minimum window bounds.
pub fn calculate_clamped_window_size(
    logical_width: u32,
    logical_height: u32,
    work_area: PhysicalRect,
    scale_factor: f64,
) -> (i32, i32) {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let desired_w = (logical_width as f64 * scale).round() as i32;
    let desired_h = (logical_height as f64 * scale).round() as i32;
    let work_w = work_area.width().max(1);
    let work_h = work_area.height().max(1);
    // The configured logical minimum applies only while it still fits.
    let min_w = ((MIN_LOGICAL_WIDTH as f64 * scale).round() as i32).clamp(1, work_w);
    let min_h = ((MIN_LOGICAL_HEIGHT as f64 * scale).round() as i32).clamp(1, work_h);
    (
        desired_w.clamp(min_w, work_w),
        desired_h.clamp(min_h, work_h),
    )
}

fn monitor_scale_factor(monitor: HMONITOR) -> f64 {
    let mut dpi_x = 0;
    let mut dpi_y = 0;
    if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_ok()
        && dpi_x > 0
    {
        dpi_x as f64 / 96.0
    } else {
        1.0
    }
}

pub fn window_scale_factor(window: HWND) -> f64 {
    let dpi = unsafe { GetDpiForWindow(window) };
    if dpi > 0 {
        dpi as f64 / 96.0
    } else {
        1.0
    }
}

fn monitor_geometry_for_point(point: PhysicalPoint) -> Option<(PhysicalRect, PhysicalRect, f64)> {
    let monitor = unsafe {
        MonitorFromPoint(
            POINT {
                x: point.x,
                y: point.y,
            },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    monitor_geometry(monitor)
}

fn monitor_geometry_for_window(window: HWND) -> Option<(PhysicalRect, PhysicalRect, f64)> {
    let mut point = POINT::default();
    let monitor = unsafe {
        if GetCursorPos(&mut point).is_ok() {
            MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST)
        }
    };
    monitor_geometry(monitor)
}

fn monitor_geometry(monitor: HMONITOR) -> Option<(PhysicalRect, PhysicalRect, f64)> {
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info).as_bool() } {
        Some((
            PhysicalRect::from(info.rcMonitor),
            PhysicalRect::from(info.rcWork),
            monitor_scale_factor(monitor),
        ))
    } else {
        None
    }
}

fn taskbar_edge(monitor: PhysicalRect, work: PhysicalRect) -> Option<TaskbarEdge> {
    let candidates = [
        (work.top - monitor.top, TaskbarEdge::Top),
        (monitor.bottom - work.bottom, TaskbarEdge::Bottom),
        (work.left - monitor.left, TaskbarEdge::Left),
        (monitor.right - work.right, TaskbarEdge::Right),
    ];
    candidates
        .into_iter()
        .filter(|(inset, _)| *inset > 0)
        .max_by_key(|(inset, _)| *inset)
        .map(|(_, edge)| edge)
}

pub fn palette_position(
    work: PhysicalRect,
    edge: TaskbarEdge,
    alignment: Alignment,
    width: i32,
    height: i32,
) -> (i32, i32) {
    let aligned_x = match alignment {
        Alignment::Left => work.left,
        Alignment::Center => work.left + (work.width() - width) / 2,
        Alignment::Right => work.right - width,
    };
    let aligned_y = match alignment {
        Alignment::Left => work.top,
        Alignment::Center => work.top + (work.height() - height) / 2,
        Alignment::Right => work.bottom - height,
    };
    let (x, y) = match edge {
        TaskbarEdge::Bottom => (aligned_x, work.bottom - height),
        TaskbarEdge::Top => (aligned_x, work.top),
        TaskbarEdge::Left => (work.left, aligned_y),
        TaskbarEdge::Right => (work.right - width, aligned_y),
    };
    // A window larger than the work area still clamps inside it instead of
    // panicking on inverted bounds.
    let max_x = (work.right - width).max(work.left);
    let max_y = (work.bottom - height).max(work.top);
    (x.clamp(work.left, max_x), y.clamp(work.top, max_y))
}

impl PhysicalRect {
    pub fn width(self) -> i32 {
        self.right - self.left
    }

    pub fn height(self) -> i32 {
        self.bottom - self.top
    }

    pub fn center(self) -> PhysicalPoint {
        PhysicalPoint {
            x: self.left + self.width() / 2,
            y: self.top + self.height() / 2,
        }
    }
}

impl From<RECT> for PhysicalRect {
    fn from(rect: RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

impl From<POINT> for PhysicalPoint {
    fn from(point: POINT) -> Self {
        Self {
            x: point.x,
            y: point.y,
        }
    }
}

/// True when the foreground window belongs to this Prism process. Compare the
/// owning process rather than a single HWND: a host can own child windows.
pub fn is_prism_foreground() -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(foreground, Some(&mut pid));
        pid != 0 && pid == GetCurrentProcessId()
    }
}

/// Ask Windows for keyboard focus. Deliberately does not use
/// `AttachThreadInput`: attaching to an elevated foreground thread is blocked
/// by UIPI (and can deadlock the UI thread), so it silently failed whenever
/// Task Manager or another elevated window held focus. Unlocking the
/// foreground lock and raising the window works across integrity levels.
pub fn force_foreground(hwnd: HWND) {
    unsafe {
        if is_prism_foreground() {
            return;
        }
        let _ = LockSetForegroundWindow(LSFW_UNLOCK);
        let _ = AllowSetForegroundWindow(GetCurrentProcessId());
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetActiveWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
}

/// Reasserts Prism at the front of the topmost band and asks for foreground.
pub fn raise(hwnd: HWND) -> Result<(), String> {
    unsafe {
        // Topmost first without activation: when the same call also asks to
        // activate and Windows refuses, it discards the topmost request too.
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        )
        .map_err(|error| error.to_string())?;
        force_foreground(hwnd);
        let foreground = GetForegroundWindow();
        crate::win_key::debug_trace(&format!(
            "raise fg_self={} fg={:p}",
            foreground == hwnd,
            foreground.0
        ));
    }
    Ok(())
}

/// Debug-only: record what currently owns the foreground and whether it is
/// topmost, so a failing toggle can be attributed to the right window.
#[cfg(debug_assertions)]
pub fn log_foreground(tag: &str) {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            crate::win_key::debug_trace(&format!("{tag} fg=null"));
            return;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(foreground, Some(&mut pid));
        let mut class = [0u16; 64];
        let length = windows::Win32::UI::WindowsAndMessaging::GetClassNameW(foreground, &mut class);
        let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
        let ex_style = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
            foreground,
            windows::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
        ) as u32;
        crate::win_key::debug_trace(&format!(
            "{tag} fg_pid={pid} self={} topmost={} class={class}",
            pid == GetCurrentProcessId(),
            ex_style & 0x8 != 0
        ));
    }
}

#[cfg(not(debug_assertions))]
pub fn log_foreground(_tag: &str) {}
