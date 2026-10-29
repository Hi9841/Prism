//! Launch latency of one app, through the same call that the palette makes.
//!
//! `cargo run --release --example launch_bench -- <apps.json> <app name or id> <window match> [reps] [gap ms]`
//!
//! Each launch runs `apps::launch` on the runtime's blocking pool, called from a
//! plain thread (the palette calls it from a GPUI pool thread). The bench then
//! waits for the first new visible top-level window whose process name or title
//! contains `<window match>` (case-insensitive), closes it with `WM_CLOSE`, and
//! waits `gap` ms. Each launch prints one JSON line:
//! `{"app", "rep", "callMs", "windowMs"}` (`windowMs` is null after 20 s).
//!
//! It launches real apps on the desktop: use apps that close cleanly.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use prism_core::apps::{self, AppEntry};
use prism_core::runtime::{block_on, spawn_blocking};
use serde_json::{json, Value};
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    PostMessageW, GW_OWNER, WM_CLOSE,
};

const WINDOW_TIMEOUT: Duration = Duration::from_secs(20);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [cache, name, matcher, rest @ ..] = args.as_slice() else {
        eprintln!("usage: launch_bench <apps.json> <app name> <window match> [reps] [gap ms]");
        std::process::exit(2);
    };
    let reps: usize = rest.first().map_or(5, |value| value.parse().expect("reps"));
    let gap = Duration::from_millis(
        rest.get(1)
            .map_or(1500, |value| value.parse().expect("gap ms")),
    );
    let matcher = matcher.to_lowercase();

    let text = std::fs::read_to_string(cache).expect("read apps.json");
    let index: Value = serde_json::from_str(&text).expect("parse apps.json");
    let apps: Vec<AppEntry> = serde_json::from_value(index["apps"].clone()).expect("apps list");
    let entry = apps
        .into_iter()
        .find(|app| app.name.eq_ignore_ascii_case(name) || app.app_id == *name)
        .unwrap_or_else(|| panic!("no app named {name}"));

    for rep in 0..reps {
        let before: HashSet<isize> = top_windows()
            .into_iter()
            .map(|hwnd| hwnd.0 as isize)
            .collect();
        let launch_entry = entry.clone();
        let launched_at = file_time_now();
        let started = Instant::now();
        // The palette's path: a pool thread blocks on the runtime, which runs
        // the launch on its blocking pool.
        let result = std::thread::spawn(move || {
            block_on(async move { spawn_blocking(move || apps::launch(&launch_entry)).await })
        })
        .join()
        .expect("launch thread");
        let call = started.elapsed();
        if let Err(error) = result.expect("launch task") {
            eprintln!("launch failed: {error}");
            std::process::exit(1);
        }
        let window = wait_for_window(&before, &matcher, started);
        println!(
            "{}",
            json!({
                "app": entry.name,
                "rep": rep,
                "callMs": ms(call),
                "windowMs": window.map(|(_, at)| ms(at)),
            })
        );
        if let Some((hwnd, _)) = window {
            close(hwnd, launched_at);
        }
        std::thread::sleep(gap);
    }
}

fn ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}

fn wait_for_window(
    before: &HashSet<isize>,
    matcher: &str,
    started: Instant,
) -> Option<(HWND, Duration)> {
    while started.elapsed() < WINDOW_TIMEOUT {
        for hwnd in top_windows() {
            if !before.contains(&(hwnd.0 as isize)) && matches(hwnd, matcher) {
                return Some((hwnd, started.elapsed()));
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

/// Visible, uncloaked, unowned top-level windows that have a title.
fn top_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, list: LPARAM) -> BOOL {
        let list = unsafe { &mut *(list.0 as *mut Vec<HWND>) };
        list.push(hwnd);
        BOOL(1)
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut Vec<HWND> as isize));
    }
    all.retain(|&hwnd| unsafe {
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        );
        IsWindowVisible(hwnd).as_bool()
            && cloaked == 0
            && GetWindow(hwnd, GW_OWNER).is_err()
            && !title(hwnd).is_empty()
    });
    all
}

fn title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn process_id(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn process_name(pid: u32) -> String {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        if ok.is_err() {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        path.rsplit('\\').next().unwrap_or_default().to_lowercase()
    }
}

fn matches(hwnd: HWND, matcher: &str) -> bool {
    process_name(process_id(hwnd)).contains(matcher) || title(hwnd).to_lowercase().contains(matcher)
}

/// Now, as a FILETIME count (100 ns steps since 1601).
fn file_time_now() -> u64 {
    const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;
    let since_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_AS_FILETIME + (since_unix.as_nanos() / 100) as u64
}

/// When the process started, as a FILETIME count; 0 when unknown.
fn process_started(pid: u32) -> u64 {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return 0;
        };
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let ok = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        if ok.is_err() {
            return 0;
        }
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)
    }
}

/// Closes the window and waits for it to go. A process that the launch
/// started and that outlives its window by 5 s is ended. A process from
/// before the launch is never ended: File Explorer windows belong to the
/// shell (`explorer.exe`), and packaged apps' frames to their host.
fn close(hwnd: HWND, launched_at: u64) {
    let pid = process_id(hwnd);
    let ours = process_started(pid) >= launched_at;
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        let deadline = Instant::now() + Duration::from_secs(5);
        while IsWindow(Some(hwnd)).as_bool() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !ours {
            return;
        }
        if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) {
            if WaitForSingleObject(process, 5_000).0 != 0 {
                let _ = TerminateProcess(process, 1);
            }
            let _ = CloseHandle(process);
        }
    }
}
