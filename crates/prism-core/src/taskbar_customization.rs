//! Reads and applies user-facing Windows taskbar preferences.
//!
//! Registry-backed choices are limited to current-user Explorer settings.
//! Start glyphs are rendered by Prism's own native overlay; this module never
//! reads from or writes to third-party shell-provider settings.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use image::codecs::png::PngDecoder;
use image::imageops::{overlay, FilterType};
use image::{DynamicImage, ImageDecoder, ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::LPARAM;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SAM_FLAGS,
};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABM_GETSTATE, ABM_SETSTATE, ABS_ALWAYSONTOP, ABS_AUTOHIDE, APPBARDATA,
};
use crate::win32::{registry, WriteError};

const EXPLORER_ADVANCED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const TASKBAR_SIZE_VALUE: &str = "TaskbarSi";
const ICON_SIZE_PREFERENCE_VALUE: &str = "IconSizePreference";
const COMBINE_VALUE: &str = "TaskbarGlomLevel";
const SECONDARY_COMBINE_VALUE: &str = "MMTaskbarGlomLevel";
const ICON_SETTINGS_NAME: &str = "taskbar-start-icon.json";
const CUSTOM_ICON_PNG: &str = "taskbar-start-icon.png";
const CUSTOM_ICON_DIR: &str = "taskbar-start-icons";
const MAX_ICON_BYTES: usize = 2 * 1024 * 1024;
const MAX_CUSTOM_ICONS: usize = 12;
const MAX_ICON_EDGE: u32 = 4_096;
const MAX_ICON_PIXELS: u64 = 16_777_216;
const PREVIEW_EDGE: u32 = 96;

const GEM_ICON: &[u8] = include_bytes!("../assets/taskbar-icons/gem.png");
const DIAMOND_ICON: &[u8] = include_bytes!("../assets/taskbar-icons/diamond.png");

/// The icon density choices of `set_thickness`.
pub const THICKNESSES: [&str; 3] = {
    let mut names = [""; 3];
    let mut index = 0;
    while index < names.len() {
        names[index] = Thickness::ALL[index].name();
        index += 1;
    }
    names
};
/// The button grouping choices of `set_combine_buttons`, in the order of their registry codes.
pub const COMBINE_BUTTONS: [&str; 3] = ["always", "whenFull", "never"];
/// The built-in Start icons of `set_start_icon`; `"custom"` selects a user image.
pub const BUILTIN_START_ICONS: [&str; 3] = {
    let mut names = [""; 3];
    let mut index = 0;
    while index < names.len() {
        names[index] = StartIcon::BUILTIN[index].name();
        index += 1;
    }
    names
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Thickness {
    Compact,
    Default,
    Adaptive,
}

impl Thickness {
    const ALL: [Self; 3] = [Self::Compact, Self::Default, Self::Adaptive];

    const fn name(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Default => "default",
            Self::Adaptive => "adaptive",
        }
    }

    /// The `TaskbarSi` (legacy size) and `IconSizePreference` values.
    const fn registry_values(self) -> (u32, u32) {
        match self {
            Self::Compact => (0, 0),
            Self::Default => (1, 1),
            Self::Adaptive => (1, 2),
        }
    }

    fn from_registry(legacy_value: u32, icon_preference: u32) -> Self {
        if icon_preference == 0 || legacy_value == 0 {
            Self::Compact
        } else if icon_preference == 2 {
            Self::Adaptive
        } else {
            Self::Default
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum StartIcon {
    #[default]
    System,
    Gem,
    Diamond,
    Custom,
}

impl StartIcon {
    const BUILTIN: [Self; 3] = [Self::System, Self::Gem, Self::Diamond];

    fn parse(value: &str) -> Result<Self, String> {
        Self::BUILTIN
            .into_iter()
            .chain([Self::Custom])
            .find(|icon| icon.name() == value)
            .ok_or_else(|| format!("unsupported Start icon '{value}'"))
    }

    const fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Gem => "gem",
            Self::Diamond => "diamond",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct IconSettings {
    version: u32,
    start_icon: StartIcon,
    #[serde(default)]
    selected_custom_icon: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CustomStartIcon {
    id: String,
    /// Base64 PNG preview. Serialized as a string so a dozen previews cost
    /// kilobytes of IPC instead of megabytes of JSON number arrays.
    preview: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskbarSettings {
    thickness: &'static str,
    auto_hide: bool,
    combine_buttons: &'static str,
    start_icon: &'static str,
    selected_custom_icon: Option<String>,
    custom_start_icons: Vec<CustomStartIcon>,
}

pub fn init() {
    let settings = read_icon_settings().unwrap_or_default();
    let initial = overlay_icon(&settings).ok().flatten();
    crate::taskbar_icon_overlay::init(initial);
}

pub fn settings() -> Result<TaskbarSettings, String> {
    let icon_settings = read_icon_settings()?;
    let advanced = open_key(EXPLORER_ADVANCED_KEY, KEY_QUERY_VALUE)?;
    let read_advanced = |name, fallback| {
        advanced
            .as_ref()
            .and_then(|key| read_dword(key.0, name).ok().flatten())
            .unwrap_or(fallback)
    };
    Ok(TaskbarSettings {
        thickness: Thickness::from_registry(
            read_advanced(TASKBAR_SIZE_VALUE, 1),
            read_advanced(ICON_SIZE_PREFERENCE_VALUE, 1),
        )
        .name(),
        auto_hide: taskbar_auto_hide(),
        combine_buttons: combine_name(read_advanced(COMBINE_VALUE, 0)),
        start_icon: icon_settings.start_icon.name(),
        selected_custom_icon: selected_custom_id(&icon_settings),
        custom_start_icons: custom_start_icons()?,
    })
}

pub fn set_thickness(value: &str) -> Result<(), String> {
    let (legacy_size, icon_preference) = Thickness::ALL
        .into_iter()
        .find(|thickness| thickness.name() == value)
        .ok_or_else(|| format!("unsupported taskbar thickness '{value}'"))?
        .registry_values();
    write_advanced_dword(TASKBAR_SIZE_VALUE, legacy_size)?;
    write_advanced_dword(ICON_SIZE_PREFERENCE_VALUE, icon_preference)?;
    crate::taskbar_alignment::notify_taskbars("TraySettings");
    Ok(())
}

pub fn set_auto_hide(enabled: bool) -> Result<(), String> {
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        ..Default::default()
    };
    let current = unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) } as u32;
    let state = if enabled {
        current | ABS_ALWAYSONTOP | ABS_AUTOHIDE
    } else {
        (current | ABS_ALWAYSONTOP) & !ABS_AUTOHIDE
    };
    data.lParam = LPARAM(state as isize);
    unsafe {
        SHAppBarMessage(ABM_SETSTATE, &mut data);
    }
    if taskbar_auto_hide() == enabled {
        Ok(())
    } else {
        Err("Windows did not apply the taskbar auto-hide setting".to_string())
    }
}

pub fn set_combine_buttons(value: &str) -> Result<(), String> {
    let code = COMBINE_BUTTONS
        .iter()
        .position(|&known| known == value)
        .ok_or_else(|| format!("unsupported taskbar grouping '{value}'"))? as u32;
    write_advanced_dword(COMBINE_VALUE, code)?;
    write_advanced_dword(SECONDARY_COMBINE_VALUE, code)?;
    crate::taskbar_alignment::notify_taskbars("TraySettings");
    Ok(())
}

pub fn set_start_icon(value: &str) -> Result<(), String> {
    let selected = StartIcon::parse(value)?;
    let mut settings = read_icon_settings()?;
    settings.start_icon = selected;
    let icon = overlay_icon(&settings)?;
    write_icon_settings(&settings)?;
    crate::taskbar_icon_overlay::set(icon)
}

pub fn set_custom_start_icon(base64_png: &str) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_png)
        .map_err(|_| "The icon data was corrupted in transit".to_string())?;
    let (preview, pixels) = prepare_icon(&bytes)?;
    let existing = custom_start_icons()?;
    if existing.len() >= MAX_CUSTOM_ICONS {
        return Err(format!(
            "Remove an icon before adding another (maximum {MAX_CUSTOM_ICONS})"
        ));
    }
    let id = new_custom_icon_id();
    write_customization_file(&custom_icon_path_for_id(&id)?, &preview)?;
    let settings = IconSettings {
        version: 1,
        start_icon: StartIcon::Custom,
        selected_custom_icon: Some(id),
    };
    write_icon_settings(&settings)?;
    crate::taskbar_icon_overlay::set(Some(crate::taskbar_icon_overlay::OverlayIcon { pixels }),
    )
}

pub fn select_custom_start_icon(id: &str) -> Result<(), String> {
    let path = custom_icon_path_for_id(id)?;
    let bytes = std::fs::read(path).map_err(|_| "That custom icon no longer exists".to_string())?;
    let pixels = decode_png(&bytes)?;
    let settings = IconSettings {
        version: 1,
        start_icon: StartIcon::Custom,
        selected_custom_icon: (id != "legacy").then(|| id.to_string()),
    };
    write_icon_settings(&settings)?;
    crate::taskbar_icon_overlay::set(Some(crate::taskbar_icon_overlay::OverlayIcon { pixels }),
    )
}

pub fn remove_custom_start_icon(id: &str) -> Result<(), String> {
    let mut settings = read_icon_settings()?;
    let selected = selected_custom_id(&settings).is_some_and(|selected| selected == id);
    let path = custom_icon_path_for_id(id)?;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("That custom icon no longer exists".to_string());
        }
        Err(error) => return Err(format!("remove custom Start icon: {error}")),
    }
    if selected {
        settings.start_icon = StartIcon::System;
        settings.selected_custom_icon = None;
        write_icon_settings(&settings)?;
        crate::taskbar_icon_overlay::set(None)?;
    }
    Ok(())
}

fn overlay_icon(settings: &IconSettings,
) -> Result<Option<crate::taskbar_icon_overlay::OverlayIcon>, String> {
    let bytes = match settings.start_icon {
        StartIcon::System => return Ok(None),
        StartIcon::Gem => GEM_ICON.to_vec(),
        StartIcon::Diamond => DIAMOND_ICON.to_vec(),
        StartIcon::Custom => std::fs::read(custom_icon_path_for_settings(settings)?)
            .map_err(|_| "Choose a custom PNG before selecting Custom".to_string())?,
    };
    let pixels = decode_png(&bytes)?;
    Ok(Some(crate::taskbar_icon_overlay::OverlayIcon { pixels }))
}

fn prepare_icon(bytes: &[u8]) -> Result<(Vec<u8>, RgbaImage), String> {
    let source = decode_png(bytes)?;
    let scale = (PREVIEW_EDGE as f32 / source.width().max(source.height()) as f32).min(1.0);
    let width = ((source.width() as f32 * scale).round() as u32).max(1);
    let height = ((source.height() as f32 * scale).round() as u32).max(1);
    let resized = image::imageops::resize(&source, width, height, FilterType::Lanczos3);
    let mut preview = RgbaImage::new(PREVIEW_EDGE, PREVIEW_EDGE);
    overlay(
        &mut preview,
        &resized,
        i64::from((PREVIEW_EDGE - width) / 2),
        i64::from((PREVIEW_EDGE - height) / 2),
    );
    let encoded = encode_png(&preview)?;
    Ok((encoded, preview))
}

fn decode_png(bytes: &[u8]) -> Result<RgbaImage, String> {
    if bytes.is_empty() || bytes.len() > MAX_ICON_BYTES {
        return Err("Choose a PNG smaller than 2 MB".to_string());
    }
    let decoder = PngDecoder::new(Cursor::new(bytes))
        .map_err(|_| "The selected file is not a valid PNG".to_string())?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || width > MAX_ICON_EDGE
        || height > MAX_ICON_EDGE
        || u64::from(width) * u64::from(height) > MAX_ICON_PIXELS
    {
        return Err("Choose a PNG no larger than 4096 x 4096 pixels".to_string());
    }
    DynamicImage::from_decoder(decoder)
        .map_err(|_| "The selected PNG could not be decoded".to_string())
        .map(|image| image.to_rgba8())
}

fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|error| format!("encode taskbar icon: {error}"))?;
    Ok(output.into_inner())
}

fn read_icon_settings() -> Result<IconSettings, String> {
    let path = icon_settings_path();
    if !path.exists() {
        return Ok(IconSettings::default());
    }
    let settings: IconSettings = serde_json::from_slice(
        &std::fs::read(path).map_err(|error| format!("read Start icon settings: {error}"))?,
    )
    .map_err(|error| format!("parse Start icon settings: {error}"))?;
    if settings.version == 1 {
        Ok(settings)
    } else {
        Ok(IconSettings::default())
    }
}

fn write_icon_settings(settings: &IconSettings) -> Result<(), String> {
    let bytes = serde_json::to_vec(&settings).map_err(|error| error.to_string())?;
    write_customization_file(&icon_settings_path(), &bytes)
}

fn selected_custom_id(settings: &IconSettings) -> Option<String> {
    if settings.start_icon != StartIcon::Custom {
        return None;
    }
    settings.selected_custom_icon.clone().or_else(|| {
        Some(custom_icon_path())
            .filter(|path| path.is_file())
            .map(|_| "legacy".to_string())
    })
}

fn custom_start_icons() -> Result<Vec<CustomStartIcon>, String> {
    let mut icons = Vec::new();
    let legacy = custom_icon_path();
    if let Ok(preview) = std::fs::read(legacy) {
        icons.push(CustomStartIcon {
            id: "legacy".to_string(),
            preview: encode_base64(&preview),
        });
    }
    let directory = custom_icon_dir();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(icons),
        Err(error) => return Err(format!("read custom Start icons: {error}")),
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let Some(id) = path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        if !valid_custom_icon_id(id) {
            continue;
        }
        if let Ok(preview) = std::fs::read(&path) {
            icons.push(CustomStartIcon {
                id: id.to_string(),
                preview: encode_base64(&preview),
            });
        }
    }
    Ok(icons)
}

fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn new_custom_icon_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{timestamp:032x}")
}

fn valid_custom_icon_id(id: &str) -> bool {
    id == "legacy" || (id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn taskbar_auto_hide() -> bool {
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        ..Default::default()
    };
    unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) as u32 & ABS_AUTOHIDE != 0 }
}

fn combine_name(value: u32) -> &'static str {
    COMBINE_BUTTONS.get(value as usize).copied().unwrap_or(COMBINE_BUTTONS[0])
}

fn write_advanced_dword(name: &str, value: u32) -> Result<(), String> {
    let key = open_key(EXPLORER_ADVANCED_KEY, KEY_SET_VALUE)?
        .ok_or_else(|| "Windows taskbar settings are unavailable".to_string())?;
    write_dword(key.0, name, value)
}

fn open_key(path: &str, access: REG_SAM_FLAGS) -> Result<Option<registry::Key>, String> {
    registry::Key::open(HKEY_CURRENT_USER, path, access)
        .map_err(|code| format!("open taskbar registry settings: Win32 error {code}"))
}

fn read_dword(key: HKEY, name: &str) -> Result<Option<u32>, String> {
    registry::dword(key, name).map_err(|error| {
        let code = match error {
            registry::DwordError::Win32(code) => code,
            registry::DwordError::NotDword => 0,
        };
        format!("read taskbar registry setting: Win32 error {code}")
    })
}

fn write_dword(key: HKEY, name: &str, value: u32) -> Result<(), String> {
    registry::set_dword(key, name, value).map_err(|code| format!("write taskbar registry setting: Win32 error {code}"))
}


fn write_customization_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::win32::write_atomic(path, bytes).map_err(|error| match error {
        WriteError::Folder(e) => format!("create taskbar customization directory: {e}"),
        WriteError::Create(e) | WriteError::Write(e) => format!("write taskbar customization file: {e}"),
        WriteError::Replace(e) => e,
    })
}

fn icon_settings_path() -> PathBuf {
    crate::host::data_dir().join(ICON_SETTINGS_NAME)
}

fn custom_icon_path() -> PathBuf {
    crate::host::data_dir().join(CUSTOM_ICON_PNG)
}

fn custom_icon_dir() -> PathBuf {
    crate::host::data_dir().join(CUSTOM_ICON_DIR)
}

fn custom_icon_path_for_settings(settings: &IconSettings,
) -> Result<PathBuf, String> {
    match settings.selected_custom_icon.as_deref() {
        Some(id) => custom_icon_path_for_id(id),
        None => Ok(custom_icon_path()),
    }
}

fn custom_icon_path_for_id(id: &str) -> Result<PathBuf, String> {
    if !valid_custom_icon_id(id) {
        return Err("Invalid custom icon identifier".to_string());
    }
    if id == "legacy" {
        Ok(custom_icon_path())
    } else {
        Ok(custom_icon_dir().join(format!("{id}.png")))
    }
}

