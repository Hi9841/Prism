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
pub mod files;
pub mod host;
pub mod palette_window;
pub mod perf;
pub mod power;
pub mod query;
pub mod runtime;
pub mod settings_index;
pub mod start_menu;
pub mod taskbar;
pub mod taskbar_alignment;
pub mod taskbar_customization;
pub mod taskbar_icon_overlay;
pub mod theme;
pub mod win_key;
pub mod windows_settings;
pub mod windows_tools;
