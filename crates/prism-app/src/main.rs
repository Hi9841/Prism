//! Prism with a native GPUI interface. Startup mirrors the Tauri app's
//! `run()`: single instance, persisted width, the backend, the hidden palette
//! window, and the first presentation unless launched for autostart.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod host;
mod hotkey;
mod parity;
mod shell;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt;
use gpui::{
    App, AppContext, Application, Bounds, Timer, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
    px, size,
};
use prism_core::backend::{autostart_disabled_by_user, backend, launched_for_autostart, load_state_value};
use prism_ui::lib::bridge::{self, BackendEvent, ShellHandle};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, INFINITE, ResetEvent, SetEvent, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
use windows::core::w;

use crate::host::{GpuiHost, HostMessage};
use crate::shell::{GpuiShell, LOGICAL_WIDTH};

const PALETTE_STARTUP_DELAY: Duration = Duration::from_millis(100);

/// Single instance: a second launch signals the first one to open the palette.
/// An autostart launch does not (`open_running` false): the sign-in task and
/// the Run entry both start Prism, and the later one must exit quietly.
fn claim_single_instance(open_running: bool) -> Option<HANDLE> {
    /// An instance that was just killed (an update, Task Manager) still owns
    /// the mutex until Windows tears the process down.
    const DYING_OWNER_WAIT_MS: u32 = 3000;
    unsafe {
        let activate = CreateEventW(None, false, false, w!("Local\\PrismGpuiActivate")).ok()?;
        let mutex = CreateMutexW(None, true, w!("Local\\PrismGpuiInstance")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Open the running palette at once; if the owner was only dying,
            // take over instead of exiting (no Prism would run otherwise).
            if open_running {
                // This launch came from the user (Start menu, a shortcut), so it may set the
                // foreground. Pass that right on, or the running palette opens without focus.
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = SetEvent(activate);
            }
            let waited = WaitForSingleObject(mutex, DYING_OWNER_WAIT_MS);
            if waited != WAIT_OBJECT_0 && waited != WAIT_ABANDONED {
                return None;
            }
            let _ = ResetEvent(activate);
        }
        Some(activate)
    }
}

pub fn hwnd_of(window: &gpui::Window) -> HWND {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => HWND(handle.hwnd.get() as *mut _),
        _ => HWND(std::ptr::null_mut()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--parity") {
        parity::run(&args[2..]);
        return;
    }
    if prism_core::backend::run_start_restore_watchdog_if_requested() {
        return;
    }
    let autostart = launched_for_autostart();
    // The sign-in task starts Prism even when Task Manager's switch is off.
    if autostart && autostart_disabled_by_user() {
        return;
    }
    let Some(activate_event) = claim_single_instance(!autostart) else { return };
    let startup_timer = prism_core::perf::start();
    let show_on_start = !autostart;

    let (sender, mut receiver) = futures::channel::mpsc::unbounded::<HostMessage>();
    let palette_visible = Arc::new(AtomicBool::new(false));
    let hotkeys = hotkey::Hotkeys::start(sender.clone());
    prism_core::host::install(Arc::new(GpuiHost {
        sender: sender.clone(),
        app_data_dir: host::app_data_dir(),
        hotkeys,
        palette_visible: palette_visible.clone(),
    }));

    // Second instances signal this event; open the palette for them.
    let activation_sender = sender.clone();
    let activate_raw = activate_event.0 as isize;
    std::thread::spawn(move || loop {
        let event = HANDLE(activate_raw as *mut _);
        if unsafe { WaitForSingleObject(event, INFINITE) } != WAIT_OBJECT_0 {
            return;
        }
        if activation_sender.unbounded_send(HostMessage::Activate).is_err() {
            return;
        }
    });

    Application::new().with_assets(prism_ui::gfx::Assets).run(move |cx: &mut App| {
        prism_ui::components::text_input::bind_keys(cx);
        bridge::install_event_hub(cx);
        let shell = Rc::new(GpuiShell::new(palette_visible));
        shell::set_instance(shell.clone());
        cx.set_global(ShellHandle(shell.clone()));

        let persisted = load_state_value().ok().flatten();
        if let Some(width) = persisted.as_ref().and_then(|v| v.pointer("/settings/width")).and_then(|v| v.as_u64())
            && (560..=720).contains(&width)
        {
            LOGICAL_WIDTH.store(width as u32, Ordering::Release);
        }

        prism_ui::state::app::install(cx);
        prism_ui::state::palette::install(cx);

        // tauri.conf.json: 720x620 (min 480x400), transparent, undecorated, no
        // taskbar entry (PopUp is a tool window), hidden, not resizable (A8).
        let window_size = size(px(720.), px(620.));
        let bounds = Bounds::centered(None, window_size, cx);
        let window_shell = shell.clone();
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    kind: WindowKind::PopUp,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    window_min_size: Some(size(px(480.), px(400.))),
                    focus: false,
                    show: false,
                    app_id: Some("app.prism.launcher".into()),
                    ..Default::default()
                },
                move |window, cx| {
                    window_shell.set_hwnd(hwnd_of(window));
                    cx.new(|cx| prism_ui::app::Launcher::new(window, cx))
                },
            )
            .expect("open the palette window");
        let _ = window;
        shell::remove_frame(HWND(shell.hwnd_raw() as *mut _));

        backend().start(persisted.as_ref());

        // Host messages arrive from backend threads; handle them on the UI thread.
        let message_shell = shell.clone();
        cx.spawn(async move |cx| {
            while let Some(message) = receiver.next().await {
                let shell = message_shell.clone();
                let _ = cx.update(|cx| match message {
                    HostMessage::Emit { event, payload } => {
                        let event = match event.as_str() {
                            "file-index-updated" => Some(BackendEvent::FileIndexUpdated),
                            "apps-updated" => Some(BackendEvent::AppsUpdated),
                            "system-theme-changed" => payload
                                .get("theme")
                                .and_then(|t| t.as_str())
                                .map(|t| BackendEvent::SystemThemeChanged(t.to_owned())),
                            "win-mode-failed" => {
                                Some(BackendEvent::WinModeFailed(payload.as_str().unwrap_or_default().to_owned()))
                            }
                            _ => None,
                        };
                        if let Some(event) = event {
                            bridge::emit_event(event, cx);
                        }
                    }
                    HostMessage::ToggleFromWin(rect) => shell.toggle_palette_from_win(rect, cx),
                    HostMessage::ToggleFromTaskbar(point, rect) => shell.toggle_palette_from_taskbar(point, rect, cx),
                    HostMessage::ToggleShortcut => shell.toggle_palette(cx),
                    HostMessage::Activate => shell.activate_palette(cx),
                });
            }
        })
        .detach();

        if show_on_start {
            let initial_shell = shell.clone();
            cx.spawn(async move |cx| {
                Timer::after(PALETTE_STARTUP_DELAY).await;
                let _ = cx.update(|cx| initial_shell.activate_palette(cx));
            })
            .detach();
        }
        prism_core::perf::finish(startup_timer, "startup_setup", String::new);
    });
}
