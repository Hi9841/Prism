use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;

use super::types::VolumeInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Ntfs,
    Directory,
}

/// Chooses the metadata backend without conflating a mount path with a stable
/// volume identity. Raw access is probed separately so access-denied and
/// unsupported environments naturally retain the directory fallback.
pub fn select_backend(volume: &VolumeInfo, raw_access_available: bool) -> BackendKind {
    if raw_access_available
        && volume.drive_type == DRIVE_FIXED
        && volume.fs_type.eq_ignore_ascii_case("NTFS")
        && volume.drive_letter.is_some()
    {
        BackendKind::Ntfs
    } else {
        BackendKind::Directory
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use windows::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_REMOTE, DRIVE_REMOVABLE};

    use super::*;

    fn volume(fs_type: &str, drive_type: u32) -> VolumeInfo {
        VolumeInfo {
            volume_id: "stable-id".into(),
            drive_letter: Some("C:".into()),
            mount_paths: vec![PathBuf::from("C:\\")],
            drive_type,
            label: String::new(),
            fs_type: fs_type.into(),
        }
    }

    #[test]
    fn selects_ntfs_only_for_accessible_local_fixed_volumes() {
        assert_eq!(
            select_backend(&volume("NTFS", DRIVE_FIXED), true),
            BackendKind::Ntfs
        );
        assert_eq!(
            select_backend(&volume("NTFS", DRIVE_FIXED), false),
            BackendKind::Directory
        );
        assert_eq!(
            select_backend(&volume("exFAT", DRIVE_FIXED), true),
            BackendKind::Directory
        );
        assert_eq!(
            select_backend(&volume("NTFS", DRIVE_REMOTE), true),
            BackendKind::Directory
        );
        assert_eq!(
            select_backend(&volume("NTFS", DRIVE_REMOVABLE), true),
            BackendKind::Directory
        );
    }
}
