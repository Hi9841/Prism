//! The seam between the backend and the app that hosts the palette window.
//!
//! The app installs one `Host` at startup. Backend modules use it for the few
//! things only the app can do: deliver events to the UI, report the palette
//! window state, toggle the palette from the Win-key pump,
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

/// Prism's data folder (`shell_protocol::data_dir`), or the working directory
/// without `%APPDATA%`. The Explorer bridge DLL reads its files there too.
pub fn data_dir() -> PathBuf {
    crate::shell_protocol::data_dir().unwrap_or_else(|| PathBuf::from("."))
}
