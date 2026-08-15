use std::collections::HashSet;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDriveTypeW, GetLogicalDriveStringsW,
    GetVolumeInformationW, GetVolumePathNamesForVolumeNameW,
};
use windows::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
};

use super::types::VolumeInfo;

pub fn discover_volumes() -> Vec<VolumeInfo> {
    let mut volumes = Vec::new();

    // 1. Enumerate volumes via FindFirstVolumeW to get volume GUID paths
    let mut volume_name_buf = [0u16; 512];
    if let Ok(handle) = unsafe { FindFirstVolumeW(&mut volume_name_buf) } {
        loop {
            let volume_guid_path = wide_to_string(&volume_name_buf);
            if !volume_guid_path.is_empty() {
                if let Some(vol) = inspect_volume(&volume_guid_path) {
                    volumes.push(vol);
                }
            }

            let success = unsafe { FindNextVolumeW(handle, &mut volume_name_buf).is_ok() };
            if !success {
                break;
            }
        }
        let _ = unsafe { FindVolumeClose(handle) };
    }

    // 2. Also check standard logical drive strings (covers mapped network drives and any missed letters)
    let logical_drives = get_logical_drives();
    for drive in logical_drives {
        let drive_root = format!("{drive}:\\");
        if let Some(vol) = inspect_volume(&drive_root) {
            volumes.push(vol);
        }
    }

    dedupe_by_mount(volumes)
}

/// The same drive is enumerated both as a volume GUID (FindFirstVolumeW) and
/// as a drive letter (GetLogicalDriveStringsW), each producing a different
/// volume_id. Deduplicate by mount path so a drive is never scanned twice or
/// stored under two identities.
fn dedupe_by_mount(volumes: Vec<VolumeInfo>) -> Vec<VolumeInfo> {
    let mut seen = HashSet::new();
    let mut volumes = volumes;
    // Windows does not guarantee enumeration order. Stable ordering prevents
    // a harmless drive-order change from looking like a topology change to the
    // background poller.
    volumes.sort_by_key(mount_key);
    volumes
        .into_iter()
        .filter(|vol| seen.insert(mount_key(vol)))
        .collect()
}

/// Canonical identity of a volume for deduplication: its first mount path,
/// lowercased (falling back to the volume id).
fn mount_key(vol: &VolumeInfo) -> String {
    vol.mount_paths
        .first()
        .map(|p| p.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| vol.volume_id.clone())
}
fn inspect_volume(root_path: &str) -> Option<VolumeInfo> {
    let wide_root: Vec<u16> = root_path.encode_utf16().chain(Some(0)).collect();
    let drive_type = unsafe { GetDriveTypeW(PCWSTR(wide_root.as_ptr())) };

    if !is_supported_drive_type(drive_type) {
        return None;
    }

    // Get volume information (serial number, label, fs)
    let mut volume_name_buf = [0u16; 260];
    let mut serial_number = 0u32;
    let mut max_component_len = 0u32;
    let mut flags = 0u32;
    let mut fs_name_buf = [0u16; 260];

    let info_ok = unsafe {
        GetVolumeInformationW(
            PCWSTR(wide_root.as_ptr()),
            Some(&mut volume_name_buf),
            Some(&mut serial_number),
            Some(&mut max_component_len),
            Some(&mut flags),
            Some(&mut fs_name_buf),
        )
        .is_ok()
    };

    // For unready optical media or disconnected drives, GetVolumeInformationW fails
    if !info_ok && drive_type == DRIVE_CDROM {
        return None;
    }

    let label = wide_to_string(&volume_name_buf);
    let fs_type = wide_to_string(&fs_name_buf);

    // Get all mount paths (e.g. C:\ or mounted folder paths)
    let mount_paths = get_volume_mount_paths(root_path);

    let drive_letter = mount_paths.iter().find_map(|p| {
        let s = p.to_string_lossy();
        if s.len() >= 2 && s.as_bytes()[1] == b':' {
            Some(s[..2].to_uppercase())
        } else {
            None
        }
    });

    // Create a stable volume identity
    let volume_id = if root_path.starts_with(r"\\?\Volume{") {
        root_path
            .trim_start_matches(r"\\?\Volume{")
            .trim_end_matches(r"}\")
            .to_string()
    } else if serial_number != 0 {
        format!("vol_{serial_number:08x}")
    } else if let Some(ref letter) = drive_letter {
        format!("drive_{letter}")
    } else {
        root_path.replace(['\\', '/', ':', '?'], "_")
    };

    let effective_mount_paths = if mount_paths.is_empty() {
        if root_path.len() >= 2 && root_path.as_bytes()[1] == b':' {
            vec![PathBuf::from(root_path)]
        } else {
            Vec::new()
        }
    } else {
        mount_paths
    };

    if effective_mount_paths.is_empty() {
        return None;
    }

    Some(VolumeInfo {
        volume_id,
        drive_letter,
        mount_paths: effective_mount_paths,
        drive_type,
        label,
        fs_type,
    })
}

fn get_volume_mount_paths(volume_guid_or_path: &str) -> Vec<PathBuf> {
    let wide_vol: Vec<u16> = volume_guid_or_path.encode_utf16().chain(Some(0)).collect();
    let mut return_len = 0u32;

    unsafe {
        let _ = GetVolumePathNamesForVolumeNameW(PCWSTR(wide_vol.as_ptr()), None, &mut return_len);
    }

    if return_len == 0 {
        return Vec::new();
    }

    let mut buffer = vec![0u16; return_len as usize + 1];
    let ok = unsafe {
        GetVolumePathNamesForVolumeNameW(
            PCWSTR(wide_vol.as_ptr()),
            Some(&mut buffer),
            &mut return_len,
        )
        .is_ok()
    };

    if !ok {
        return Vec::new();
    }

    buffer[..return_len as usize]
        .split(|c| *c == 0)
        .filter(|slice| !slice.is_empty())
        .map(|slice| PathBuf::from(OsString::from_wide(slice)))
        .collect()
}

fn get_logical_drives() -> Vec<char> {
    let required = unsafe { GetLogicalDriveStringsW(None) };
    if required == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; required as usize + 1];
    let written = unsafe { GetLogicalDriveStringsW(Some(&mut buffer)) } as usize;
    if written == 0 || written > buffer.len() {
        return Vec::new();
    }

    buffer[..written]
        .split(|c| *c == 0)
        .filter(|slice| !slice.is_empty())
        .filter_map(|slice| {
            let str = OsString::from_wide(slice).to_string_lossy().into_owned();
            str.chars().next().map(|c| c.to_ascii_uppercase())
        })
        .collect()
}

fn is_supported_drive_type(drive_type: u32) -> bool {
    matches!(
        drive_type,
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK | DRIVE_REMOTE | DRIVE_CDROM
    )
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    OsString::from_wide(&wide[..end])
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(id: &str, mount: &str) -> VolumeInfo {
        VolumeInfo {
            volume_id: id.into(),
            drive_letter: Some(mount.into()),
            mount_paths: vec![PathBuf::from(mount)],
            drive_type: 3,
            label: String::new(),
            fs_type: "NTFS".into(),
        }
    }

    #[test]
    fn same_drive_under_two_identities_is_deduplicated_by_mount_path() {
        let volumes = vec![
            vol("guid-1f8ab82f", "C:\\"),
            vol("vol_3a48e069", "C:\\"),
            vol("guid-other", "D:\\"),
        ];
        let deduped = dedupe_by_mount(volumes);
        assert_eq!(deduped.len(), 2);
        assert!(deduped
            .iter()
            .any(|v| v.drive_letter.as_deref() == Some("C:\\")));
        assert!(deduped
            .iter()
            .any(|v| v.drive_letter.as_deref() == Some("D:\\")));
    }

    #[test]
    fn mount_paths_are_case_insensitive() {
        let volumes = vec![vol("a", "C:\\"), vol("b", "c:\\")];
        assert_eq!(dedupe_by_mount(volumes).len(), 1);
    }
}
