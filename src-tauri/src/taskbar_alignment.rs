//! Aligns the Windows taskbar's Start and application-button group.
//!
//! Windows 11 persists left/center alignment in `TaskbarAl`. StartAllBack's
//! classic taskbar also exposes the Start button and task list as child HWNDs,
//! which lets Prism provide a right-aligned mode without restarting Explorer.

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    RedrawWindow, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetWindowRect, IsWindowVisible,
    SendMessageTimeoutW, SetWindowPos, SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOREDRAW, SWP_NOSIZE,
    SWP_NOZORDER, WM_SETTINGCHANGE,
};

const EXPLORER_ADVANCED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const TASKBAR_ALIGNMENT_VALUE: &str = "TaskbarAl";
const ALIGNMENT_MARKER_FILE: &str = "taskbar-alignment";
const ALIGNMENT_UNSET: u8 = u8::MAX;
const ALIGNMENT_WATCH_INTERVAL: Duration = Duration::from_millis(50);

static ACTIVE_ALIGNMENT: AtomicU8 = AtomicU8::new(ALIGNMENT_UNSET);
static ALIGNMENT_WATCHER: OnceLock<()> = OnceLock::new();
static ALIGNMENT_APPLY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Alignment {
    Left,
    Center,
    Right,
}

impl Alignment {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "left" => Ok(Self::Left),
            "center" => Ok(Self::Center),
            "right" => Ok(Self::Right),
            _ => Err(format!("unsupported taskbar alignment '{value}'")),
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Center => 1,
            Self::Right => 2,
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }

    fn from_code(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Left),
            1 => Some(Self::Center),
            2 => Some(Self::Right),
            _ => None,
        }
    }

    fn windows_value(self) -> u32 {
        match self {
            Self::Left => 0,
            // Windows has no right-aligned value. Center is the least
            // surprising fallback while Prism reapplies StartAllBack's HWNDs.
            Self::Center | Self::Right => 1,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CompanionMove {
    pub window: HWND,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy)]
struct WindowMove {
    window: HWND,
    x: i32,
    y: i32,
}

#[derive(Default)]
struct TaskbarChildren {
    start: Option<HWND>,
    task_list: Option<HWND>,
    notification_area: Option<HWND>,
    xaml_content_host: Option<HWND>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Bounds {
    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }
}

impl From<RECT> for Bounds {
    fn from(value: RECT) -> Self {
        Self {
            left: value.left,
            top: value.top,
            right: value.right,
            bottom: value.bottom,
        }
    }
}

pub fn set(value: &str) -> Result<(), String> {
    set_alignment(Alignment::parse(value)?, None)
}

pub(crate) fn set_with_companion(
    alignment: Alignment,
    companion: CompanionMove,
) -> Result<(), String> {
    set_alignment(alignment, Some(companion))
}

/// Reasserts the active alignment while reopening the palette. This path does
/// not rewrite Explorer settings; it only repairs the live HWND geometry when
/// Explorer/StartAllBack has relaid out the taskbar since the last selection.
pub(crate) fn reapply_with_companion(companion: CompanionMove) -> Result<(), String> {
    let _apply_guard = ALIGNMENT_APPLY_LOCK
        .lock()
        .map_err(|error| format!("lock taskbar alignment: {error}"))?;
    let alignment = shared_alignment().unwrap_or_else(current);
    if classic_taskbar_count() > 0 {
        apply_classic_taskbars(alignment, Some(companion))?;
    } else {
        // Native Windows does not expose movable task-list HWNDs, but Prism's
        // companion window still belongs in this same position transaction.
        apply_window_moves(&[WindowMove {
            window: companion.window,
            x: companion.x,
            y: companion.y,
        }])?;
    }
    start_alignment_watcher();
    Ok(())
}

fn set_alignment(alignment: Alignment, companion: Option<CompanionMove>) -> Result<(), String> {
    let _apply_guard = ALIGNMENT_APPLY_LOCK
        .lock()
        .map_err(|error| format!("lock taskbar alignment: {error}"))?;
    let taskbars = taskbar_windows();
    let classic_taskbars = taskbars
        .iter()
        .filter(|taskbar| classic_children(**taskbar).is_some())
        .count();
    let native_taskbars = taskbars.len().saturating_sub(classic_taskbars);
    if alignment == Alignment::Right && (classic_taskbars == 0 || native_taskbars > 0) {
        return Err(
            "Right alignment requires the StartAllBack classic taskbar; Windows supports Left and Center only"
                .to_string(),
        );
    }

    if classic_taskbars > 0 {
        // StartAllBack exposes movable Start/task-list HWNDs. Moving them
        // directly avoids its delayed settings reload racing Prism's window.
        apply_classic_taskbars(alignment, companion)?;
    } else {
        if let Some(companion) = companion {
            apply_window_moves(&[WindowMove {
                window: companion.window,
                x: companion.x,
                y: companion.y,
            }])?;
        }
    }
    if native_taskbars > 0 || taskbars.is_empty() {
        // Native Windows supports left/center through TaskbarAl. When Explorer
        // is not ready yet, persist now and let it consume the value at boot.
        // SendMessageTimeout does not return until the XAML Start button has
        // accepted its new alignment, so the companion move remains coupled.
        write_windows_alignment(alignment.windows_value())?;
        notify_taskbars("TraySettings");
    }
    let _ = write_shared_alignment(alignment);
    ACTIVE_ALIGNMENT.store(alignment.code(), Ordering::Release);
    start_alignment_watcher();
    Ok(())
}

pub(crate) fn current() -> Alignment {
    Alignment::from_code(ACTIVE_ALIGNMENT.load(Ordering::Acquire)).unwrap_or(Alignment::Center)
}

fn start_alignment_watcher() {
    ALIGNMENT_WATCHER.get_or_init(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(ALIGNMENT_WATCH_INTERVAL);
            let Ok(_apply_guard) = ALIGNMENT_APPLY_LOCK.lock() else {
                continue;
            };
            let alignment = shared_alignment()
                .or_else(|| Alignment::from_code(ACTIVE_ALIGNMENT.load(Ordering::Acquire)));
            let Some(alignment) = alignment else { continue };
            if alignment.code() != ACTIVE_ALIGNMENT.load(Ordering::Acquire) {
                ACTIVE_ALIGNMENT.store(alignment.code(), Ordering::Release);
            }
            let _ = apply_classic_taskbars(alignment, None);
        });
    });
}

fn shared_alignment_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|dir| dir.join("app.prism.launcher").join(ALIGNMENT_MARKER_FILE))
}

fn write_shared_alignment(alignment: Alignment) -> Result<(), String> {
    let Some(path) = shared_alignment_path() else {
        return Ok(());
    };
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|error| format!("create alignment state: {error}"))?;
    std::fs::write(&path, alignment.marker())
        .map_err(|error| format!("write alignment state: {error}"))
}

fn shared_alignment() -> Option<Alignment> {
    let path = shared_alignment_path()?;
    let value = std::fs::read_to_string(path).ok()?;
    Alignment::parse(value.trim()).ok()
}

fn write_windows_alignment(value: u32) -> Result<(), String> {
    let key_path = wide(EXPLORER_ADVANCED_KEY);
    let mut key = HKEY::default();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut key,
        )
    };
    if opened.0 != 0 || key.0.is_null() {
        return Err(format!(
            "open Windows taskbar settings: Win32 error {}",
            opened.0
        ));
    }
    let key_guard = RegistryKey(key);
    let value_name = wide(TASKBAR_ALIGNMENT_VALUE);
    let written = unsafe {
        RegSetValueExW(
            key_guard.0,
            PCWSTR(value_name.as_ptr()),
            None,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        )
    };
    if written.0 == 0 {
        Ok(())
    } else {
        Err(format!(
            "write Windows taskbar alignment: Win32 error {}",
            written.0
        ))
    }
}

fn classic_taskbar_count() -> usize {
    taskbar_windows()
        .into_iter()
        .filter(|taskbar| classic_children(*taskbar).is_some())
        .count()
}

fn apply_classic_taskbars(
    alignment: Alignment,
    companion: Option<CompanionMove>,
) -> Result<(), String> {
    let mut moves = Vec::new();
    for taskbar in taskbar_windows() {
        let Some(children) = classic_children(taskbar) else {
            continue;
        };
        unsafe {
            moves.extend(classic_taskbar_moves(taskbar, children, alignment));
        }
    }
    if let Some(companion) = companion {
        moves.push(WindowMove {
            window: companion.window,
            x: companion.x,
            y: companion.y,
        });
    }
    apply_window_moves(&moves)
}

fn taskbar_windows() -> Vec<HWND> {
    let mut windows = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect_taskbar_window),
            LPARAM((&mut windows as *mut Vec<HWND>) as isize),
        );
    }
    windows
}

unsafe extern "system" fn collect_taskbar_window(window: HWND, detail: LPARAM) -> BOOL {
    let class_name = window_class(window);
    if class_name_eq(&class_name, "Shell_TrayWnd")
        || class_name_eq(&class_name, "Shell_SecondaryTrayWnd")
    {
        (*(detail.0 as *mut Vec<HWND>)).push(window);
    }
    BOOL(1)
}

fn classic_children(taskbar: HWND) -> Option<TaskbarChildren> {
    let mut children = TaskbarChildren::default();
    unsafe {
        let _ = EnumChildWindows(
            Some(taskbar),
            Some(collect_taskbar_child),
            LPARAM((&mut children as *mut TaskbarChildren) as isize),
        );
    }
    let (start, task_list) = children.start.zip(children.task_list)?;
    let controls_are_visible =
        unsafe { IsWindowVisible(start).as_bool() && IsWindowVisible(task_list).as_bool() };
    if !controls_are_visible {
        return None;
    }
    let xaml_owns_taskbar = children
        .xaml_content_host
        .is_some_and(|host| unsafe { visible_host_covers_taskbar(taskbar, host) });
    (!xaml_owns_taskbar).then_some(children)
}

unsafe extern "system" fn collect_taskbar_child(window: HWND, detail: LPARAM) -> BOOL {
    let children = &mut *(detail.0 as *mut TaskbarChildren);
    let class_name = window_class(window);
    if class_name_eq(&class_name, "Start") || class_name_eq(&class_name, "StartButton") {
        children.start = Some(window);
    } else if class_name_eq(&class_name, "ReBarWindow32") {
        children.task_list = Some(window);
    } else if class_name_eq(&class_name, "TrayNotifyWnd") {
        children.notification_area = Some(window);
    } else if class_name_eq(
        &class_name,
        "Windows.UI.Composition.DesktopWindowContentBridge",
    ) {
        children.xaml_content_host = Some(window);
    }
    BOOL(1)
}

unsafe fn visible_host_covers_taskbar(taskbar: HWND, host: HWND) -> bool {
    if !IsWindowVisible(host).as_bool() {
        return false;
    }
    let (Some(taskbar), Some(host)) = (window_bounds(taskbar), window_bounds(host)) else {
        return false;
    };
    bounds_cover(host, taskbar)
}

fn bounds_cover(outer: Bounds, inner: Bounds) -> bool {
    const EDGE_TOLERANCE: i32 = 2;
    outer.left <= inner.left + EDGE_TOLERANCE
        && outer.top <= inner.top + EDGE_TOLERANCE
        && outer.right >= inner.right - EDGE_TOLERANCE
        && outer.bottom >= inner.bottom - EDGE_TOLERANCE
}

unsafe fn classic_taskbar_moves(
    taskbar: HWND,
    children: TaskbarChildren,
    alignment: Alignment,
) -> Vec<WindowMove> {
    let (Some(start), Some(task_list)) = (children.start, children.task_list) else {
        return Vec::new();
    };
    let (Some(taskbar_bounds), Some(start_bounds), Some(task_list_bounds)) = (
        window_bounds(taskbar),
        window_bounds(start),
        window_bounds(task_list),
    ) else {
        return Vec::new();
    };
    let notification_bounds = children
        .notification_area
        .and_then(|window| window_bounds(window));
    let horizontal = taskbar_bounds.width() >= taskbar_bounds.height();
    let compact_cluster = if horizontal {
        Bounds {
            left: taskbar_bounds.left,
            top: taskbar_bounds.top,
            right: taskbar_bounds.left + start_bounds.width() + task_list_bounds.width(),
            bottom: taskbar_bounds.bottom,
        }
    } else {
        Bounds {
            left: taskbar_bounds.left,
            top: taskbar_bounds.top,
            right: taskbar_bounds.right,
            bottom: taskbar_bounds.top + start_bounds.height() + task_list_bounds.height(),
        }
    };
    let (delta_x, delta_y) = alignment_delta(
        taskbar_bounds,
        compact_cluster,
        notification_bounds,
        alignment,
    );
    let cluster_left = taskbar_bounds.left + delta_x;
    let cluster_top = taskbar_bounds.top + delta_y;
    let positions = if horizontal {
        [
            (start, cluster_left, start_bounds.top),
            (
                task_list,
                cluster_left + start_bounds.width(),
                task_list_bounds.top,
            ),
        ]
    } else {
        [
            (start, start_bounds.left, cluster_top),
            (
                task_list,
                task_list_bounds.left,
                cluster_top + start_bounds.height(),
            ),
        ]
    };

    [(start, start_bounds), (task_list, task_list_bounds)]
        .into_iter()
        .zip(positions)
        .filter_map(|((window, bounds), (_, target_x, target_y))| {
            (bounds.left != target_x || bounds.top != target_y).then_some(WindowMove {
                window,
                x: target_x - taskbar_bounds.left,
                y: target_y - taskbar_bounds.top,
            })
        })
        .collect()
}

fn apply_window_moves(moves: &[WindowMove]) -> Result<(), String> {
    if moves.is_empty() {
        return Ok(());
    }
    for movement in moves {
        unsafe {
            SetWindowPos(
                movement.window,
                None,
                movement.x,
                movement.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
            )
        }
        .map_err(|error| format!("position taskbar alignment window: {error}"))?;
    }
    // The windows may belong to Explorer or Prism, so the Win32 defer batch
    // cannot reliably include both process-owned HWNDs. Suppress painting
    // while committing every final position, then invalidate the completed
    // geometry so the user sees one settled layout instead of a staged move.
    for movement in moves {
        let redrawn = unsafe {
            RedrawWindow(
                Some(movement.window),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW,
            )
        };
        if !redrawn.as_bool() {
            return Err("redraw taskbar alignment window failed".to_string());
        }
    }
    Ok(())
}

fn alignment_delta(
    taskbar: Bounds,
    cluster: Bounds,
    notification_area: Option<Bounds>,
    alignment: Alignment,
) -> (i32, i32) {
    if taskbar.width() >= taskbar.height() {
        let available_right = notification_area
            .filter(|bounds| bounds.left > taskbar.left)
            .map(|bounds| bounds.left)
            .unwrap_or(taskbar.right);
        let latest_start = (available_right - cluster.width()).max(taskbar.left);
        let target = match alignment {
            Alignment::Left => taskbar.left,
            Alignment::Center => taskbar.left + (taskbar.width() - cluster.width()) / 2,
            Alignment::Right => latest_start,
        }
        .clamp(taskbar.left, latest_start);
        (target - cluster.left, 0)
    } else {
        let available_bottom = notification_area
            .filter(|bounds| bounds.top > taskbar.top)
            .map(|bounds| bounds.top)
            .unwrap_or(taskbar.bottom);
        let latest_start = (available_bottom - cluster.height()).max(taskbar.top);
        let target = match alignment {
            Alignment::Left => taskbar.top,
            Alignment::Center => taskbar.top + (taskbar.height() - cluster.height()) / 2,
            Alignment::Right => latest_start,
        }
        .clamp(taskbar.top, latest_start);
        (0, target - cluster.top)
    }
}

unsafe fn window_bounds(window: HWND) -> Option<Bounds> {
    let mut rect = RECT::default();
    GetWindowRect(window, &mut rect)
        .is_ok()
        .then_some(Bounds::from(rect))
        .filter(|bounds| bounds.width() > 0 && bounds.height() > 0)
}

unsafe fn window_class(window: HWND) -> Vec<u16> {
    let mut class_name = [0u16; 64];
    let length = GetClassNameW(window, &mut class_name).max(0) as usize;
    class_name[..length].to_vec()
}

fn class_name_eq(actual: &[u16], expected: &str) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected.bytes())
            .all(|(actual, expected)| (*actual as u8).eq_ignore_ascii_case(&expected))
}

fn notify_taskbars(setting: &str) {
    let setting = wide(setting);
    for taskbar in taskbar_windows() {
        let mut result = 0usize;
        unsafe {
            let _ = SendMessageTimeoutW(
                taskbar,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(setting.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                500,
                Some(&mut result),
            );
        }
    }
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
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

    #[test]
    fn horizontal_alignment_uses_full_center_and_stays_before_notification_area() {
        let taskbar = Bounds {
            left: 0,
            top: 1_032,
            right: 1_920,
            bottom: 1_080,
        };
        let cluster = Bounds {
            left: 805,
            top: 1_032,
            right: 982,
            bottom: 1_080,
        };
        let notification = Some(Bounds {
            left: 1_681,
            top: 1_032,
            right: 1_920,
            bottom: 1_080,
        });

        assert_eq!(
            alignment_delta(taskbar, cluster, notification, Alignment::Left),
            (-805, 0)
        );
        assert_eq!(
            alignment_delta(taskbar, cluster, notification, Alignment::Center),
            (66, 0)
        );
        assert_eq!(
            alignment_delta(taskbar, cluster, notification, Alignment::Right),
            (699, 0)
        );
    }

    #[test]
    fn vertical_alignment_maps_left_center_right_to_top_center_bottom() {
        let taskbar = Bounds {
            left: 0,
            top: 0,
            right: 48,
            bottom: 1_080,
        };
        let cluster = Bounds {
            left: 0,
            top: 300,
            right: 48,
            bottom: 500,
        };
        let notification = Some(Bounds {
            left: 0,
            top: 900,
            right: 48,
            bottom: 1_080,
        });

        assert_eq!(
            alignment_delta(taskbar, cluster, notification, Alignment::Left),
            (0, -300)
        );
        assert_eq!(
            alignment_delta(taskbar, cluster, notification, Alignment::Center),
            (0, 140)
        );
        assert_eq!(
            alignment_delta(taskbar, cluster, notification, Alignment::Right),
            (0, 400)
        );
    }

    #[test]
    fn full_width_xaml_host_owns_native_taskbar_rendering() {
        let taskbar = Bounds {
            left: 0,
            top: 1_032,
            right: 1_920,
            bottom: 1_080,
        };
        assert!(bounds_cover(taskbar, taskbar));
        assert!(bounds_cover(
            Bounds {
                left: 0,
                top: 1_030,
                right: 1_920,
                bottom: 1_080,
            },
            taskbar,
        ));
        assert!(!bounds_cover(
            Bounds {
                left: 850,
                top: 1_032,
                right: 1_070,
                bottom: 1_080,
            },
            taskbar,
        ));
    }
}
