//! The contract between Prism and the bridge DLL inside Explorer: the window
//! message, its codes, the observer window class, and the files both sides
//! read. Std only: the DLL has no dependencies. The DLL and `prism-core` (`crate::shell_protocol`) compile this one
//! file, so a change here reaches both sides.

/// The registered window message that carries every code below (`wParam`).
pub const BRIDGE_MESSAGE_NAME: &str = "Prism.ShellBridge.v1";
/// The class of Prism's raw-input observer window: the DLL posts events to it.
pub const OBSERVER_CLASS_NAME: &str = "PrismRawKeyboardObserver";

pub const CONTROL_DISABLE_WIN_HOTKEY: usize = 1;
pub const EVENT_HOTKEY_DISABLED: usize = 2;
pub const CONTROL_START_RECT_LEFT: usize = 4;
pub const CONTROL_START_RECT_TOP: usize = 5;
pub const CONTROL_START_RECT_RIGHT: usize = 6;
pub const CONTROL_START_RECT_BOTTOM: usize = 7;
pub const EVENT_START_RECT_CONFIGURED: usize = 8;
pub const EVENT_TASKBAR_START_CLICK_X: usize = 9;
pub const EVENT_TASKBAR_START_CLICK_Y: usize = 10;
pub const CONTROL_START_ICON_REFRESH: usize = 11;
pub const CONTROL_START_ICON_SHUTDOWN: usize = 12;
pub const EVENT_START_ICON_SHUTDOWN: usize = 13;
pub const EVENT_START_ICON_REFRESHED: usize = 14;
pub const CONTROL_SEARCH_RECT_LEFT: usize = 15;
pub const CONTROL_SEARCH_RECT_TOP: usize = 16;
pub const CONTROL_SEARCH_RECT_RIGHT: usize = 17;
pub const CONTROL_SEARCH_RECT_BOTTOM: usize = 18;
pub const EVENT_SEARCH_RECT_CONFIGURED: usize = 19;
pub const CONTROL_TASKBAR_PIN: usize = 20;
pub const CONTROL_TASKBAR_UNPIN: usize = 21;
pub const EVENT_TASKBAR_PIN_COMPLETED: usize = 22;
pub const CONTROL_FOREGROUND_WINDOW: usize = 23;
pub const EVENT_FOREGROUND_RESULT: usize = 24;
pub const EVENT_SHELL_START_COMMAND: usize = 25;
pub const EVENT_SHELL_SEARCH_COMMAND: usize = 26;
pub const EVENT_TASKBAR_POINTER: usize = 27;

/// Prism's data folder under `%APPDATA%` (the Tauri app's identifier). The
/// pin request file uses `%LOCALAPPDATA%` first, then `%APPDATA%`.
pub const DATA_DIR_NAME: &str = "app.prism.launcher";
/// The custom Start icon: `ICON_MAGIC`, width and height (u32 LE), RGBA pixels.
pub const START_ICON_FILE: &str = "taskbar-start-icon.rgba";
pub const ICON_MAGIC: &[u8] = b"PRISICON1";
/// The path to pin or unpin, written before `CONTROL_TASKBAR_PIN` / `_UNPIN`.
pub const PIN_TARGET_FILE: &str = "taskbar-pin-target.txt";

/// Where Prism writes the pin target and the DLL reads it: the data folder
/// under `%LOCALAPPDATA%` (else `%APPDATA%`), else `%TEMP%\Prism`.
pub fn pin_target_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA"));
    match base {
        Some(base) => Some(std::path::PathBuf::from(base).join(DATA_DIR_NAME).join(PIN_TARGET_FILE)),
        None => std::env::var_os("TEMP").map(|temp| std::path::PathBuf::from(temp).join("Prism").join(PIN_TARGET_FILE)),
    }
}
