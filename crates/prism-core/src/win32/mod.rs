//! Small helpers that Win32 calls across the backend share.

pub(crate) mod registry;

use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::{GUID, PCWSTR};
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED};
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

/// Saves `bytes` as `path` in one step: a reader sees the old file or the new
/// one, never a part. The bytes go to `<path>.tmp` first.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    write_through_temp(path, bytes, false)
}

/// As `write_atomic`, and the bytes are on the disk before the replace, so a
/// crash or power loss keeps a whole file (the restore journal, the file
/// index snapshot). The flush costs about 2 ms, so caches and settings do not.
pub(crate) fn write_atomic_synced(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    write_through_temp(path, bytes, true)
}

/// The step of an atomic write that failed, with the system's message. Each
/// caller names the file in its own words.
#[derive(Debug)]
pub(crate) enum WriteError {
    /// Creating the folder.
    Folder(String),
    /// Creating `<path>.tmp`.
    Create(String),
    /// Writing (and flushing) the bytes.
    Write(String),
    /// Replacing `path` with the temp file.
    Replace(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::Folder(detail) | Self::Create(detail) | Self::Write(detail) | Self::Replace(detail)) = self;
        f.write_str(detail)
    }
}

impl From<WriteError> for String {
    fn from(error: WriteError) -> Self {
        error.to_string()
    }
}

fn write_through_temp(path: &Path, bytes: &[u8], sync: bool) -> Result<(), WriteError> {
    if let Some(folder) = path.parent().filter(|folder| !folder.as_os_str().is_empty()) {
        std::fs::create_dir_all(folder).map_err(|error| WriteError::Folder(error.to_string()))?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    let result = std::fs::File::create(&temp)
        .map_err(|error| WriteError::Create(error.to_string()))
        .and_then(|mut file| {
            file.write_all(bytes)
                .and_then(|()| if sync { file.sync_all() } else { Ok(()) })
                .map_err(|error| WriteError::Write(error.to_string()))
        })
        .and_then(|()| replace_file(&temp, path).map_err(WriteError::Replace));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Moves `temp` over `destination` in one step.
fn replace_file(temp: &Path, destination: &Path) -> Result<(), String> {
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

/// COM for this thread (apartment-threaded) while the guard lives.
pub(crate) struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    pub(crate) fn init() -> Self {
        Self {
            initialized: unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok(),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // CoUninitialize must only balance a successful CoInitializeEx call.
        // In particular, RPC_E_CHANGED_MODE means this thread already belongs
        // to a different apartment and must be left untouched.
        if self.initialized {
            unsafe { CoUninitialize() }
        }
    }
}
