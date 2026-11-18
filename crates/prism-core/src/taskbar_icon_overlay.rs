//! Persists the selected Start icon for the Explorer-thread renderer.
//!
//! The actual paint window lives in Prism's already-loaded Explorer shell hook
//! so it is composed in the same taskbar process and band as the native glyph.

use std::path::{Path, PathBuf};

use image::RgbaImage;

use crate::shell_protocol::{ICON_MAGIC, START_ICON_FILE};

pub struct OverlayIcon {
    pub pixels: RgbaImage,
}

pub fn init(initial: Option<OverlayIcon>) {
    let _ = sync_file(initial.as_ref());
    crate::shell_bridge::notify_start_icon_changed();
}

pub fn set(icon: Option<OverlayIcon>) -> Result<(), String> {
    sync_file(icon.as_ref())?;
    crate::shell_bridge::notify_start_icon_changed();
    Ok(())
}

fn sync_file(icon: Option<&OverlayIcon>) -> Result<(), String> {
    let path = icon_path();
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
    let raw_pixels = pixels.as_raw();
    let mut bytes = Vec::with_capacity(ICON_MAGIC.len() + 8 + raw_pixels.len());
    bytes.extend_from_slice(ICON_MAGIC);
    bytes.extend_from_slice(&pixels.width().to_le_bytes());
    bytes.extend_from_slice(&pixels.height().to_le_bytes());
    bytes.extend_from_slice(raw_pixels);
    crate::win32::write_atomic(path, &bytes).map_err(|error| format!("write taskbar icon file: {error}"))
}

fn icon_path() -> PathBuf {
    crate::host::data_dir().join(START_ICON_FILE)
}
