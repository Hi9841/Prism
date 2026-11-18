//! Prism backend: app catalog, file index, Phase 1 query, power, audio,
//! taskbar, and the Win-key bridge. No UI framework: the app that hosts the
//! palette window installs a [`host::Host`] at startup.

pub mod apps;
pub mod audio;
pub mod audio_hook;
pub mod audio_osd;
pub mod backend;
pub mod catalog;
pub mod drag;
pub mod focus_log;
pub mod logs;
pub mod ui_stall;
pub mod host;
pub mod palette_window;
pub mod perf;
pub mod power;
pub mod query;
pub mod runtime;
pub mod settings;
pub mod settings_index;
pub mod shell_bridge;
pub(crate) mod shell_icon;
#[path = "../shell-hook/src/protocol.rs"]
pub(crate) mod shell_protocol;
pub mod start_menu;
pub mod taskbar;
pub mod taskbar_alignment;
pub mod taskbar_customization;
pub mod taskbar_icon_overlay;
pub mod theme;
pub mod win_key;
pub mod windows_settings;
pub mod windows_tools;
pub(crate) mod win32;

/// FNV-1a, 64 bits: a stable short name for a string or a file (not a security check).
pub(crate) fn fnv1a_64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}
