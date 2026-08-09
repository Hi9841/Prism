//! Environment-gated performance events for release profiling.

use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PerfEvent<'a> {
    unix_ms: u128,
    event: &'a str,
    duration_ms: f64,
    detail: String,
}

pub fn start() -> Option<Instant> {
    log_path().map(|_| Instant::now())
}

pub fn finish<F>(started: Option<Instant>, event: &str, detail: F)
where
    F: FnOnce() -> String,
{
    let (Some(started), Some(path)) = (started, log_path()) else {
        return;
    };
    write_event(path, event, started.elapsed(), detail());
}

fn log_path() -> Option<&'static PathBuf> {
    LOG_PATH
        .get_or_init(|| {
            std::env::var_os("PRISM_PERF_LOG")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .as_ref()
}

fn write_event(path: &PathBuf, event: &str, duration: Duration, detail: String) {
    let record = PerfEvent {
        unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        event,
        duration_ms: duration.as_secs_f64() * 1_000.0,
        detail,
    };
    let Ok(mut line) = serde_json::to_vec(&record) else {
        return;
    };
    line.push(b'\n');
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(&line);
    }
}
