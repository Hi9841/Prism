use windows::Win32::Foundation::{ERROR_HANDLE_EOF, ERROR_MORE_DATA};
use windows::Win32::System::Ioctl::{FSCTL_ENUM_USN_DATA, MFT_ENUM_DATA_V0};

use crate::catalog::types::NtfsNode;

use super::usn::{parse_record, read_u64, ParsedUsnRecord};
use super::volume::{win32_error_code, NtfsVolume};

const ENUM_BUFFER_SIZE: usize = 1024 * 1024;

pub(super) fn enumerate(
    volume: &mut NtfsVolume,
    high_usn: i64,
    mut consume: impl FnMut(NtfsNode) -> Result<(), String>,
) -> Result<(), String> {
    let mut start_frn = 0u64;
    let mut output = vec![0u8; ENUM_BUFFER_SIZE];

    loop {
        let input = MFT_ENUM_DATA_V0 {
            StartFileReferenceNumber: start_frn,
            LowUsn: 0,
            HighUsn: high_usn,
        };
        let returned = match volume.ioctl(FSCTL_ENUM_USN_DATA, Some(&input), &mut output) {
            Ok(bytes) => bytes as usize,
            Err(error) => match win32_error_code(&error) {
                code if code == ERROR_HANDLE_EOF.0 => break,
                code if code == ERROR_MORE_DATA.0 && output.len() < 16 * ENUM_BUFFER_SIZE => {
                    output.resize(output.len() * 2, 0);
                    continue;
                }
                code if code == ERROR_MORE_DATA.0 => {
                    return Err("MFT enumeration record exceeded the maximum buffer".to_string())
                }
                _ => return Err(format!("enumerate NTFS MFT: {error}")),
            },
        };
        if returned < 8 || returned > output.len() {
            return Err("FSCTL_ENUM_USN_DATA returned an invalid buffer length".to_string());
        }
        let next_frn = read_u64(&output[..returned], 0)
            .ok_or_else(|| "MFT response did not contain a cursor".to_string())?;
        let mut offset = 8usize;
        while offset < returned {
            let (record, length) = parse_record(&output[offset..returned])?;
            match record {
                ParsedUsnRecord::V2(record) => consume(record.into_node())?,
                ParsedUsnRecord::Unsupported { major_version } => {
                    return Err(format!(
                        "unsupported USN record major version {major_version} during MFT enumeration"
                    ));
                }
            }
            offset = offset
                .checked_add(length)
                .ok_or_else(|| "MFT record offset overflow".to_string())?;
        }
        if offset != returned {
            return Err("MFT response ended between records".to_string());
        }
        if next_frn <= start_frn {
            return Err("MFT enumeration cursor did not advance".to_string());
        }
        start_frn = next_frn;
    }
    Ok(())
}
