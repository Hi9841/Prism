//! The values that `prism-core` (the backend) and `prism-ui` (including its
//! pure part, which the judge adapter builds alone) must agree on. Std only.

pub mod settings;

/// The rank of an app's `source` when two entries are the same app: the
/// curated shortcut stores first, the raw install folders last. Inline: the
/// UI search calls it for each app on each keystroke, from another crate.
#[inline]
pub fn source_rank(source: &str) -> i32 {
    match source {
        "taskbar" => 6,
        "startMenu" => 5,
        "desktop" => 4,
        "appsFolder" => 3,
        "registry" | "appPaths" | "applications" => 2,
        "programs" => 1,
        _ => 0,
    }
}

/// The `source` of a Phase 1 hit that lists with the Windows tools.
pub const WINDOWS_TOOL_SOURCE: &str = "windows-tool";

/// The lowercase extensions that "Run as administrator" accepts.
pub const ELEVATABLE_EXTENSIONS: [&str; 8] = ["exe", "com", "bat", "cmd", "ps1", "vbs", "js", "wsf"];

/// The lowercase extensions that get a file thumbnail.
pub const THUMBNAIL_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "bmp", "webp"];

/// The error text of a taskbar pin that Windows asks the user to confirm.
pub const MANUAL_PIN_REQUIRED: &str = "prism-manual-pin-required";
