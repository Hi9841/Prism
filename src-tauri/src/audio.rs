//! Windows Core Audio (WASAPI) per-application volume control.

use std::path::PathBuf;
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, HWND, POINT};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioSessionControl2, IAudioSessionEnumerator, IAudioSessionManager2,
    IMMDevice, IMMDeviceCollection, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetClassNameW, GA_ROOT};

#[derive(Debug, Clone)]
pub struct AudioSessionEntry {
    pub device_name: String,
    pub pid: u32,
    pub process_name: String,
    pub volume: f32,
    pub muted: bool,
}

/// Owns COM initialization for the dedicated audio worker thread.
pub(crate) struct ComApartment;

impl ComApartment {
    pub(crate) fn initialize() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|error| format!("initialize audio COM apartment: {error}"))?;
        }
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub fn get_process_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let success = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if success.is_ok() && size > 0 {
            let path = String::from_utf16_lossy(&buffer[..size as usize]);
            let file_name = PathBuf::from(&path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or(path);
            return Some(file_name);
        }
    }
    None
}

/// Retrieves or adjusts the default render device's master volume.
fn adjust_master_volume(delta: f32) -> Result<(f32, bool), String> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("create MMDeviceEnumerator: {e}"))?;
        let device: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .map_err(|e| format!("get default audio endpoint: {e}"))?;
        let endpoint_vol: IAudioEndpointVolume = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("activate IAudioEndpointVolume: {e}"))?;

        let current = endpoint_vol
            .GetMasterVolumeLevelScalar()
            .map_err(|e| format!("get master volume: {e}"))?;
        let new_vol = (current + delta).clamp(0.0, 1.0);
        endpoint_vol
            .SetMasterVolumeLevelScalar(new_vol, std::ptr::null())
            .map_err(|e| format!("set master volume: {e}"))?;

        if new_vol > 0.0 {
            endpoint_vol
                .SetMute(false, std::ptr::null())
                .map_err(|error| format!("unmute master volume: {error}"))?;
        }
        let muted = endpoint_vol
            .GetMute()
            .map(|value| value.as_bool())
            .map_err(|error| format!("get master mute state: {error}"))?;

        Ok((new_vol, muted))
    }
}

/// Adjusts every session that belongs to one exact executable identity.
/// If no matching session is found, returns Ok(None).
fn adjust_app_volume(
    executable_stem: &str,
    delta: f32,
) -> Result<Option<(String, f32, bool)>, String> {
    let executable_stem = normalize_executable_stem(executable_stem);
    if executable_stem.is_empty() {
        return Ok(None);
    }
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("create MMDeviceEnumerator: {e}"))?;

        let devices: IMMDeviceCollection = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| format!("enum audio endpoints: {e}"))?;

        let dev_count = devices.GetCount().unwrap_or(0);
        let mut matched_title = None;
        let mut result_vol = 0.0f32;
        let mut result_muted = false;
        let mut matched = false;

        for d in 0..dev_count {
            let device: IMMDevice = match devices.Item(d) {
                Ok(dev) => dev,
                Err(_) => continue,
            };
            let session_manager: IAudioSessionManager2 = match device.Activate(CLSCTX_ALL, None) {
                Ok(sm) => sm,
                Err(_) => continue,
            };
            let session_enumerator: IAudioSessionEnumerator =
                match session_manager.GetSessionEnumerator() {
                    Ok(se) => se,
                    Err(_) => continue,
                };

            let count = session_enumerator.GetCount().unwrap_or(0);
            for i in 0..count {
                let session = match session_enumerator.GetSession(i) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let control: IAudioSessionControl2 = match session.cast() {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let simple: ISimpleAudioVolume = match session.cast() {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let pid = control.GetProcessId().unwrap_or(0);
                if pid == 0 {
                    // pid 0 is the system sounds session. Never match an application to pid 0!
                    continue;
                }
                if control.IsSystemSoundsSession() == windows::Win32::Foundation::S_OK {
                    continue;
                }

                let process_name = get_process_name(pid).unwrap_or_default();
                if process_matches_executable(&process_name, &executable_stem) {
                    let current = simple
                        .GetMasterVolume()
                        .map_err(|error| format!("get {process_name} session volume: {error}"))?;
                    let new_vol = (current + delta).clamp(0.0, 1.0);
                    simple
                        .SetMasterVolume(new_vol, std::ptr::null())
                        .map_err(|error| format!("set {process_name} session volume: {error}"))?;
                    if new_vol > 0.0 {
                        simple
                            .SetMute(false, std::ptr::null())
                            .map_err(|error| format!("unmute {process_name} session: {error}"))?;
                    }
                    let muted = simple
                        .GetMute()
                        .map(|value| value.as_bool())
                        .map_err(|error| format!("get {process_name} mute state: {error}"))?;

                    matched = true;
                    result_vol = new_vol;
                    result_muted = muted;
                    if matched_title.is_none() {
                        let clean_title = process_name.replace(".exe", "");
                        matched_title = Some(clean_title);
                    }
                }
            }
        }

        if matched {
            Ok(Some((
                matched_title.unwrap_or_else(|| "App".to_string()),
                result_vol,
                result_muted,
            )))
        } else {
            Ok(None)
        }
    }
}

fn inspect_element_at(point: POINT) -> Option<InspectedElement> {
    unsafe {
        let uia: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let mut element = uia.ElementFromPoint(point).ok()?;

        let walker = uia.ControlViewWalker().ok();

        // If hitting a child element (e.g. icon image or running bar), walk up to find the taskbar button
        for _ in 0..4 {
            let name = element
                .CurrentName()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let auto_id = element
                .CurrentAutomationId()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let class_name = element
                .CurrentClassName()
                .map(|s| s.to_string())
                .unwrap_or_default();
            if !name.is_empty()
                || auto_id.starts_with("Appid: ")
                || class_name.contains("Button")
                || class_name.contains("TaskList")
            {
                return Some(InspectedElement {
                    name,
                    class_name,
                    automation_id: auto_id,
                });
            }

            if let Some(ref w) = walker {
                if let Ok(parent) = w.GetParentElement(&element) {
                    element = parent;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let name = element
            .CurrentName()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let class_name = element
            .CurrentClassName()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let auto_id = element
            .CurrentAutomationId()
            .map(|s| s.to_string())
            .unwrap_or_default();
        Some(InspectedElement {
            name,
            class_name,
            automation_id: auto_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InspectedElement {
    name: String,
    class_name: String,
    automation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskbarTarget {
    Unknown,
    Master,
    Application {
        display_title: String,
        executable_stem: String,
    },
}

/// Identifies the application or taskbar element under the cursor point.
pub(crate) fn identify_taskbar_target_at(point: POINT) -> TaskbarTarget {
    classify_taskbar_element(inspect_element_at(point))
}

fn classify_taskbar_element(element: Option<InspectedElement>) -> TaskbarTarget {
    let Some(element) = element else {
        return TaskbarTarget::Unknown;
    };

    // Check if over system tray volume icon or tray
    let is_volume_tray = element.name.to_ascii_lowercase().starts_with("volume")
        && (element.automation_id == "SystemTrayIcon"
            || element.class_name.contains("OmniButtonRight"));

    let is_empty_taskbar =
        element.automation_id == "TaskbarFrame" || element.class_name.contains("TaskbarFrame");

    if is_volume_tray || is_empty_taskbar {
        return TaskbarTarget::Master;
    }

    let display_title = clean_app_display_name(&element.name, &element.automation_id);
    let Some(executable_stem) = executable_stem_from_app_id(&element.automation_id) else {
        return TaskbarTarget::Unknown;
    };

    TaskbarTarget::Application {
        display_title,
        executable_stem,
    }
}

fn normalize_executable_stem(value: &str) -> String {
    let lowercase = value.trim().to_ascii_lowercase();
    lowercase
        .strip_suffix(".exe")
        .unwrap_or(&lowercase)
        .to_string()
}

fn process_matches_executable(process_name: &str, executable_stem: &str) -> bool {
    normalize_executable_stem(process_name) == normalize_executable_stem(executable_stem)
}

fn executable_stem_from_app_id(automation_id: &str) -> Option<String> {
    let app_id = automation_id.strip_prefix("Appid: ")?.trim();
    if app_id.is_empty() || app_id.contains('!') {
        return None;
    }
    if app_id.to_ascii_lowercase().ends_with(".exe") {
        return Some(normalize_executable_stem(app_id));
    }

    let mut unique_segments = app_id
        .split('.')
        .map(normalize_executable_stem)
        .filter(|segment| !segment.is_empty());
    let first = unique_segments.next()?;
    if unique_segments.all(|segment| segment == first) {
        Some(first)
    } else {
        None
    }
}

fn clean_app_display_name(name: &str, auto_id: &str) -> String {
    if !name.is_empty() {
        let mut s = name.to_string();
        for pattern in &[
            " - 1 running window pinned",
            " - 2 running windows pinned",
            " - 3 running windows pinned",
            " - 4 running windows pinned",
            " - 5 running windows pinned",
            " running window pinned",
            " running windows pinned",
            " - 1 running window",
            " - 2 running windows",
            " - 3 running windows",
            " - 4 running windows",
            " - 5 running windows",
            " running window",
            " running windows",
            " pinned",
        ] {
            s = s.replace(pattern, "");
        }
        let first_part = s.split(" - ").next().unwrap_or(&s).trim();
        if !first_part.is_empty() {
            return first_part.to_string();
        }
    }
    if auto_id.starts_with("Appid: ") {
        let part = auto_id.trim_start_matches("Appid: ").trim();
        return part.split('.').next_back().unwrap_or(part).to_string();
    }
    "Application".to_string()
}

/// Checks if an HWND belongs to a taskbar window.
pub fn is_taskbar_window(hwnd: HWND) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    unsafe {
        let root = GetAncestor(hwnd, GA_ROOT);
        let target = if !root.0.is_null() { root } else { hwnd };
        let mut class_name = [0u16; 64];
        let len = GetClassNameW(target, &mut class_name);
        if len > 0 {
            let class_str = String::from_utf16_lossy(&class_name[..len as usize]);
            return class_str == "Shell_TrayWnd" || class_str == "Shell_SecondaryTrayWnd";
        }
    }
    false
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VolumeChangeResult {
    pub title: String,
    pub volume: f32,
    pub percentage: u32,
    pub muted: bool,
    pub is_master: bool,
}

/// Main entry point: called when mouse wheel scrolls over the taskbar.
pub(crate) fn adjust_volume_for_target(
    target: &TaskbarTarget,
    delta: f32,
) -> Option<VolumeChangeResult> {
    match target {
        TaskbarTarget::Unknown => None,
        TaskbarTarget::Master => {
            let (vol, muted) = adjust_master_volume(delta).ok()?;
            Some(VolumeChangeResult {
                title: "Master Volume".to_string(),
                volume: vol,
                percentage: (vol * 100.0).round() as u32,
                muted,
                is_master: true,
            })
        }
        TaskbarTarget::Application {
            display_title,
            executable_stem,
        } => match adjust_app_volume(executable_stem, delta) {
            Ok(Some((_app_name, vol, muted))) => Some(VolumeChangeResult {
                title: display_title.clone(),
                volume: vol,
                percentage: (vol * 100.0).round() as u32,
                muted,
                is_master: false,
            }),
            Ok(None) => {
                // The hovered application currently has no active audio session in the Windows Audio Mixer.
                // Do NOT adjust master volume! Show clear inactive status instead of touching overall volume.
                Some(VolumeChangeResult {
                    title: format!("{display_title} (No Audio)"),
                    volume: 0.0,
                    percentage: 0,
                    muted: true,
                    is_master: false,
                })
            }
            Err(_) => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_app_display_name() {
        assert_eq!(
            clean_app_display_name("Google Chrome - 1 running window pinned", "Appid: Chrome"),
            "Google Chrome"
        );
        assert_eq!(
            clean_app_display_name("Discord - 2 running windows", "Appid: Discord"),
            "Discord"
        );
        assert_eq!(clean_app_display_name("Fortnite  ", ""), "Fortnite");
        assert_eq!(
            clean_app_display_name("Spotify Free", "Appid: Spotify.Spotify"),
            "Spotify Free"
        );
        assert_eq!(clean_app_display_name("", "Appid: Chrome"), "Chrome");
    }

    #[test]
    fn inspection_failure_is_unknown_instead_of_master() {
        assert_eq!(classify_taskbar_element(None), TaskbarTarget::Unknown);
    }

    #[test]
    fn only_positive_taskbar_background_identification_is_master() {
        let blank = InspectedElement {
            name: String::new(),
            class_name: String::new(),
            automation_id: String::new(),
        };
        assert_eq!(
            classify_taskbar_element(Some(blank)),
            TaskbarTarget::Unknown
        );

        let background = InspectedElement {
            name: String::new(),
            class_name: "TaskbarFrame".to_string(),
            automation_id: "TaskbarFrame".to_string(),
        };
        assert_eq!(
            classify_taskbar_element(Some(background)),
            TaskbarTarget::Master
        );

        let unrelated_tray_icon = InspectedElement {
            name: "Network".to_string(),
            class_name: "SystemTrayIcon".to_string(),
            automation_id: "SystemTrayIcon".to_string(),
        };
        assert_eq!(
            classify_taskbar_element(Some(unrelated_tray_icon)),
            TaskbarTarget::Unknown
        );

        let volume_tray_icon = InspectedElement {
            name: "Volume 72%".to_string(),
            class_name: "SystemTrayIcon".to_string(),
            automation_id: "SystemTrayIcon".to_string(),
        };
        assert_eq!(
            classify_taskbar_element(Some(volume_tray_icon)),
            TaskbarTarget::Master
        );
    }

    #[test]
    fn unknown_target_returns_before_audio_access() {
        assert!(adjust_volume_for_target(&TaskbarTarget::Unknown, 0.02).is_none());
    }

    #[test]
    fn executable_matching_is_exact_and_case_insensitive() {
        assert!(process_matches_executable("Music.exe", "music"));
        assert!(process_matches_executable("MUSIC.EXE", "music"));
        assert!(!process_matches_executable("MusicBee.exe", "music"));
        assert!(!process_matches_executable("Music.exe", "musicbee"));
    }

    #[test]
    fn ambiguous_app_ids_do_not_resolve_an_executable() {
        assert_eq!(
            executable_stem_from_app_id("Appid: Discord"),
            Some("discord".to_string())
        );
        assert_eq!(
            executable_stem_from_app_id("Appid: Chrome.exe"),
            Some("chrome".to_string())
        );
        assert_eq!(
            executable_stem_from_app_id("Appid: Spotify.Spotify"),
            Some("spotify".to_string())
        );
        assert_eq!(executable_stem_from_app_id("Appid: Vendor.Player"), None);
        assert_eq!(executable_stem_from_app_id("Appid: Package!App"), None);
    }
}
