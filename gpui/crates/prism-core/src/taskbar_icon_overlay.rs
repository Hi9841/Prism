//! Persists the selected Start icon for the Explorer-thread renderer.
//!
//! The actual paint window lives in Prism's already-loaded Explorer shell hook
//! so it is composed in the same taskbar process and band as the native glyph.

use std::path::{Path, PathBuf};

use image::RgbaImage;

const ICON_FILE_NAME: &str = "taskbar-start-icon.rgba";
const ICON_MAGIC: &[u8] = b"PRISICON1";

pub struct OverlayIcon {
    pub pixels: RgbaImage,
}

pub fn init(initial: Option<OverlayIcon>) {
    let _ = sync_file(initial.as_ref());
    crate::win_key::notify_start_icon_changed();
}

pub fn set(icon: Option<OverlayIcon>) -> Result<(), String> {
    sync_file(icon.as_ref())?;
    crate::win_key::notify_start_icon_changed();
    Ok(())
}

fn sync_file(icon: Option<&OverlayIcon>) -> Result<(), String> {
    let path = icon_path()?;
    match icon {
        Some(icon) => write_icon_file(&path, &icon.pixels),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("remove taskbar icon file: {error}")),
        },
    }
}

fn write_icon_file(path: &Path, pixels: &RgbaImage) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "taskbar icon path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create taskbar icon directory: {error}"))?;
    let raw_pixels = pixels.as_raw();
    let mut bytes = Vec::with_capacity(ICON_MAGIC.len() + 8 + raw_pixels.len());
    bytes.extend_from_slice(ICON_MAGIC);
    bytes.extend_from_slice(&pixels.width().to_le_bytes());
    bytes.extend_from_slice(&pixels.height().to_le_bytes());
    bytes.extend_from_slice(raw_pixels);
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes).map_err(|error| format!("write taskbar icon file: {error}"))?;
    crate::files::replace_file(&temp, path)
}

fn icon_path() -> Result<PathBuf, String> {
    crate::host::app_data_dir()
        .map(|path| path.join(ICON_FILE_NAME))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_file_header_is_stable() {
        assert_eq!(ICON_MAGIC, b"PRISICON1");
    }
}
