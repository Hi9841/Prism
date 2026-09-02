//! Windows Core Audio (WASAPI) per-application volume control.

use std::path::PathBuf;
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, HWND, POINT};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, DEVICE_STATE_ACTIVE,
    IAudioSessionControl2, IAudioSessionEnumerator, IAudioSessionManager2,
    IMMDevice, IMMDeviceCollection, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GA_ROOT,
};

#[derive(Debug, Clone)]
pub struct AudioSessionEntry {
    pub device_name: String,
    pub pid: u32,
    pub process_name: String,
    pub volume: f32,
    pub muted: bool,
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
pub fn adjust_master_volume(delta: f32) -> Result<(f32, bool), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
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
            let _ = endpoint_vol.SetMute(false, std::ptr::null());
        }
        let muted = endpoint_vol
            .GetMute()
            .map(|b| b.as_bool())
            .unwrap_or(false);

        Ok((new_vol, muted))
    }
}

/// Adjusts the volume of any audio session whose process or window matches any of the tokens.
/// If no matching session is found, returns Ok(None).
pub fn adjust_app_volume(tokens: &[String], delta: f32) -> Result<Option<(String, f32, bool)>, String> {
    if tokens.is_empty() {
        return Ok(None);
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
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
            let session_enumerator: IAudioSessionEnumerator = match session_manager.GetSessionEnumerator() {
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
                let proc_lower = process_name.to_ascii_lowercase();
                let proc_stem = proc_lower.strip_suffix(".exe").unwrap_or(&proc_lower);
                if proc_stem.is_empty() {
                    continue;
                }

                let is_match = tokens.iter().any(|token| {
                    let t = token.to_ascii_lowercase();
                    if t.len() < 3 {
                        return false;
                    }
                    proc_stem.contains(&t) || t.contains(proc_stem)
                });

                if is_match {
                    let current = simple.GetMasterVolume().unwrap_or(0.5);
                    let new_vol = (current + delta).clamp(0.0, 1.0);
                    let _ = simple.SetMasterVolume(new_vol, std::ptr::null());
                    if new_vol > 0.0 {
                        let _ = simple.SetMute(false, std::ptr::null());
                    }
                    let muted = simple.GetMute().map(|b| b.as_bool()).unwrap_or(false);

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
            Ok(Some((matched_title.unwrap_or_else(|| "App".to_string()), result_vol, result_muted)))
        } else {
            Ok(None)
        }
    }
}

pub fn inspect_element_at(point: POINT) -> Option<(String, String, String, isize)> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let mut element = uia.ElementFromPoint(point).ok()?;

        let walker = uia.ControlViewWalker().ok();

        // If hitting a child element (e.g. icon image or running bar), walk up to find the taskbar button
        for _ in 0..4 {
            let name = element.CurrentName().map(|s| s.to_string()).unwrap_or_default();
            let auto_id = element.CurrentAutomationId().map(|s| s.to_string()).unwrap_or_default();
            let class_name = element.CurrentClassName().map(|s| s.to_string()).unwrap_or_default();
            let hwnd = element.CurrentNativeWindowHandle().map(|h| h.0 as isize).unwrap_or(0);

            if !name.is_empty() || auto_id.starts_with("Appid: ") || class_name.contains("Button") || class_name.contains("TaskList") {
                return Some((name, class_name, auto_id, hwnd));
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

        let name = element.CurrentName().map(|s| s.to_string()).unwrap_or_default();
        let class_name = element.CurrentClassName().map(|s| s.to_string()).unwrap_or_default();
        let auto_id = element.CurrentAutomationId().map(|s| s.to_string()).unwrap_or_default();
        let hwnd = element.CurrentNativeWindowHandle().map(|h| h.0 as isize).unwrap_or(0);
        Some((name, class_name, auto_id, hwnd))
    }
}

#[derive(Debug, Clone)]
pub struct TaskbarTargetInfo {
    pub display_title: String,
    pub tokens: Vec<String>,
    pub is_master: bool,
}

/// Identifies the application or taskbar element under the cursor point.
pub fn identify_taskbar_target_at(point: POINT) -> TaskbarTargetInfo {
    let (name, class_name, auto_id, _) = inspect_element_at(point).unwrap_or_default();

    // Check if over system tray volume icon or tray
    let is_volume_tray = auto_id == "SystemTrayIcon"
        || name.to_ascii_lowercase().starts_with("volume")
        || class_name.contains("OmniButtonRight");

    let is_empty_taskbar = auto_id == "TaskbarFrame"
        || class_name.contains("TaskbarFrame")
        || (name.is_empty() && auto_id.is_empty());

    if is_volume_tray || is_empty_taskbar {
        return TaskbarTargetInfo {
            display_title: "Master Volume".to_string(),
            tokens: Vec::new(),
            is_master: true,
        };
    }

    // Extract clean display title from name
    // e.g. "Google Chrome - 1 running window pinned" -> "Google Chrome"
    let clean_title = clean_app_display_name(&name, &auto_id);

    // Extract search tokens from name and auto_id
    let mut tokens = Vec::new();
    if auto_id.starts_with("Appid: ") {
        let id_part = auto_id.trim_start_matches("Appid: ").trim();
        for segment in id_part.split('.') {
            if segment.len() > 2 {
                tokens.push(segment.to_string());
            }
        }
    } else if !auto_id.is_empty() {
        tokens.push(auto_id);
    }

    for word in clean_title.split_whitespace() {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric());
        if w.len() > 2 {
            tokens.push(w.to_string());
        }
    }

    TaskbarTargetInfo {
        display_title: clean_title,
        tokens,
        is_master: false,
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
        return part.split('.').last().unwrap_or(part).to_string();
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
pub fn adjust_volume_at_taskbar(point: POINT, delta: f32) -> Option<VolumeChangeResult> {
    let target = identify_taskbar_target_at(point);

    if target.is_master {
        let (vol, muted) = adjust_master_volume(delta).ok()?;
        return Some(VolumeChangeResult {
            title: "Master Volume".to_string(),
            volume: vol,
            percentage: (vol * 100.0).round() as u32,
            muted,
            is_master: true,
        });
    }

    // Adjust specific application volume (NEVER fall back to master volume on app buttons)
    match adjust_app_volume(&target.tokens, delta) {
        Ok(Some((_app_name, vol, muted))) => {
            Some(VolumeChangeResult {
                title: target.display_title,
                volume: vol,
                percentage: (vol * 100.0).round() as u32,
                muted,
                is_master: false,
            })
        }
        Ok(None) => {
            // The hovered application currently has no active audio session in the Windows Audio Mixer.
            // Do NOT adjust master volume! Show clear inactive status instead of touching overall volume.
            Some(VolumeChangeResult {
                title: format!("{} (No Audio)", target.display_title),
                volume: 0.0,
                percentage: 0,
                muted: true,
                is_master: false,
            })
        }
        Err(_) => None,
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
        assert_eq!(
            clean_app_display_name("Fortnite  ", ""),
            "Fortnite"
        );
        assert_eq!(
            clean_app_display_name("Spotify Free", "Appid: Spotify.Spotify"),
            "Spotify Free"
        );
        assert_eq!(
            clean_app_display_name("", "Appid: Chrome"),
            "Chrome"
        );
    }

    #[test]
    fn test_adjust_master_volume_query() {
        let res = adjust_master_volume(0.0);
        assert!(res.is_ok(), "query master volume failed: {:?}", res.err());
        let (vol, _muted) = res.unwrap();
        assert!((0.0..=1.0).contains(&vol));
    }

    #[test]
    fn test_adjust_app_volume_isolation() {
        // Query Discord with 0.0 delta
        let discord_res = adjust_app_volume(&["discord".to_string()], 0.0);
        println!("adjust_app_volume discord res: {:?}", discord_res);
        assert!(discord_res.unwrap().is_some());

        // Query Fortnite with 0.0 delta
        let fn_res = adjust_app_volume(&["fortnite".to_string()], 0.0);
        println!("adjust_app_volume fortnite res: {:?}", fn_res);
        assert!(fn_res.unwrap().is_some());

        // Ensure unknown app returns Ok(None) and does not error or alter master
        let none_res = adjust_app_volume(&["nonexistent_prism_app_xyz".to_string()], 0.0);
        assert_eq!(none_res.unwrap(), None);
    }
}
