//! Parity capture mode: `prism --parity <fixture.json> <script.json> <scenario> <out-dir>`.
//!
//! Runs the real views on fixture data, with no backend hooks (no Win key, no
//! taskbar), in a window that is never activated and sits off-screen. The
//! script drives the window through GPUI's normal input path and captures
//! PNGs with PrintWindow. `migration/parity/run.mjs` runs the same script
//! against the original in headless Chrome.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, AppContext, Application, AsyncApp, Bounds, Keystroke, Modifiers, Point, Task, Timer,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px, size,
};
use prism_ui::lib::bridge::{self, BackendEvent, BridgeResult, ShellHandle};
use prism_ui::lib::fixture::{self, Fixture};
use serde_json::Value;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen,
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDC, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsWindowVisible, PostMessageW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER,
    SetWindowPos, ShowWindow, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WM_RBUTTONUP,
};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_LBUTTON, MK_RBUTTON};

/// Posts a mouse message to this process's own window (client coordinates).
fn post_mouse(hwnd: HWND, message: u32, keys: usize, at: Point<gpui::Pixels>) {
    let (x, y) = (f32::from(at.x).round() as i32, f32::from(at.y).round() as i32);
    let lparam = ((x as u16 as u32) | ((y as u16 as u32) << 16)) as i32 as isize;
    unsafe {
        let _ = PostMessageW(Some(hwnd), message, WPARAM(keys), LPARAM(lparam));
    }
}

/// Off every monitor: captures never appear on the desktop.
const PARK_X: i32 = -4000;
const PARK_Y: i32 = 100;
const PW_RENDERFULLCONTENT: u32 = 2;

struct ParityShell {
    hwnd: Cell<isize>,
    width: Cell<i32>,
}

impl ParityShell {
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd.get() as *mut _)
    }

}

fn place(hwnd: HWND, width: i32) {
    unsafe {
        let _ = SetWindowPos(hwnd, None, PARK_X, PARK_Y, width, 620, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

impl bridge::Shell for ParityShell {
    fn present_palette(&self, cx: &mut App) -> BridgeResult<bool> {
        // Native moves run outside GPUI updates (see shell::GpuiShell::present).
        let hwnd = self.hwnd.get();
        let width = self.width.get();
        cx.spawn(async move |_: &mut AsyncApp| {
            place(HWND(hwnd as *mut _), width);
            unsafe {
                let _ = ShowWindow(HWND(hwnd as *mut _), SW_SHOWNOACTIVATE);
            }
        })
        .detach();
        Ok(true)
    }

    fn hide_palette(&self, _defer: bool, _cx: &mut App) -> BridgeResult<()> {
        unsafe {
            let _ = ShowWindow(self.hwnd(), SW_HIDE);
        }
        Ok(())
    }

    fn is_window_visible(&self, _cx: &mut App) -> bool {
        unsafe { IsWindowVisible(self.hwnd()) }.as_bool()
    }

    fn set_always_on_top(&self, _on: bool, _cx: &mut App) -> BridgeResult<()> {
        Ok(())
    }

    fn set_window_style(&self, _theme: &str, _cx: &mut App) -> BridgeResult<()> {
        Ok(())
    }

    fn set_window_width(&self, width: u32, cx: &mut App) -> BridgeResult<()> {
        self.width.set(width as i32);
        let hwnd = self.hwnd.get();
        cx.spawn(async move |_: &mut AsyncApp| place(HWND(hwnd as *mut _), width as i32)).detach();
        Ok(())
    }

    fn set_taskbar_alignment(&self, _alignment: &str, _cx: &mut App) -> BridgeResult<()> {
        Ok(())
    }

    fn start_file_drag(&self, _paths: Vec<String>, _cx: &mut App) -> Task<BridgeResult<bool>> {
        Task::ready(Ok(false))
    }

    fn quit(&self, _cx: &mut App) -> Task<BridgeResult<()>> {
        Task::ready(Ok(()))
    }

    fn app_version(&self) -> String {
        fixture::get().map(|f| f.version.clone()).unwrap_or_default()
    }

    fn window_activation_changed(&self, _active: bool, _cx: &mut App) {}
}

/// Captures the window's client area as RGBA through PrintWindow.
fn capture(hwnd: HWND) -> Result<image::RgbaImage, String> {
    unsafe {
        let mut rect = Default::default();
        GetWindowRect(hwnd, &mut rect).map_err(|e| e.to_string())?;
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).map_err(|e| e.to_string())?;
        let previous = SelectObject(dc, bitmap.into());
        let ok = PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
        let pixels = std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize).to_vec();
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        if !ok {
            return Err("PrintWindow failed".into());
        }
        let mut rgba = pixels;
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        image::RgbaImage::from_raw(width as u32, height as u32, rgba).ok_or_else(|| "bad capture size".into())
    }
}

fn key_event(window: &mut gpui::Window, spec: &str, cx: &mut App) {
    if let Ok(keystroke) = Keystroke::parse(spec) {
        window.dispatch_keystroke(keystroke, cx);
    }
}

fn point_of(value: &Value) -> Point<gpui::Pixels> {
    let x = value.get(0).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let y = value.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    point(px(x), px(y))
}

async fn run_steps(steps: Vec<Value>, window: AnyWindowHandle, shell: Rc<ParityShell>, out: PathBuf, cx: &mut AsyncApp) -> Result<(), String> {
    let mut mouse = point(px(-10.), px(-10.));
    for step in steps {
        if let Some(ms) = step.get("wait").and_then(Value::as_u64) {
            Timer::after(Duration::from_millis(ms)).await;
            continue;
        }
        if step.get("open").is_some() {
            cx.update(|cx| bridge::emit_event(BackendEvent::Toggle { open: true }, cx)).map_err(|e| e.to_string())?;
        } else if let Some(text) = step.get("type").and_then(Value::as_str) {
            let text = text.to_owned();
            cx.update_window(window, |_, window, cx| {
                for ch in text.chars() {
                    let keystroke = Keystroke {
                        modifiers: Modifiers::none(),
                        key: ch.to_lowercase().to_string(),
                        key_char: Some(ch.to_string()),
                    };
                    window.dispatch_keystroke(keystroke, cx);
                }
            })
            .map_err(|e| e.to_string())?;
        } else if let Some(spec) = step.get("key").and_then(Value::as_str) {
            let spec = spec.to_owned();
            cx.update_window(window, |_, window, cx| key_event(window, &spec, cx)).map_err(|e| e.to_string())?;
        } else if let Some(position) = step.get("move") {
            mouse = point_of(position);
            post_mouse(shell.hwnd(), WM_MOUSEMOVE, 0, mouse);
        } else if let Some(position) = step.get("click") {
            mouse = point_of(position);
            let right = step.get("button").and_then(Value::as_str) == Some("right");
            let (down, up, flag) =
                if right { (WM_RBUTTONDOWN, WM_RBUTTONUP, MK_RBUTTON.0) } else { (WM_LBUTTONDOWN, WM_LBUTTONUP, MK_LBUTTON.0) };
            post_mouse(shell.hwnd(), WM_MOUSEMOVE, 0, mouse);
            post_mouse(shell.hwnd(), down, flag as usize, mouse);
            Timer::after(Duration::from_millis(16)).await;
            post_mouse(shell.hwnd(), up, 0, mouse);
        } else if let Some(wheel) = step.get("wheel") {
            // Chromium deltaY in CSS px: one notch (120) is 100 px.
            let delta_y = wheel.get(2).and_then(Value::as_f64).unwrap_or(0.0);
            let ctrl = step.get("ctrl").and_then(Value::as_bool).unwrap_or(false);
            mouse = point_of(wheel);
            let notches = (-delta_y / 100.0 * 120.0).round() as i16;
            let mut screen = POINT { x: f32::from(mouse.x) as i32, y: f32::from(mouse.y) as i32 };
            unsafe {
                let _ = ClientToScreen(shell.hwnd(), &mut screen);
            }
            let keys = if ctrl { MK_CONTROL.0 as usize } else { 0 };
            let wparam = ((notches as u16 as usize) << 16) | keys;
            let lparam = ((screen.x as u16 as u32) | ((screen.y as u16 as u32) << 16)) as i32 as isize;
            unsafe {
                let _ = PostMessageW(Some(shell.hwnd()), WM_MOUSEWHEEL, WPARAM(wparam), LPARAM(lparam));
            }
        } else if step.get("info").is_some() {
            let hwnd = shell.hwnd();
            let mut window_rect = Default::default();
            let mut client_rect = Default::default();
            unsafe {
                let _ = GetWindowRect(hwnd, &mut window_rect);
                let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client_rect);
            }
            let viewport = cx
                .update_window(window, |_, window, _| (window.viewport_size(), window.bounds(), window.scale_factor()))
                .map_err(|e| e.to_string())?;
            eprintln!("parity-info window={window_rect:?} client={client_rect:?} gpui={viewport:?}");
        } else if let Some(name) = step.get("capture").and_then(Value::as_str) {
            // Let pending frames land.
            Timer::after(Duration::from_millis(120)).await;
            let image = capture(shell.hwnd())?;
            image.save(out.join(format!("{name}.png"))).map_err(|e| e.to_string())?;
        }
        let _ = mouse;
        Timer::after(Duration::from_millis(30)).await;
    }
    Ok(())
}

pub fn run(args: &[String]) {
    let [fixture_path, script_path, scenario_name, out_dir] = args else {
        eprintln!("usage: prism --parity <fixture.json> <script.json> <scenario> <out-dir>");
        std::process::exit(2);
    };
    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(fixture_path).expect("read fixture"))
        .expect("parse fixture");
    let script: Value = serde_json::from_str(&std::fs::read_to_string(script_path).expect("read script")).expect("parse script");
    let scenario = script
        .get("scenarios")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find(|s| s.get("name").and_then(Value::as_str) == Some(scenario_name.as_str())))
        .cloned()
        .expect("scenario");
    let backdrop = script.get("backdrop").and_then(Value::as_str).unwrap_or("#1c1d22").to_owned();
    let out = PathBuf::from(out_dir);
    std::fs::create_dir_all(&out).expect("create out dir");
    fixture::install(fixture);
    prism_ui::gfx::enable_parity(prism_ui::gfx::ParityOptions { backdrop: parse_hex(&backdrop) });

    Application::new().with_assets(prism_ui::gfx::Assets).run(move |cx: &mut App| {
        prism_ui::components::text_input::bind_keys(cx);
        bridge::install_event_hub(cx);
        let shell = Rc::new(ParityShell { hwnd: Cell::new(0), width: Cell::new(640) });
        cx.set_global(ShellHandle(shell.clone()));
        prism_ui::state::app::install(cx);
        prism_ui::state::palette::install(cx);
        let window_shell = shell.clone();
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(PARK_X as f32), px(PARK_Y as f32)),
                        size(px(720.), px(620.)),
                    ))),
                    titlebar: None,
                    kind: WindowKind::PopUp,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                move |window, cx| {
                    window_shell.hwnd.set(crate::hwnd_of(window).0 as isize);
                    cx.new(|cx| prism_ui::app::Launcher::new(window, cx))
                },
            )
            .expect("open window");
        crate::shell::remove_frame(shell.hwnd());
        let any_window: AnyWindowHandle = window.into();
        let steps = scenario.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
        cx.spawn(async move |cx| {
            // Settings load and the first layout.
            Timer::after(Duration::from_millis(400)).await;
            if let Err(error) = run_steps(steps, any_window, shell, out, cx).await {
                eprintln!("parity: {error}");
            }
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

fn parse_hex(hex: &str) -> gpui::Rgba {
    let value = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0x1c1d22);
    gpui::Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.0,
        g: ((value >> 8) & 0xff) as f32 / 255.0,
        b: (value & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}
