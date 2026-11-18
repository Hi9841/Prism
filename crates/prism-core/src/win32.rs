//! Small helpers that Win32 calls across the backend share.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::{GUID, PCWSTR};
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::SHGetKnownFolderPath;

/// NUL-terminated UTF-16 for a `PCWSTR` argument.
pub(crate) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

/// The text of a UTF-16 buffer up to its first NUL, trimmed.
pub(crate) fn wide_trim(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}

/// NUL-terminated UTF-16 of a path.
pub(crate) fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Moves `temp` over `destination` in one step (the atomic save of a data file).
pub(crate) fn replace_file(temp: &Path, destination: &Path) -> Result<(), String> {
    let (from, to) = (wide_path(temp), wide_path(destination));
    unsafe {
        MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)
            .map_err(|error| error.to_string())
    }
}

/// The path of a known folder (`FOLDERID_*`), when Windows has one.
pub(crate) fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let path = SHGetKnownFolderPath(id, Default::default(), None).ok()?;
        let text = path.to_string().unwrap_or_default();
        CoTaskMemFree(Some(path.as_ptr() as *const std::ffi::c_void));
        (!text.is_empty()).then(|| PathBuf::from(text))
    }
}
