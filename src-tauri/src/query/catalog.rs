//! In-memory typed action catalog.
//!
//! OS actions live here, never in the file FTS5 tables.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    Settings,
    AudioMute,
    AudioUnmute,
    AudioToggleMute,
    PowerLock,
    PowerSleep,
    PowerShutdown,
    PowerRestart,
}

#[derive(Clone, Copy, Debug)]
pub struct TypedAction {
    pub id: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub keywords: &'static [&'static str],
    pub kind: ActionKind,
    pub uri: Option<&'static str>,
    pub icon_key: &'static str,
}

const fn settings(
    id: &'static str,
    title: &'static str,
    uri: &'static str,
    icon_key: &'static str,
    keywords: &'static [&'static str],
) -> TypedAction {
    TypedAction {
        id,
        title,
        subtitle: "Windows Settings",
        keywords,
        kind: ActionKind::Settings,
        uri: Some(uri),
        icon_key,
    }
}

const fn action(
    id: &'static str,
    title: &'static str,
    subtitle: &'static str,
    kind: ActionKind,
    icon_key: &'static str,
    keywords: &'static [&'static str],
) -> TypedAction {
    TypedAction {
        id,
        title,
        subtitle,
        keywords,
        kind,
        uri: None,
        icon_key,
    }
}

const ACTIONS: &[TypedAction] = &[
    settings(
        "settings.nightlight",
        "Night Light",
        "ms-settings:nightlight",
        "nightlight",
        &[
            "blue light",
            "eyes",
            "warm",
            "night mode",
            "nightlight",
            "reduce blue",
        ],
    ),
    settings(
        "settings.display",
        "Display",
        "ms-settings:display",
        "display",
        &[
            "display settings",
            "screen",
            "monitor",
            "resolution",
            "brightness",
        ],
    ),
    settings(
        "settings.hdr",
        "HDR",
        "ms-settings:hdr",
        "hdr",
        &["high dynamic range", "auto hdr", "hdr display", "hdr video"],
    ),
    settings(
        "settings.display-advanced",
        "Advanced display",
        "ms-settings:display-advanced",
        "display",
        &["refresh rate", "display information"],
    ),
    settings(
        "settings.sound",
        "Sound",
        "ms-settings:sound",
        "sound",
        &["audio", "speakers", "microphone", "sound settings"],
    ),
    settings(
        "settings.sound-devices",
        "Sound devices",
        "ms-settings:sound-devices",
        "sound",
        &["audio devices", "input", "output"],
    ),
    settings(
        "settings.apps-volume",
        "Volume mixer",
        "ms-settings:apps-volume",
        "sound",
        &["app volume", "sound mixer"],
    ),
    settings(
        "settings.storagesense",
        "Storage",
        "ms-settings:storagesense",
        "storage",
        &["disk space", "storage settings"],
    ),
    settings(
        "settings.storagepolicies",
        "Storage Sense",
        "ms-settings:storagepolicies",
        "storage",
        &[
            "automatic cleanup",
            "temporary files",
            "free space",
            "disk cleanup",
        ],
    ),
    settings(
        "settings.powersleep",
        "Power & sleep",
        "ms-settings:powersleep",
        "power",
        &["power", "battery", "sleep settings"],
    ),
    settings(
        "settings.bluetooth",
        "Bluetooth",
        "ms-settings:bluetooth",
        "bluetooth",
        &["bluetooth settings", "pair device", "wireless devices"],
    ),
    settings(
        "settings.network-status",
        "Network & internet",
        "ms-settings:network-status",
        "network",
        &["network status", "internet settings", "wifi", "ethernet"],
    ),
    settings(
        "settings.network-wifi",
        "Wi-Fi",
        "ms-settings:network-wifi",
        "network",
        &["wifi", "wireless network"],
    ),
    settings(
        "settings.windowsupdate",
        "Windows Update",
        "ms-settings:windowsupdate",
        "update",
        &["updates", "check for updates"],
    ),
    settings(
        "settings.windowsupdate-history",
        "Update history",
        "ms-settings:windowsupdate-history",
        "update",
        &["windows update history", "installed updates"],
    ),
    settings(
        "settings.notifications",
        "Notifications",
        "ms-settings:notifications",
        "settings",
        &["notification settings", "alerts"],
    ),
    settings(
        "settings.quiethours",
        "Focus assist",
        "ms-settings:quiethours",
        "settings",
        &["focus", "do not disturb", "quiet hours"],
    ),
    settings(
        "settings.clipboard",
        "Clipboard",
        "ms-settings:clipboard",
        "settings",
        &["clipboard history", "copy paste"],
    ),
    settings(
        "settings.multitasking",
        "Multitasking",
        "ms-settings:multitasking",
        "settings",
        &["snap windows", "desktops"],
    ),
    settings(
        "settings.about",
        "About",
        "ms-settings:about",
        "settings",
        &["system information", "device specifications", "pc info"],
    ),
    settings(
        "settings.printers",
        "Printers & scanners",
        "ms-settings:printers",
        "bluetooth",
        &["printer", "scanner", "printing"],
    ),
    settings(
        "settings.mousetouchpad",
        "Mouse & touchpad",
        "ms-settings:mousetouchpad",
        "settings",
        &["mouse settings", "pointer", "trackpad"],
    ),
    settings(
        "settings.home",
        "Settings",
        "ms-settings:",
        "settings",
        &["windows settings", "system settings"],
    ),
    settings(
        "settings.personalization-background",
        "Background",
        "ms-settings:personalization-background",
        "settings",
        &["wallpaper", "desktop background"],
    ),
    settings(
        "settings.personalization-colors",
        "Colors",
        "ms-settings:personalization-colors",
        "settings",
        &["accent color", "dark mode", "light mode"],
    ),
    settings(
        "settings.themes",
        "Themes",
        "ms-settings:themes",
        "settings",
        &["windows theme", "personalization"],
    ),
    settings(
        "settings.taskbar",
        "Taskbar",
        "ms-settings:taskbar",
        "settings",
        &["taskbar settings", "system tray"],
    ),
    settings(
        "settings.appsfeatures",
        "Installed apps",
        "ms-settings:appsfeatures",
        "settings",
        &["apps and features", "uninstall apps", "programs"],
    ),
    settings(
        "settings.defaultapps",
        "Default apps",
        "ms-settings:defaultapps",
        "settings",
        &["file associations", "default programs", "browser default"],
    ),
    settings(
        "settings.privacy-microphone",
        "Microphone privacy",
        "ms-settings:privacy-microphone",
        "settings",
        &["microphone permissions", "mic privacy", "microphone"],
    ),
    settings(
        "settings.privacy",
        "Privacy & security",
        "ms-settings:privacy",
        "settings",
        &["privacy settings", "permissions"],
    ),
    settings(
        "settings.windowsdefender",
        "Windows Security",
        "ms-settings:windowsdefender",
        "settings",
        &["defender", "virus protection", "security settings"],
    ),
    settings(
        "settings.remotedesktop",
        "Remote Desktop",
        "ms-settings:remotedesktop",
        "settings",
        &["remote access", "rdp"],
    ),
    settings(
        "settings.devices-touchpad",
        "Touchpad",
        "ms-settings:devices-touchpad",
        "settings",
        &["trackpad", "touchpad gestures"],
    ),
    settings(
        "settings.typing",
        "Typing",
        "ms-settings:typing",
        "settings",
        &["keyboard typing", "text suggestions", "autocorrect"],
    ),
    settings(
        "settings.usb",
        "USB",
        "ms-settings:usb",
        "settings",
        &["usb devices", "connection notifications"],
    ),
    settings(
        "settings.camera",
        "Camera settings",
        "ms-settings:camera",
        "settings",
        &["webcam", "camera device"],
    ),
    settings(
        "settings.network-ethernet",
        "Ethernet",
        "ms-settings:network-ethernet",
        "network",
        &["wired network", "lan"],
    ),
    settings(
        "settings.network-vpn",
        "VPN",
        "ms-settings:network-vpn",
        "network",
        &["virtual private network"],
    ),
    settings(
        "settings.network-mobilehotspot",
        "Mobile hotspot",
        "ms-settings:network-mobilehotspot",
        "network",
        &["hotspot", "internet sharing"],
    ),
    settings(
        "settings.network-airplanemode",
        "Airplane mode",
        "ms-settings:network-airplanemode",
        "network",
        &["flight mode", "wireless off"],
    ),
    settings(
        "settings.network-proxy",
        "Proxy",
        "ms-settings:network-proxy",
        "network",
        &["proxy server", "network proxy"],
    ),
    settings(
        "settings.lockscreen",
        "Lock screen",
        "ms-settings:lockscreen",
        "settings",
        &["lockscreen", "screen timeout"],
    ),
    settings(
        "settings.personalization-start",
        "Start",
        "ms-settings:personalization-start",
        "settings",
        &["start menu", "start settings"],
    ),
    settings(
        "settings.fonts",
        "Fonts",
        "ms-settings:fonts",
        "settings",
        &["font settings", "typefaces"],
    ),
    settings(
        "settings.optionalfeatures",
        "Optional features",
        "ms-settings:optionalfeatures",
        "settings",
        &["windows features", "feature install"],
    ),
    settings(
        "settings.startupapps",
        "Startup apps",
        "ms-settings:startupapps",
        "settings",
        &["startup programs", "login apps", "boot apps"],
    ),
    settings(
        "settings.yourinfo",
        "Your info",
        "ms-settings:yourinfo",
        "settings",
        &["account info", "profile picture"],
    ),
    settings(
        "settings.signinoptions",
        "Sign-in options",
        "ms-settings:signinoptions",
        "settings",
        &["password", "pin", "windows hello", "login"],
    ),
    settings(
        "settings.emailandaccounts",
        "Email & app accounts",
        "ms-settings:emailandaccounts",
        "settings",
        &["email accounts", "app accounts"],
    ),
    settings(
        "settings.workplace",
        "Access work or school",
        "ms-settings:workplace",
        "settings",
        &["work account", "school account", "organization"],
    ),
    settings(
        "settings.otherusers",
        "Family & other users",
        "ms-settings:otherusers",
        "settings",
        &["family", "users", "add account"],
    ),
    settings(
        "settings.dateandtime",
        "Date & time",
        "ms-settings:dateandtime",
        "settings",
        &["clock", "time zone", "date settings"],
    ),
    settings(
        "settings.regionlanguage",
        "Language & region",
        "ms-settings:regionlanguage",
        "settings",
        &["language", "region", "locale"],
    ),
    settings(
        "settings.speech",
        "Speech",
        "ms-settings:speech",
        "settings",
        &["speech language", "voice recognition"],
    ),
    settings(
        "settings.gaming-gamemode",
        "Game Mode",
        "ms-settings:gaming-gamemode",
        "settings",
        &["gaming mode", "game performance"],
    ),
    settings(
        "settings.gaming-gamebar",
        "Xbox Game Bar",
        "ms-settings:gaming-gamebar",
        "settings",
        &["game bar", "xbox overlay"],
    ),
    settings(
        "settings.easeofaccess-narrator",
        "Narrator",
        "ms-settings:easeofaccess-narrator",
        "settings",
        &["screen reader", "accessibility narrator"],
    ),
    settings(
        "settings.easeofaccess-magnifier",
        "Magnifier",
        "ms-settings:easeofaccess-magnifier",
        "settings",
        &["screen magnifier", "zoom accessibility"],
    ),
    settings(
        "settings.easeofaccess-highcontrast",
        "Contrast themes",
        "ms-settings:easeofaccess-highcontrast",
        "settings",
        &["high contrast", "accessibility colors"],
    ),
    settings(
        "settings.easeofaccess-colorfilter",
        "Color filters",
        "ms-settings:easeofaccess-colorfilter",
        "settings",
        &["color blindness", "accessibility filters"],
    ),
    settings(
        "settings.easeofaccess-closedcaptioning",
        "Captions",
        "ms-settings:easeofaccess-closedcaptioning",
        "settings",
        &["closed captions", "subtitles", "accessibility captions"],
    ),
    settings(
        "settings.privacy-webcam",
        "Camera privacy",
        "ms-settings:privacy-webcam",
        "settings",
        &["camera permissions", "webcam privacy"],
    ),
    settings(
        "settings.privacy-location",
        "Location privacy",
        "ms-settings:privacy-location",
        "settings",
        &["location permissions", "gps privacy"],
    ),
    settings(
        "settings.search-permissions",
        "Search permissions",
        "ms-settings:search-permissions",
        "settings",
        &["windows search", "search history"],
    ),
    settings(
        "settings.windowsupdate-options",
        "Advanced update options",
        "ms-settings:windowsupdate-options",
        "update",
        &["windows update advanced", "update options"],
    ),
    settings(
        "settings.windowsupdate-optionalupdates",
        "Optional updates",
        "ms-settings:windowsupdate-optionalupdates",
        "update",
        &["driver updates", "windows optional updates"],
    ),
    settings(
        "settings.activation",
        "Activation",
        "ms-settings:activation",
        "settings",
        &["windows activation", "product key", "license"],
    ),
    settings(
        "settings.recovery",
        "Recovery",
        "ms-settings:recovery",
        "settings",
        &["reset pc", "advanced startup", "restore"],
    ),
    settings(
        "settings.troubleshoot",
        "Troubleshoot",
        "ms-settings:troubleshoot",
        "settings",
        &["troubleshooter", "fix problems"],
    ),
    settings(
        "settings.developers",
        "For developers",
        "ms-settings:developers",
        "settings",
        &["developer mode", "development settings"],
    ),
    settings(
        "settings.findmydevice",
        "Find my device",
        "ms-settings:findmydevice",
        "settings",
        &["locate device", "lost pc"],
    ),
    action(
        "audio.mute",
        "Mute",
        "Volume",
        ActionKind::AudioMute,
        "mute",
        &["mute sound", "silence", "no audio", "kill audio"],
    ),
    action(
        "audio.unmute",
        "Unmute",
        "Volume",
        ActionKind::AudioUnmute,
        "unmute",
        &["restore audio", "sound on", "unmute speakers"],
    ),
    action(
        "audio.toggle-mute",
        "Toggle mute",
        "Volume",
        ActionKind::AudioToggleMute,
        "mute",
        &["mute toggle", "endpoint mute", "speaker mute"],
    ),
    action(
        "power.lock",
        "Lock",
        "Power",
        ActionKind::PowerLock,
        "lock",
        &["lock pc", "lock computer", "lock workstation"],
    ),
    action(
        "power.sleep",
        "Sleep",
        "Power",
        ActionKind::PowerSleep,
        "sleep",
        &["go to sleep", "put to sleep", "sleep now"],
    ),
    action(
        "power.shutdown",
        "Shut down",
        "Power",
        ActionKind::PowerShutdown,
        "shutdown",
        &["shutdown", "power off", "turn off"],
    ),
    action(
        "power.restart",
        "Restart",
        "Power",
        ActionKind::PowerRestart,
        "restart",
        &["reboot", "restart pc"],
    ),
];

pub fn get(id: &str) -> Option<&'static TypedAction> {
    ACTIONS.iter().find(|action| action.id == id)
}

pub fn search(query: &str, limit: usize) -> Vec<(&'static TypedAction, i32)> {
    let mut ranked: Vec<(&'static TypedAction, i32)> = ACTIONS
        .iter()
        .filter_map(|action| {
            super::score::score_with_keywords(query, action.title, action.keywords)
                .map(|score| (action, score))
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.title.cmp(b.0.title)));
    ranked.truncate(limit);
    ranked
}

pub fn execute(action: &TypedAction) -> Result<(), String> {
    match action.kind {
        ActionKind::Settings => {
            let uri = action
                .uri
                .ok_or_else(|| format!("settings action {} is missing a URI", action.id))?;
            crate::windows_settings::open(uri)
        }
        ActionKind::AudioMute => crate::audio::set_default_endpoint_mute(true).map(|_| ()),
        ActionKind::AudioUnmute => crate::audio::set_default_endpoint_mute(false).map(|_| ()),
        ActionKind::AudioToggleMute => crate::audio::toggle_default_endpoint_mute().map(|_| ()),
        ActionKind::PowerLock => crate::power::perform("lock"),
        ActionKind::PowerSleep => crate::power::perform("sleep"),
        ActionKind::PowerShutdown => crate::power::perform("shutdown"),
        ActionKind::PowerRestart => crate::power::perform("restart"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_actions_are_present() {
        assert!(!ACTIONS.is_empty());
        for id in [
            "settings.nightlight",
            "settings.display",
            "settings.hdr",
            "settings.bluetooth",
            "settings.sound",
            "settings.storagepolicies",
            "settings.powersleep",
            "settings.windowsupdate",
            "settings.network-status",
            "settings.easeofaccess-narrator",
            "audio.mute",
            "audio.unmute",
            "audio.toggle-mute",
            "power.lock",
            "power.sleep",
            "power.shutdown",
            "power.restart",
        ] {
            assert!(get(id).is_some(), "missing catalog action {id}");
        }
    }

    #[test]
    fn night_light_matches_direct_query() {
        let hits = search("night light", 4);
        assert_eq!(hits[0].0.id, "settings.nightlight");
    }

    #[test]
    fn narrator_is_in_the_typed_catalog() {
        let hits = search("narrator", 4);
        assert_eq!(hits[0].0.id, "settings.easeofaccess-narrator");
    }

    #[test]
    fn storage_sense_matches_cleanup_keyword() {
        let hits = search("disk cleanup", 4);
        assert!(hits
            .iter()
            .any(|(action, _)| action.id == "settings.storagepolicies"));
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = ACTIONS.iter().map(|action| action.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ACTIONS.len());
    }
}
