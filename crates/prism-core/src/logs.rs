//! Always-on JSON-line logs in `%LOCALAPPDATA%\Prism\logs`. Each log keeps at
//! most `MAX_BYTES`; past that it moves to `<name>.old.log` and starts again.
//! File I/O: call these off the UI thread.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub(crate) const MAX_BYTES: u64 = 256 * 1024;

pub(crate) fn dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("Prism").join("logs"))
}

/// Appends `record` as one JSON line to `<dir>\<name>.log`.
pub(crate) fn append(dir: &Path, name: &str, record: &impl Serialize) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join(format!("{name}.log"));
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, dir.join(format!("{name}.old.log")));
    }
    let Ok(mut line) = serde_json::to_vec(record) else {
        return;
    };
    line.push(b'\n');
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(&line);
    }
}

pub(crate) fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
