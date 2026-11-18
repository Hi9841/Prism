//! The `prism-core` host for the GPUI app: backend calls arrive on any thread
//! and are forwarded to the UI thread through a channel.


use futures::channel::mpsc::UnboundedSender;
use windows::Win32::Foundation::{POINT, RECT};

/// Work for the UI thread.
pub enum HostMessage {
    Emit(prism_core::host::CoreEvent),
    ToggleFromWin(Option<RECT>),
    ToggleFromTaskbar(POINT, Option<RECT>),
    /// The configured (non-Win) shortcut was pressed.
    ToggleShortcut,
    Activate,
}

pub struct GpuiHost {
    pub sender: UnboundedSender<HostMessage>,
    pub hotkeys: crate::hotkey::Hotkeys,
    pub palette_visible: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl prism_core::host::Host for GpuiHost {
    fn emit(&self, event: prism_core::host::CoreEvent) {
        let _ = self.sender.unbounded_send(HostMessage::Emit(event));
    }

    fn palette_visible(&self) -> bool {
        self.palette_visible.load(std::sync::atomic::Ordering::Acquire)
    }

    fn toggle_from_win(&self, start_button: Option<RECT>) {
        prism_core::ui_stall::input_sent("win");
        let _ = self.sender.unbounded_send(HostMessage::ToggleFromWin(start_button));
    }

    fn toggle_from_taskbar(&self, click: POINT, start_button: Option<RECT>) {
        prism_core::ui_stall::input_sent("taskbar");
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


