//! The seam between the backend and the app that hosts the palette window.
//!
//! The app installs one `Host` at startup. Backend modules use it for the few
//! things only the app can do: deliver events to the UI, name the data folder,
//! report the palette window state, toggle the palette from the Win-key pump,
//! and register global shortcuts.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use windows::Win32::Foundation::{POINT, RECT};

/// What the backend tells the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreEvent {
    /// The file index changed (at most one each watcher flush).
    FileIndexUpdated,
    /// A rescan found new or changed apps.
    AppsUpdated,
    /// The Windows app theme changed.
    SystemThemeChanged { light: bool },
    /// Win-key mode stopped; the text is the reason for the user.
    WinModeFailed(String),
}

pub trait Host: Send + Sync + 'static {
    /// Delivers an event to the UI. Must be callable from any thread.
    fn emit(&self, event: CoreEvent);
    /// The per-user data folder (`%APPDATA%\app.prism.launcher`).
    fn app_data_dir(&self) -> Result<PathBuf, String>;
    /// True when the palette window is shown.
    fn palette_visible(&self) -> bool;
    /// Win key pressed. Called from the Win-key pump thread.
    fn toggle_from_win(&self, start_button: Option<RECT>);
    /// Start button clicked. Called from the Win-key pump thread.
    fn toggle_from_taskbar(&self, click: POINT, start_button: Option<RECT>);
    /// Explorer asked for search. Called from the Win-key pump thread.
    fn activate(&self);
    /// Registers a global shortcut that toggles the palette (not "Win").
    fn register_shortcut(&self, combo: &str) -> Result<(), String>;
    /// Releases a shortcut that `register_shortcut` installed.
    fn unregister_shortcut(&self, combo: &str);
}

static HOST: OnceLock<Arc<dyn Host>> = OnceLock::new();

/// Installs the host. The first call wins; later calls are ignored.
pub fn install(host: Arc<dyn Host>) {
    let _ = HOST.set(host);
}

pub fn get() -> Option<&'static Arc<dyn Host>> {
    HOST.get()
}

pub fn emit(event: CoreEvent) {
    if let Some(host) = HOST.get() {
        host.emit(event);
    }
}

pub fn app_data_dir() -> Result<PathBuf, String> {
    HOST.get()
        .ok_or_else(|| "the app host is not installed".to_string())?
        .app_data_dir()
}

/// `%APPDATA%\app.prism.launcher`: the data folder of the installed app (the
/// Tauri app's identifier). The GPUI host returns it, and the Explorer bridge
/// DLL reads its files there, so both use this one path.
pub fn installed_data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join(crate::shell_protocol::DATA_DIR_NAME))
}

/// The data folder, or the working directory when the host cannot name one.
pub fn app_data_dir_or_cwd() -> PathBuf {
    app_data_dir().unwrap_or_else(|_| PathBuf::from("."))
}
