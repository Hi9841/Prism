use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub fn open(uri: &str) -> Result<(), String> {
    let operation = wide("open");
    let target = wide(uri);
    let result = unsafe {
        ShellExecuteW(
            None,
            Some(&PCWSTR(operation.as_ptr())),
            PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("Windows could not open {uri} (Shell error {code})"))
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
