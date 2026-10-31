//! The `prism-core` host for the GPUI app: backend calls arrive on any thread
//! and are forwarded to the UI thread through a channel.

use std::path::PathBuf;

use futures::channel::mpsc::UnboundedSender;
use windows::Win32::Foundation::{POINT, RECT};

/// Work for the UI thread.
pub enum HostMessage {
    Emit { event: String, payload: serde_json::Value },
    ToggleFromWin(Option<RECT>),
    ToggleFromTaskbar(POINT, Option<RECT>),
    /// The configured (non-Win) shortcut was pressed.
    ToggleShortcut,
    Activate,
}

pub struct GpuiHost {
    pub sender: UnboundedSender<HostMessage>,
    pub app_data_dir: PathBuf,
    pub hotkeys: crate::hotkey::Hotkeys,
    pub palette_visible: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl prism_core::host::Host for GpuiHost {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.sender.unbounded_send(HostMessage::Emit { event: event.to_owned(), payload });
    }

    fn app_data_dir(&self) -> Result<PathBuf, String> {
        Ok(self.app_data_dir.clone())
    }

    fn palette_visible(&self) -> bool {
        self.palette_visible.load(std::sync::atomic::Ordering::Acquire)
    }

    fn toggle_from_win(&self, start_button: Option<RECT>) {
        let _ = self.sender.unbounded_send(HostMessage::ToggleFromWin(start_button));
    }

    fn toggle_from_taskbar(&self, click: POINT, start_button: Option<RECT>) {
        let _ = self.sender.unbounded_send(HostMessage::ToggleFromTaskbar(click, start_button));
    }

    fn activate(&self) {
        let _ = self.sender.unbounded_send(HostMessage::Activate);
    }

    fn register_shortcut(&self, combo: &str) -> Result<(), String> {
        self.hotkeys.register(combo)
    }

    fn unregister_shortcut(&self, combo: &str) {
        self.hotkeys.unregister(combo);
    }
}

/// `%APPDATA%\app.prism.launcher`, the Tauri app's data folder (same identifier).
pub fn app_data_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("app.prism.launcher")
}
