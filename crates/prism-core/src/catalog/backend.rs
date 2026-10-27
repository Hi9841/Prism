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
