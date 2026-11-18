//! The persisted settings (`prism.json`): the values each setting can take,
//! the list limits, and the version. The backend validates each save
//! (`prism_core::settings`); the UI cleans each load (`prism-ui`
//! `state::app::sanitize_settings`).

pub const STATE_VERSION: u32 = 3;
/// Version 2 files load: the 0.3.3 installer contract moves their shortcut to `WIN_SHORTCUT`.
pub const LEGACY_STATE_VERSION: u32 = 2;

/// The Win-key mode: Prism observes the Win key itself, with no hotkey registration.
pub const WIN_SHORTCUT: &str = "Win";
/// The only accepted global shortcuts. Bare typing keys, reserved keys and
/// known system or security combos are never accepted. The first is the default.
pub const SHORTCUTS: [&str; 8] = [
    WIN_SHORTCUT,
    "Ctrl+Alt+Space",
    "Ctrl+Alt+S",
    "Ctrl+Alt+Shift+S",
    "Ctrl+Alt+P",
    "Ctrl+Alt+Shift+P",
    "Ctrl+Alt+Enter",
    "Ctrl+Alt+Shift+Enter",
];

/// Each list below starts with its default value.
pub const ACCENTS: [&str; 5] = ["iris", "azure", "mint", "amber", "rose"];
pub const THEMES: [&str; 3] = ["system", "dark", "light"];
pub const TASKBAR_ALIGNMENTS: [&str; 3] = ["center", "left", "right"];
/// Window widths in logical pixels; the window animates between them.
pub const WIDTHS: [u32; 3] = [560, 640, 720];
pub const DEFAULT_WIDTH: u32 = 640;
/// View zoom in percent.
pub const VIEW_ZOOM_LEVELS: [u32; 9] = [70, 80, 90, 100, 110, 120, 130, 140, 150];
pub const DEFAULT_VIEW_ZOOM: u32 = 100;

pub const QUICK_ACCESS_KINDS: [&str; 7] = ["home", "desktop", "downloads", "documents", "pictures", "music", "videos"];
pub const QUICK_ACCESS_LIMIT: usize = 6;
/// The idle sections, in their default order.
pub const SECTION_IDS: [&str; 4] = ["pinned", "recent", "quick", "apps"];
pub const PINNED_APP_LIMIT: usize = 64;
pub const APP_GROUP_LIMIT: usize = 16;
pub const APP_GROUP_APP_LIMIT: usize = 64;
pub const APP_GROUP_ID_MAX: usize = 96;
pub const APP_GROUP_NAME_MAX: usize = 64;
/// The longest app id, history id or path id.
pub const ID_MAX: usize = 4096;
pub const HISTORY_LIMIT: usize = 20;
pub const HISTORY_TITLE_MAX: usize = 512;

/// A width between the smallest and the largest of `WIDTHS`: the native
/// window takes each step of the width animation.
pub fn is_window_width(width: u32) -> bool {
    (WIDTHS[0]..=WIDTHS[WIDTHS.len() - 1]).contains(&width)
}
