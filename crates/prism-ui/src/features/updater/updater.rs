//! The release check, download, signature check, and installer handoff that
//! `@tauri-apps/plugin-updater` performed (tauri-plugin-updater 2.10). Port
//! infrastructure: same endpoint, same minisign public key, same NSIS arguments.

use std::io::Read;
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;

/// The public channel. A test build can point at a local channel with its own
/// key: both overrides are fixed at compile time (`PRISM_UPDATE_ENDPOINT`,
/// `PRISM_UPDATE_PUBKEY`), so a release build cannot be redirected at runtime.
const ENDPOINT: &str = match option_env!("PRISM_UPDATE_ENDPOINT") {
    Some(endpoint) => endpoint,
    None => "https://github.com/Hi9841/Prism/releases/latest/download/latest.json",
};
const PUBKEY: &str = match option_env!("PRISM_UPDATE_PUBKEY") {
    Some(key) => key,
    None => "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEZFQzE1QzM0MjE1OThDMDUKUldRRmpGa2hORnpCL29ldEQyYmRRa3VVTGFoSGhsNmVvdGtVS04wSHUzcm9EUGJ6OFVWVCtGMWgK",
};
const TARGET: &str = "windows-x86_64-nsis";
const USER_AGENT: &str = concat!("prism/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug)]
pub struct Update {
    pub version: String,
    url: String,
    signature: String,
}

#[derive(Deserialize)]
struct Platform {
    url: String,
    signature: String,
}

#[derive(Deserialize)]
struct Release {
    #[serde(alias = "name")]
    version: String,
    platforms: Option<std::collections::HashMap<String, Platform>>,
    url: Option<String>,
    signature: Option<String>,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(timeout).user_agent(USER_AGENT).build()
}

/// `check()`: `Some` when the published release is newer than this build.
pub fn check(timeout: Duration, no_cache: bool, current_version: &str) -> Result<Option<Update>, String> {
    if crate::lib::fixture::get().is_some() {
        return Ok(None); // parity runs: the mocked original answers "no update" too
    }
    let mut request = agent(timeout).get(ENDPOINT).set("Accept", "application/json");
    if no_cache {
        request = request.set("Cache-Control", "no-cache").set("Pragma", "no-cache");
    }
    let response = request.call().map_err(|error| error.to_string())?;
    if response.status() == 204 {
        return Ok(None);
    }
    let release: Release = response.into_json().map_err(|error| error.to_string())?;
    let remote = semver::Version::parse(release.version.trim_start_matches('v')).map_err(|error| error.to_string())?;
    let current = semver::Version::parse(current_version.trim_start_matches('v')).map_err(|error| error.to_string())?;
    let (url, signature) = match release.platforms {
        Some(mut platforms) => {
            let platform = platforms
                .remove(TARGET)
                .or_else(|| platforms.remove("windows-x86_64"))
                .ok_or_else(|| format!("the platform `{TARGET}` was not found in the response `platforms` object"))?;
            (platform.url, platform.signature)
        }
        None => (
            release.url.ok_or("the `url` field was not set on the updater response")?,
            release.signature.ok_or("the `signature` field was not set on the updater response")?,
        ),
    };
    Ok((remote > current).then(|| Update { version: remote.to_string(), url, signature }))
}

pub enum DownloadEvent {
    Started { content_length: Option<u64> },
    Progress { chunk_length: u64 },
}

/// Downloads and verifies the installer; `on_event` reports progress.
pub fn download(update: &Update, timeout: Duration, mut on_event: impl FnMut(DownloadEvent)) -> Result<Vec<u8>, String> {
    let response = agent(timeout)
        .get(&update.url)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|error| error.to_string())?;
    let content_length = response.header("Content-Length").and_then(|v| v.parse::<u64>().ok());
    on_event(DownloadEvent::Started { content_length });
    let mut reader = response.into_reader();
    let mut bytes = Vec::with_capacity(content_length.unwrap_or(0) as usize);
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        on_event(DownloadEvent::Progress { chunk_length: read as u64 });
    }
    verify_signature(&bytes, &update.signature)?;
    Ok(bytes)
}

fn base64_text(value: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(value).map_err(|error| error.to_string())?;
    String::from_utf8(bytes).map_err(|_| "signature is not UTF-8".to_string())
}

fn verify_signature(data: &[u8], signature: &str) -> Result<(), String> {
    let public_key =
        minisign_verify::PublicKey::decode(&base64_text(PUBKEY)?).map_err(|error| error.to_string())?;
    let signature =
        minisign_verify::Signature::decode(&base64_text(signature)?).map_err(|error| error.to_string())?;
    public_key.verify(data, &signature, true).map_err(|error| error.to_string())
}

/// Writes the NSIS installer and starts it in passive mode with restart
/// (`/P /R /UPDATE /ARGS`), then exits, as the Tauri updater does.
pub fn install(update: &Update, bytes: &[u8]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    use windows::core::{PCWSTR, w};

    if bytes.len() < 2 || &bytes[..2] != b"MZ" {
        return Err("invalid updater binary format".into());
    }
    let dir = std::env::temp_dir().join(format!("Prism-{}-updater", update.version));
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join(format!("Prism_{}_x64-setup.exe", update.version));
    std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
    let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let parameters: Vec<u16> = "/P /R /UPDATE /ARGS".encode_utf16().chain(Some(0)).collect();
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(file.as_ptr()), PCWSTR(parameters.as_ptr()), None, SW_SHOW);
    }
    std::process::exit(0);
}

// PORT STATUS: confidence=medium todos=0
