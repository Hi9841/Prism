use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const ALLOWED_URIS: &[&str] = &[
    "ms-settings:",
    "ms-settings:display",
    "ms-settings:display-advanced",
    "ms-settings:sound",
    "ms-settings:sound-devices",
    "ms-settings:apps-volume",
    "ms-settings:notifications",
    "ms-settings:quiethours",
    "ms-settings:powersleep",
    "ms-settings:storagesense",
    "ms-settings:storagepolicies",
    "ms-settings:clipboard",
    "ms-settings:multitasking",
    "ms-settings:remotedesktop",
    "ms-settings:about",
    "ms-settings:bluetooth",
    "ms-settings:printers",
    "ms-settings:mousetouchpad",
    "ms-settings:devices-touchpad",
    "ms-settings:typing",
    "ms-settings:usb",
    "ms-settings:camera",
    "ms-settings:network-status",
    "ms-settings:network-wifi",
    "ms-settings:network-ethernet",
    "ms-settings:network-vpn",
    "ms-settings:network-mobilehotspot",
    "ms-settings:network-airplanemode",
    "ms-settings:network-proxy",
    "ms-settings:personalization-background",
    "ms-settings:personalization-colors",
    "ms-settings:themes",
    "ms-settings:lockscreen",
    "ms-settings:personalization-start",
    "ms-settings:taskbar",
    "ms-settings:fonts",
    "ms-settings:appsfeatures",
    "ms-settings:defaultapps",
    "ms-settings:optionalfeatures",
    "ms-settings:startupapps",
    "ms-settings:yourinfo",
    "ms-settings:signinoptions",
    "ms-settings:emailandaccounts",
    "ms-settings:workplace",
    "ms-settings:otherusers",
    "ms-settings:dateandtime",
    "ms-settings:regionlanguage",
    "ms-settings:speech",
    "ms-settings:gaming-gamemode",
    "ms-settings:gaming-gamebar",
    "ms-settings:easeofaccess-narrator",
    "ms-settings:easeofaccess-magnifier",
    "ms-settings:easeofaccess-highcontrast",
    "ms-settings:easeofaccess-colorfilter",
    "ms-settings:easeofaccess-closedcaptioning",
    "ms-settings:privacy",
    "ms-settings:privacy-webcam",
    "ms-settings:privacy-microphone",
    "ms-settings:privacy-location",
    "ms-settings:search-permissions",
    "ms-settings:windowsdefender",
    "ms-settings:windowsupdate",
    "ms-settings:windowsupdate-history",
    "ms-settings:windowsupdate-options",
    "ms-settings:windowsupdate-optionalupdates",
    "ms-settings:activation",
    "ms-settings:recovery",
    "ms-settings:troubleshoot",
    "ms-settings:developers",
    "ms-settings:findmydevice",
];

pub fn open(uri: &str) -> Result<(), String> {
    if !is_allowed(uri) {
        return Err("unsupported Windows Settings URI".to_string());
    }

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

fn is_allowed(uri: &str) -> bool {
    ALLOWED_URIS.contains(&uri)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_accepts_catalog_targets_only() {
        for uri in [
            "ms-settings:display",
            "ms-settings:bluetooth",
            "ms-settings:windowsupdate",
            "ms-settings:defaultapps",
        ] {
            assert!(is_allowed(uri), "should accept {uri}");
        }
        for uri in [
            "https://example.com",
            "ms-settings:unknown-page",
            "ms-settings:display?unexpected=true",
            "C:\\Windows\\System32\\cmd.exe",
        ] {
            assert!(!is_allowed(uri), "should reject {uri}");
        }
    }
}
