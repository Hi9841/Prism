use std::ffi::c_void;
use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;

use crate::catalog::types::{JournalMetadata, VolumeInfo};

use super::super::backend::{select_backend, BackendKind};
use super::mft;
use super::usn::{self, JournalReadBatch};

/// Narrow transport boundary for raw-volume operations. A future service
/// implementation can satisfy this interface over a validated named-pipe
/// protocol without changing catalog/database synchronization code.
pub(crate) trait NtfsTransport {
    fn query_journal(&self) -> Result<JournalMetadata, String>;
    fn enumerate_mft(
        &mut self,
        high_usn: i64,
        consume: &mut dyn FnMut(crate::catalog::types::NtfsNode) -> Result<(), String>,
    ) -> Result<(), String>;
    fn read_journal(
        &self,
        start_usn: i64,
        journal_id: u64,
        target_usn: i64,
    ) -> Result<JournalReadBatch, String>;
}

pub struct NtfsVolume {
    pub(super) handle: HANDLE,
}

impl NtfsVolume {
    pub fn open(info: &VolumeInfo) -> Result<Self, String> {
        if select_backend(info, true) != BackendKind::Ntfs {
            return Err("volume is not eligible for direct NTFS indexing".to_string());
        }
        let drive = info
            .drive_letter
            .as_deref()
            .filter(|drive| {
                let bytes = drive.as_bytes();
                bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
            })
            .ok_or_else(|| "NTFS raw access requires a validated drive-letter mount".to_string())?;
        let device = format!(r"\\.\{}:", drive.as_bytes()[0] as char);
        let wide: Vec<u16> = device.encode_utf16().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|error| format!("open raw NTFS volume {drive}: {error}"))?;
        Ok(Self { handle })
    }

    pub(super) fn query_journal(&self) -> Result<JournalMetadata, String> {
        usn::query_journal(self)
    }

    pub(super) fn enumerate_mft(
        &mut self,
        high_usn: i64,
        consume: impl FnMut(crate::catalog::types::NtfsNode) -> Result<(), String>,
    ) -> Result<(), String> {
        mft::enumerate(self, high_usn, consume)
    }

    pub(super) fn read_journal(
        &self,
        start_usn: i64,
        journal_id: u64,
        target_usn: i64,
    ) -> Result<JournalReadBatch, String> {
        usn::read_journal(self, start_usn, journal_id, target_usn)
    }

    pub(super) fn ioctl<I: Sized>(
        &self,
        control_code: u32,
        input: Option<&I>,
        output: &mut [u8],
    ) -> Result<u32, windows::core::Error> {
        let mut returned = 0u32;
        unsafe {
            DeviceIoControl(
                self.handle,
                control_code,
                input.map(|value| value as *const I as *const c_void),
                input.map_or(0, |_| size_of::<I>() as u32),
                if output.is_empty() {
                    None
                } else {
                    Some(output.as_mut_ptr() as *mut c_void)
                },
                output.len() as u32,
                Some(&mut returned),
                None,
            )
        }?;
        Ok(returned)
    }
}

impl Drop for NtfsVolume {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

impl NtfsTransport for NtfsVolume {
    fn query_journal(&self) -> Result<JournalMetadata, String> {
        self.query_journal()
    }

    fn enumerate_mft(
        &mut self,
        high_usn: i64,
        consume: &mut dyn FnMut(crate::catalog::types::NtfsNode) -> Result<(), String>,
    ) -> Result<(), String> {
        self.enumerate_mft(high_usn, consume)
    }

    fn read_journal(
        &self,
        start_usn: i64,
        journal_id: u64,
        target_usn: i64,
    ) -> Result<JournalReadBatch, String> {
        self.read_journal(start_usn, journal_id, target_usn)
    }
}

pub(super) fn win32_error_code(error: &windows::core::Error) -> u32 {
    // DeviceIoControl surfaces HRESULT_FROM_WIN32; its low 16 bits preserve
    // the originating system error code.
    error.code().0 as u32 & 0xffff
}
