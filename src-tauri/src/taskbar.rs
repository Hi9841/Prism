//! Presents the Windows taskbar alongside Prism whenever the palette opens.

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABM_ACTIVATE, ABM_GETSTATE, ABM_GETTASKBARPOS, ABS_AUTOHIDE, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect,
    IsWindowVisible, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOP,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
    WS_EX_TOPMOST,
};

/// Persistent marker proving Prism presented the taskbar over a fullscreen
/// window. If Prism dies while the palette is open, the next launch reads the
/// marker and restores the taskbar instead of leaving it stuck above every
/// window.
const TOPMOST_MARKER: &str = "taskbar-topmost";

/// Marker contents recording the band the taskbar wore before Prism's lease:
/// `topmost` (Windows 11 and StartAllBack defaults) or `normal`. Empty legacy
/// markers read as the normal band.
const MARKER_TOPMOST: &[u8] = b"topmost";
const MARKER_NORMAL: &[u8] = b"normal";

/// Tracks a temporary taskbar z-order lease for the current process. This is
/// deliberately presentation-based rather than based on the taskbar's
/// initial `WS_EX_TOPMOST` bit: Explorer commonly starts with that bit set,
/// but Prism still needs to demote the taskbar when its fullscreen presentation
/// ends.
static PRESENTED: AtomicBool = AtomicBool::new(false);

/// The taskbar's z-band when Prism started presenting it. `release()` restores
/// exactly this band instead of unconditionally demoting: Windows 11 and
/// StartAllBack both normally keep the taskbar topmost, and forcing
/// HWND_NOTOPMOST on exit left a taskbar that hid behind maximized windows -
/// or flickered while StartAllBack re-asserted it - with Prism not running.
static PRESENTED_WAS_TOPMOST: AtomicBool = AtomicBool::new(false);

/// Fullscreen windows must not be covered by the taskbar; a few pixels of
/// slack avoid classifying maximized windows as fullscreen.
const FULLSCREEN_TOLERANCE: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TaskbarPresentation {
    show: bool,
    topmost: bool,
}

fn taskbar_presentation(fullscreen: bool, auto_hide: bool) -> TaskbarPresentation {
    TaskbarPresentation {
        show: true,
        topmost: fullscreen || auto_hide,
    }
}

fn rect_covers_monitor(window: RECT, monitor: RECT, tolerance: i32) -> bool {
    window.left <= monitor.left + tolerance
        && window.top <= monitor.top + tolerance
        && window.right >= monitor.right - tolerance
        && window.bottom >= monitor.bottom - tolerance
}

/// Primary taskbar HWND, if Explorer has created it.
pub fn tray_present() -> bool {
    taskbar_window().is_some()
}

/// Visible primary and secondary taskbar rectangles. Windows 11's XAML
/// taskbar often leaves `rcWork` equal to the full monitor, so callers that
/// dock a window to the work area have to subtract these themselves.
pub fn bar_rects() -> Vec<RECT> {
    let mut rects = Vec::new();
    if let Some(rect) = appbar_taskbar_rect() {
        rects.push(rect);
    }
    unsafe {
        let _ = EnumWindows(
            Some(collect_taskbar_rect),
            LPARAM((&mut rects as *mut Vec<RECT>) as isize),
        );
    }
    rects
}

fn appbar_taskbar_rect() -> Option<RECT> {
    let hwnd = taskbar_window()?;
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        ..Default::default()
    };
    let found = unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut data) };
    if found == 0 {
        return None;
    }
    let rect = data.rc;
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

unsafe extern "system" fn collect_taskbar_rect(window: HWND, detail: LPARAM) -> BOOL {
    let mut class_name = [0u16; 64];
    let length = GetClassNameW(window, &mut class_name).max(0) as usize;
    let is_taskbar = class_name_is(&class_name[..length], "Shell_TrayWnd")
        || class_name_is(&class_name[..length], "Shell_SecondaryTrayWnd");
    if is_taskbar && IsWindowVisible(window).as_bool() {
        let mut rect = RECT::default();
        if GetWindowRect(window, &mut rect).is_ok()
            && rect.right > rect.left
            && rect.bottom > rect.top
        {
            (*(detail.0 as *mut Vec<RECT>)).push(rect);
        }
    }
    BOOL(1)
}

fn class_name_is(actual: &[u16], expected: &str) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected.bytes())
            .all(|(actual, expected)| (*actual as u8).eq_ignore_ascii_case(&expected))
}

pub fn present() {
    let Some(taskbar) = taskbar_window() else {
        return;
    };
    let presentation = taskbar_presentation(foreground_is_fullscreen(), taskbar_auto_hides());
    // Preserve ownership across duplicate presentation requests. The taskbar
    // may already be visible or topmost before Prism opens, but this call
    // still creates a temporary lease that must be released afterward.
    let was_topmost = window_band_is_topmost(taskbar);
    PRESENTED_WAS_TOPMOST.store(was_topmost, Ordering::Release);
    PRESENTED.store(true, Ordering::Release);
    unsafe {
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar,
            lParam: LPARAM(1),
            ..Default::default()
        };
        let _ = SHAppBarMessage(ABM_ACTIVATE, &mut appbar);
        let _ = ShowWindow(taskbar, SW_SHOWNOACTIVATE);
        let insert_after = if presentation.topmost {
            HWND_TOPMOST
        } else {
            HWND_TOP
        };
        let _ = SetWindowPos(
            taskbar,
            Some(insert_after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        // The marker records the band the taskbar had before this lease so a
        // crash mid-presentation can still restore it on the next launch.
        write_marker(was_topmost);
    }
}

/// True when the foreground window covers its entire monitor - a fullscreen
/// game or video player. Called while the palette is still hidden, so the
/// foreground window is the app the user came from.
fn foreground_is_fullscreen() -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(foreground, &mut rect).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        rect_covers_monitor(rect, info.rcMonitor, FULLSCREEN_TOLERANCE)
    }
}

fn taskbar_auto_hides() -> bool {
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        ..Default::default()
    };
    unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) as u32 & ABS_AUTOHIDE != 0 }
}

pub fn release() {
    let Some(taskbar) = taskbar_window() else {
        PRESENTED.store(false, Ordering::Release);
        clear_marker();
        return;
    };
    let fullscreen_foreground = foreground_is_fullscreen();
    unsafe {
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar,
            lParam: LPARAM(0),
            ..Default::default()
        };
        let _ = SHAppBarMessage(ABM_ACTIVATE, &mut appbar);
        // Restore the band the taskbar had before Prism presented it. The
        // lease covers the transition from whatever style the taskbar wore
        // beforehand - Windows 11 and StartAllBack normally run topmost, and
        // demoting those on exit left the taskbar broken once Prism closed.
        // While a borderless fullscreen app is foreground, a normal-band
        // taskbar still goes to the bottom so it never covers the game.
        let owned = PRESENTED.swap(false, Ordering::AcqRel);
        if owned || marker_present() {
            let recorded = if owned {
                Some(PRESENTED_WAS_TOPMOST.load(Ordering::Acquire))
            } else {
                marker_band()
            };
            let insert_after = match release_band(recorded, fullscreen_foreground) {
                ReleaseBand::Topmost => HWND_TOPMOST,
                ReleaseBand::Bottom => HWND_BOTTOM,
                ReleaseBand::NotTopmost => HWND_NOTOPMOST,
            };
            let _ = SetWindowPos(
                taskbar,
                Some(insert_after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        clear_marker();
    }
}

/// Startup repair: if a previous Prism instance crashed while the palette was
/// open, its marker is still on disk - restore the taskbar to the band the
/// marker recorded instead of leaving it stuck in Prism's presentation band.
pub fn recover() {
    let Some(recorded) = marker_band() else {
        return;
    };
    let Some(taskbar) = taskbar_window() else {
        clear_marker();
        return;
    };
    let insert_after = match release_band(Some(recorded), foreground_is_fullscreen()) {
        ReleaseBand::Topmost => HWND_TOPMOST,
        ReleaseBand::Bottom => HWND_BOTTOM,
        ReleaseBand::NotTopmost => HWND_NOTOPMOST,
    };
    unsafe {
        let _ = SetWindowPos(
            taskbar,
            Some(insert_after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    clear_marker();
}

/// One-shot taskbar repair for `prism.exe --repair-taskbar`: restores a
/// stranded z-band from the marker and nudges the shell to re-lay out the
/// taskbar. Runs without the full app so it works when Prism is "off".
pub fn repair() {
    recover();
    crate::taskbar_alignment::notify_taskbars("TraySettings");
}

/// The z-band the taskbar must be left in after Prism's presentation ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseBand {
    Topmost,
    Bottom,
    NotTopmost,
}

/// Exactly the band the taskbar wore before Prism presented it. A recorded
/// topmost taskbar (the Windows 11 and StartAllBack defaults) goes back to
/// topmost; a recorded normal-band taskbar stays demoted, pinned to the
/// bottom while a borderless fullscreen app is foreground so it never covers
/// the game. A missing record takes the safe plain demote.
fn release_band(recorded_topmost: Option<bool>, fullscreen_foreground: bool) -> ReleaseBand {
    match recorded_topmost {
        Some(true) => ReleaseBand::Topmost,
        Some(false) if fullscreen_foreground => ReleaseBand::Bottom,
        _ => ReleaseBand::NotTopmost,
    }
}

fn window_band_is_topmost(window: HWND) -> bool {
    unsafe { (GetWindowLongW(window, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0) != 0 }
}

fn marker_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|dir| dir.join("app.prism.launcher").join(TOPMOST_MARKER))
}

fn marker_present() -> bool {
    marker_path().is_some_and(|path| path.is_file())
}

/// The band recorded in the marker: `Some(true)` = topmost, `Some(false)` =
/// normal band. An empty legacy marker reads as the normal band.
fn marker_band() -> Option<bool> {
    let path = marker_path()?;
    if !path.is_file() {
        return None;
    }
    Some(std::fs::read(&path).map(|content| content == MARKER_TOPMOST).unwrap_or(false))
}

fn write_marker(topmost: bool) {
    let Some(path) = marker_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &path,
        if topmost { MARKER_TOPMOST } else { MARKER_NORMAL },
    );
}

fn clear_marker() {
    let Some(path) = marker_path() else {
        return;
    };
    let _ = std::fs::remove_file(&path);
}

fn taskbar_window() -> Option<windows::Win32::Foundation::HWND> {
    let class = wide("Shell_TrayWnd");
    unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()).ok() }
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn palette_open_always_shows_the_taskbar() {
        assert!(taskbar_presentation(false, false).show);
        assert!(taskbar_presentation(true, false).show);
        assert!(taskbar_presentation(false, true).show);
        assert!(taskbar_presentation(true, true).show);
    }

    #[test]
    fn taskbar_is_topmost_only_over_fullscreen_or_auto_hide() {
        assert!(!taskbar_presentation(false, false).topmost);
        assert!(taskbar_presentation(true, false).topmost);
        assert!(taskbar_presentation(false, true).topmost);
        assert!(taskbar_presentation(true, true).topmost);
    }

    #[test]
    fn maximized_work_area_is_not_fullscreen() {
        let monitor = rect(0, 0, 1920, 1080);
        let maximized = rect(0, 0, 1920, 1040);
        assert!(!rect_covers_monitor(
            maximized,
            monitor,
            FULLSCREEN_TOLERANCE
        ));
    }

    #[test]
    fn borderless_cover_is_fullscreen() {
        let monitor = rect(0, 0, 1920, 1080);
        let cover = rect(-2, -2, 1922, 1082);
        assert!(rect_covers_monitor(cover, monitor, FULLSCREEN_TOLERANCE));
    }

    #[test]
    fn a_recorded_topmost_taskbar_is_restored_topmost() {
        // Windows 11 and StartAllBack run the taskbar topmost by default;
        // Prism must put it back there, never leave it demoted behind apps.
        assert_eq!(release_band(Some(true), false), ReleaseBand::Topmost);
        assert_eq!(release_band(Some(true), true), ReleaseBand::Topmost);
    }

    #[test]
    fn a_recorded_normal_taskbar_stays_demoted() {
        assert_eq!(release_band(Some(false), false), ReleaseBand::NotTopmost);
        // A normal-band taskbar must never cover a borderless fullscreen game.
        assert_eq!(release_band(Some(false), true), ReleaseBand::Bottom);
    }

    #[test]
    fn a_missing_record_takes_the_safe_demote() {
        // No record means no lease to unwind; demote to the normal band and
        // never risk pushing the taskbar to the bottom behind a fullscreen app
        // without knowing which band it came from.
        assert_eq!(release_band(None, false), ReleaseBand::NotTopmost);
        assert_eq!(release_band(None, true), ReleaseBand::NotTopmost);
    }
}
