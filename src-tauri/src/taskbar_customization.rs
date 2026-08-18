//! Reads and applies user-facing Windows taskbar preferences.
//!
//! Registry-backed choices are limited to current-user Explorer settings.
//! Start glyphs are rendered by Prism's own native overlay; this module never
//! reads from or writes to third-party shell-provider settings.

use std::io::Cursor;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use image::codecs::png::PngDecoder;
use image::imageops::{overlay, FilterType};
use image::{DynamicImage, ImageDecoder, ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use windows::core::PCWSTR;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_SAM_FLAGS, REG_VALUE_TYPE,
};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABM_GETSTATE, ABM_SETSTATE, ABS_ALWAYSONTOP, ABS_AUTOHIDE, APPBARDATA,
};

const EXPLORER_ADVANCED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const SEARCH_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Search";
const TASKBAR_SIZE_VALUE: &str = "TaskbarSi";
const ICON_SIZE_PREFERENCE_VALUE: &str = "IconSizePreference";
const COMBINE_VALUE: &str = "TaskbarGlomLevel";
const SECONDARY_COMBINE_VALUE: &str = "MMTaskbarGlomLevel";
const TASK_VIEW_VALUE: &str = "ShowTaskViewButton";
const WIDGETS_VALUE: &str = "TaskbarDa";
const SEARCHBOX_MODE_VALUE: &str = "SearchboxMode";
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
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "system" => Ok(Self::System),
            "gem" => Ok(Self::Gem),
            "diamond" => Ok(Self::Diamond),
            "custom" => Ok(Self::Custom),
            _ => Err(format!("unsupported Start icon '{value}'")),
        }
    }

    fn name(self) -> &'static str {
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
pub(crate) struct CustomStartIcon {
    id: String,
    /// Base64 PNG preview. Serialized as a string so a dozen previews cost
    /// kilobytes of IPC instead of megabytes of JSON number arrays.
    preview: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskbarSettings {
    thickness: &'static str,
    auto_hide: bool,
    combine_buttons: &'static str,
    show_task_view: bool,
    show_widgets: bool,
    searchbox_mode: &'static str,
    start_icon: &'static str,
    selected_custom_icon: Option<String>,
    custom_start_icons: Vec<CustomStartIcon>,
}

pub(crate) fn init(app: AppHandle) {
    let settings = read_icon_settings(&app).unwrap_or_default();
    let initial = overlay_icon(&app, &settings).ok().flatten();
    crate::taskbar_icon_overlay::init(app, initial);
}

pub(crate) fn settings(app: &AppHandle) -> Result<TaskbarSettings, String> {
    let icon_settings = read_icon_settings(app)?;
    let advanced = open_key(EXPLORER_ADVANCED_KEY, KEY_QUERY_VALUE)?;
    let read_advanced = |name, fallback| {
        advanced
            .as_ref()
            .and_then(|key| read_dword(key.0, name).ok().flatten())
            .unwrap_or(fallback)
    };
    let search = open_key(SEARCH_KEY, KEY_QUERY_VALUE)?;
    let read_search = |name, fallback| {
        search
            .as_ref()
            .and_then(|key| read_dword(key.0, name).ok().flatten())
            .unwrap_or(fallback)
    };

    Ok(TaskbarSettings {
        thickness: thickness_name(
            read_advanced(TASKBAR_SIZE_VALUE, 1),
            read_advanced(ICON_SIZE_PREFERENCE_VALUE, 1),
        ),
        auto_hide: taskbar_auto_hide(),
        combine_buttons: combine_name(read_advanced(COMBINE_VALUE, 0)),
        show_task_view: read_advanced(TASK_VIEW_VALUE, 1) != 0,
        show_widgets: read_advanced(WIDGETS_VALUE, 1) != 0,
        searchbox_mode: searchbox_mode_name(read_search(SEARCHBOX_MODE_VALUE, 1)),
        start_icon: icon_settings.start_icon.name(),
        selected_custom_icon: selected_custom_id(app, &icon_settings),
        custom_start_icons: custom_start_icons(app)?,
    })
}

pub(crate) fn set_thickness(value: &str) -> Result<(), String> {
    let (legacy_size, icon_preference) = match value {
        "compact" => (0, 0),
        "default" => (1, 1),
        "adaptive" => (1, 2),
        _ => return Err(format!("unsupported taskbar thickness '{value}'")),
    };
    write_advanced_dword(TASKBAR_SIZE_VALUE, legacy_size)?;
    write_advanced_dword(ICON_SIZE_PREFERENCE_VALUE, icon_preference)?;
    crate::taskbar_alignment::notify_taskbars("TraySettings");
    Ok(())
}

pub(crate) fn set_auto_hide(enabled: bool) -> Result<(), String> {
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

pub(crate) fn set_combine_buttons(value: &str) -> Result<(), String> {
    let code = match value {
        "always" => 0,
        "whenFull" => 1,
        "never" => 2,
        _ => return Err(format!("unsupported taskbar grouping '{value}'")),
    };
    write_advanced_dword(COMBINE_VALUE, code)?;
    write_advanced_dword(SECONDARY_COMBINE_VALUE, code)?;
    crate::taskbar_alignment::notify_taskbars("TraySettings");
    Ok(())
}

pub(crate) fn set_task_view(visible: bool) -> Result<(), String> {
    write_advanced_dword(TASK_VIEW_VALUE, u32::from(visible))?;
    crate::taskbar_alignment::notify_taskbars("TraySettings");
    Ok(())
}

pub(crate) fn set_widgets(visible: bool) -> Result<(), String> {
    write_advanced_dword(WIDGETS_VALUE, u32::from(visible))?;
    crate::taskbar_alignment::notify_taskbars("TraySettings");
    Ok(())
}

pub(crate) fn set_searchbox_mode(value: &str) -> Result<(), String> {
    let code = match value {
        "hidden" => 0,
        "icon" | "button" => 1,
        "box" | "searchBox" => 2,
        "iconWithLabel" | "buttonWithLabel" => 3,
        _ => return Err(format!("unsupported searchbox mode '{value}'")),
    };
    let key = open_key(SEARCH_KEY, KEY_SET_VALUE)?
        .ok_or_else(|| "Windows search settings are unavailable".to_string())?;
    write_dword(key.0, SEARCHBOX_MODE_VALUE, code)?;
    crate::taskbar_alignment::notify_taskbars("TraySettings");
    crate::win_key::request_start_rect_refresh();
    Ok(())
}

pub(crate) fn set_start_icon(app: &AppHandle, value: &str) -> Result<(), String> {
    let selected = StartIcon::parse(value)?;
    let mut settings = read_icon_settings(app)?;
    settings.start_icon = selected;
    let icon = overlay_icon(app, &settings)?;
    write_icon_settings(app, &settings)?;
    crate::taskbar_icon_overlay::set(app, icon)
}

pub(crate) fn set_custom_start_icon(app: &AppHandle, base64_png: &str) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_png)
        .map_err(|_| "The icon data was corrupted in transit".to_string())?;
    let (preview, pixels) = prepare_icon(&bytes)?;
    let existing = custom_start_icons(app)?;
    if existing.len() >= MAX_CUSTOM_ICONS {
        return Err(format!(
            "Remove an icon before adding another (maximum {MAX_CUSTOM_ICONS})"
        ));
    }
    let id = new_custom_icon_id();
    write_file_atomically(&custom_icon_path_for_id(app, &id)?, &preview)?;
    let settings = IconSettings {
        version: 1,
        start_icon: StartIcon::Custom,
        selected_custom_icon: Some(id),
    };
    write_icon_settings(app, &settings)?;
    crate::taskbar_icon_overlay::set(
        app,
        Some(crate::taskbar_icon_overlay::OverlayIcon { pixels }),
    )
}

pub(crate) fn select_custom_start_icon(app: &AppHandle, id: &str) -> Result<(), String> {
    let path = custom_icon_path_for_id(app, id)?;
    let bytes = std::fs::read(path).map_err(|_| "That custom icon no longer exists".to_string())?;
    let pixels = decode_png(&bytes)?;
    let settings = IconSettings {
        version: 1,
        start_icon: StartIcon::Custom,
        selected_custom_icon: (id != "legacy").then(|| id.to_string()),
    };
    write_icon_settings(app, &settings)?;
    crate::taskbar_icon_overlay::set(
        app,
        Some(crate::taskbar_icon_overlay::OverlayIcon { pixels }),
    )
}

pub(crate) fn remove_custom_start_icon(app: &AppHandle, id: &str) -> Result<(), String> {
    let mut settings = read_icon_settings(app)?;
    let selected = selected_custom_id(app, &settings).is_some_and(|selected| selected == id);
    let path = custom_icon_path_for_id(app, id)?;
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
        write_icon_settings(app, &settings)?;
        crate::taskbar_icon_overlay::set(app, None)?;
    }
    Ok(())
}

fn overlay_icon(
    app: &AppHandle,
    settings: &IconSettings,
) -> Result<Option<crate::taskbar_icon_overlay::OverlayIcon>, String> {
    let bytes = match settings.start_icon {
        StartIcon::System => return Ok(None),
        StartIcon::Gem => GEM_ICON.to_vec(),
        StartIcon::Diamond => DIAMOND_ICON.to_vec(),
        StartIcon::Custom => std::fs::read(custom_icon_path_for_settings(app, settings)?)
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

fn read_icon_settings(app: &AppHandle) -> Result<IconSettings, String> {
    let path = icon_settings_path(app)?;
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

fn write_icon_settings(app: &AppHandle, settings: &IconSettings) -> Result<(), String> {
    let bytes = serde_json::to_vec(&settings).map_err(|error| error.to_string())?;
    write_file_atomically(&icon_settings_path(app)?, &bytes)
}

fn selected_custom_id(app: &AppHandle, settings: &IconSettings) -> Option<String> {
    if settings.start_icon != StartIcon::Custom {
        return None;
    }
    settings.selected_custom_icon.clone().or_else(|| {
        custom_icon_path(app)
            .ok()
            .filter(|path| path.is_file())
            .map(|_| "legacy".to_string())
    })
}

fn custom_start_icons(app: &AppHandle) -> Result<Vec<CustomStartIcon>, String> {
    let mut icons = Vec::new();
    let legacy = custom_icon_path(app)?;
    if let Ok(preview) = std::fs::read(legacy) {
        icons.push(CustomStartIcon {
            id: "legacy".to_string(),
            preview: encode_base64(&preview),
        });
    }
    let directory = custom_icon_dir(app)?;
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

fn thickness_name(legacy_value: u32, icon_preference: u32) -> &'static str {
    if icon_preference == 0 || legacy_value == 0 {
        "compact"
    } else if icon_preference == 2 {
        "adaptive"
    } else {
        "default"
    }
}

fn combine_name(value: u32) -> &'static str {
    match value {
        1 => "whenFull",
        2 => "never",
        _ => "always",
    }
}

fn searchbox_mode_name(value: u32) -> &'static str {
    match value {
        0 => "hidden",
        1 => "icon",
        2 => "box",
        3 => "iconWithLabel",
        _ => "icon",
    }
}

fn write_advanced_dword(name: &str, value: u32) -> Result<(), String> {
    let key = open_key(EXPLORER_ADVANCED_KEY, KEY_SET_VALUE)?
        .ok_or_else(|| "Windows taskbar settings are unavailable".to_string())?;
    write_dword(key.0, name, value)
}

fn open_key(path: &str, access: REG_SAM_FLAGS) -> Result<Option<RegistryKey>, String> {
    let path = wide(path);
    let mut key = HKEY::default();
    let error = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            access,
            &mut key,
        )
    };
    if error.0 == 2 {
        return Ok(None);
    }
    if error.0 != 0 || key.0.is_null() {
        return Err(format!(
            "open taskbar registry settings: Win32 error {}",
            error.0
        ));
    }
    Ok(Some(RegistryKey(key)))
}

fn read_dword(key: HKEY, name: &str) -> Result<Option<u32>, String> {
    let name = wide(name);
    let mut value_type = REG_VALUE_TYPE::default();
    let mut size = 4u32;
    let mut bytes = [0u8; 4];
    let error = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut value_type),
            Some(bytes.as_mut_ptr()),
            Some(&mut size),
        )
    };
    if error.0 == 2 {
        return Ok(None);
    }
    if error.0 != 0 || value_type != REG_DWORD || size != 4 {
        return Err(format!(
            "read taskbar registry setting: Win32 error {}",
            error.0
        ));
    }
    Ok(Some(u32::from_le_bytes(bytes)))
}

fn write_dword(key: HKEY, name: &str, value: u32) -> Result<(), String> {
    let name = wide(name);
    let error = unsafe {
        RegSetValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        )
    };
    if error.0 == 0 {
        Ok(())
    } else {
        Err(format!(
            "write taskbar registry setting: Win32 error {}",
            error.0
        ))
    }
}

fn write_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "taskbar customization path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create taskbar customization directory: {error}"))?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)
        .map_err(|error| format!("write taskbar customization file: {error}"))?;
    crate::files::replace_file(&temp, path)
}

fn icon_settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join(ICON_SETTINGS_NAME))
        .map_err(|error| error.to_string())
}

fn custom_icon_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join(CUSTOM_ICON_PNG))
        .map_err(|error| error.to_string())
}

fn custom_icon_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join(CUSTOM_ICON_DIR))
        .map_err(|error| error.to_string())
}

fn custom_icon_path_for_settings(
    app: &AppHandle,
    settings: &IconSettings,
) -> Result<PathBuf, String> {
    match settings.selected_custom_icon.as_deref() {
        Some(id) => custom_icon_path_for_id(app, id),
        None => custom_icon_path(app),
    }
}

fn custom_icon_path_for_id(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    if !valid_custom_icon_id(id) {
        return Err("Invalid custom icon identifier".to_string());
    }
    if id == "legacy" {
        custom_icon_path(app)
    } else {
        custom_icon_dir(app).map(|directory| directory.join(format!("{id}.png")))
    }
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taskbar_preference_codes_have_stable_fallbacks() {
        assert_eq!(thickness_name(0, 1), "compact");
        assert_eq!(thickness_name(1, 0), "compact");
        assert_eq!(thickness_name(1, 1), "default");
        assert_eq!(thickness_name(1, 2), "adaptive");
        assert_eq!(combine_name(0), "always");
        assert_eq!(combine_name(1), "whenFull");
        assert_eq!(combine_name(2), "never");
        assert_eq!(combine_name(99), "always");
    }

    #[test]
    fn searchbox_mode_codes_have_stable_fallbacks() {
        assert_eq!(searchbox_mode_name(0), "hidden");
        assert_eq!(searchbox_mode_name(1), "icon");
        assert_eq!(searchbox_mode_name(2), "box");
        assert_eq!(searchbox_mode_name(3), "iconWithLabel");
        assert_eq!(searchbox_mode_name(99), "icon");
    }

    #[test]
    fn online_presets_are_valid_transparent_pngs() {
        for bytes in [GEM_ICON, DIAMOND_ICON] {
            let icon = decode_png(bytes).unwrap();
            assert_eq!(icon.dimensions(), (96, 96));
            assert!(icon.pixels().any(|pixel| pixel[3] == 0));
            assert!(icon.pixels().any(|pixel| pixel[3] > 0));
        }
    }

    #[test]
    fn icon_input_rejects_empty_and_non_png_data() {
        assert!(prepare_icon(&[]).is_err());
        assert!(prepare_icon(b"not-png").is_err());
    }
}
