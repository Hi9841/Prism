//! Registry reads and writes. A `Key` closes its handle when it drops.

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, KEY_READ, REG_DWORD,
    REG_EXPAND_SZ, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
};

use super::wide;

/// `ERROR_FILE_NOT_FOUND`: the key or the value does not exist.
const NOT_FOUND: u32 = 2;

pub(crate) struct Key(pub(crate) HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

impl Key {
    /// `Ok(None)`: the key does not exist; `Err`: another Win32 error code.
    pub(crate) fn open(root: HKEY, path: &str, access: REG_SAM_FLAGS) -> Result<Option<Key>, u32> {
        let path = wide(path);
        let mut key = HKEY::default();
        let error = unsafe { RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, access, &mut key) };
        match error.0 as u32 {
            NOT_FOUND => Ok(None),
            0 if !key.0.is_null() => Ok(Some(Key(key))),
            code => Err(code),
        }
    }

    /// The key for reading, when it opens.
    pub(crate) fn open_read(root: HKEY, path: &str) -> Option<Key> {
        Self::open(root, path, KEY_READ).ok().flatten()
    }
}

/// A `REG_SZ` or `REG_EXPAND_SZ` value up to its first NUL, trimmed.
pub(crate) fn string(key: HKEY, name: &str) -> Option<String> {
    let name = wide(name);
    let mut value_type = REG_VALUE_TYPE(0);
    let mut length = 0u32;
    let result =
        unsafe { RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, Some(&mut value_type), None, Some(&mut length)) };
    if result.0 != 0 || length == 0 {
        return None;
    }
    let mut buffer = vec![0u8; length as usize];
    let result = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut value_type),
            Some(buffer.as_mut_ptr()),
            Some(&mut length),
        )
    };
    if result.0 != 0 || (value_type != REG_SZ && value_type != REG_EXPAND_SZ) {
        return None;
    }
    let units: Vec<u16> = buffer.as_chunks::<2>().0.iter().map(|chunk| u16::from_le_bytes(*chunk)).collect();
    Some(super::wide_trim(&units))
}

pub(crate) enum DwordError {
    Win32(u32),
    NotDword,
}

/// A `REG_DWORD` value; `Ok(None)` when it does not exist.
pub(crate) fn dword(key: HKEY, name: &str) -> Result<Option<u32>, DwordError> {
    let name = wide(name);
    let mut value_type = REG_VALUE_TYPE::default();
    let mut size = 4u32;
    let mut bytes = [0u8; 4];
    let error = unsafe {
        RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, Some(&mut value_type), Some(bytes.as_mut_ptr()), Some(&mut size))
    };
    match error.0 as u32 {
        NOT_FOUND => Ok(None),
        0 if value_type == REG_DWORD && size == 4 => Ok(Some(u32::from_le_bytes(bytes))),
        0 => Err(DwordError::NotDword),
        code => Err(DwordError::Win32(code)),
    }
}

/// A string or DWORD value as text.
pub(crate) fn text(key: HKEY, name: &str) -> Option<String> {
    string(key, name).or_else(|| dword(key, name).ok().flatten().map(|value| value.to_string()))
}

/// The value exists, whatever its type (shell registrations use empty marker values).
pub(crate) fn has_value(key: HKEY, name: &str) -> bool {
    let name = wide(name);
    unsafe { RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, None, None).0 == 0 }
}

/// Writes a `REG_DWORD`; `Err` is the Win32 error code.
pub(crate) fn set_dword(key: HKEY, name: &str, value: u32) -> Result<(), u32> {
    let name = wide(name);
    let error = unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_DWORD, Some(&value.to_le_bytes())) };
    if error.0 == 0 { Ok(()) } else { Err(error.0 as u32) }
}

/// The names of the subkeys, in registry order.
pub(crate) fn subkeys(key: HKEY) -> Vec<String> {
    let mut names = Vec::new();
    for index in 0.. {
        let mut buffer = [0u16; 512];
        let mut length = buffer.len() as u32;
        let result = unsafe {
            RegEnumKeyExW(key, index, Some(PWSTR(buffer.as_mut_ptr())), &mut length, None, None, None, None)
        };
        if result.0 != 0 {
            break;
        }
        names.push(String::from_utf16_lossy(&buffer[..length as usize]));
    }
    names
}
