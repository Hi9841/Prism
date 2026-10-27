//! Global shortcuts for the non-Win combos (what tauri-plugin-global-shortcut
//! did): `RegisterHotKey` on a dedicated thread with its own message loop.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::UnboundedSender;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey, VK_RETURN,
    VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, PostThreadMessageW, WM_APP, WM_HOTKEY};

use crate::host::HostMessage;

const WM_REGISTER: u32 = WM_APP + 1;
const WM_UNREGISTER: u32 = WM_APP + 2;

enum Request {
    Register(String, mpsc::Sender<Result<(), String>>),
    Unregister(String),
}

#[derive(Clone)]
pub struct Hotkeys {
    thread_id: u32,
    requests: Arc<Mutex<mpsc::Sender<Request>>>,
}

/// "Ctrl+Alt+Shift+S" -> modifiers and virtual key.
fn parse(combo: &str) -> Result<(HOT_KEY_MODIFIERS, u32), String> {
    let mut modifiers = MOD_NOREPEAT;
    let mut key = None;
    for part in combo.split('+') {
        match part {
            "Ctrl" => modifiers |= MOD_CONTROL,
            "Alt" => modifiers |= MOD_ALT,
            "Shift" => modifiers |= MOD_SHIFT,
            "Space" => key = Some(VK_SPACE.0 as u32),
            "Enter" => key = Some(VK_RETURN.0 as u32),
            letter if letter.len() == 1 && letter.as_bytes()[0].is_ascii_alphabetic() => {
                key = Some(letter.as_bytes()[0].to_ascii_uppercase() as u32)
            }
            other => return Err(format!("invalid shortcut '{combo}': unknown key {other}")),
        }
    }
    key.map(|key| (modifiers, key)).ok_or_else(|| format!("invalid shortcut '{combo}'"))
}

impl Hotkeys {
    pub fn start(sender: UnboundedSender<HostMessage>) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<Request>();
        let (id_tx, id_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("prism-hotkeys".into())
            .spawn(move || {
                let _ = id_tx.send(unsafe { GetCurrentThreadId() });
                let mut registered: Vec<(String, i32)> = Vec::new();
                let mut next_id = 1;
                let mut msg = MSG::default();
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                    match msg.message {
                        WM_HOTKEY => {
                            let _ = sender.unbounded_send(HostMessage::ToggleShortcut);
                        }
                        WM_REGISTER | WM_UNREGISTER => {
                            while let Ok(request) = request_rx.try_recv() {
                                match request {
                                    Request::Register(combo, reply) => {
                                        let result = parse(&combo).and_then(|(modifiers, key)| {
                                            let id = next_id;
                                            unsafe { RegisterHotKey(None, id, modifiers, key) }
                                                .map_err(|e| format!("failed to register shortcut: {e}"))?;
                                            next_id += 1;
                                            registered.push((combo.clone(), id));
                                            Ok(())
                                        });
                                        let _ = reply.send(result);
                                    }
                                    Request::Unregister(combo) => {
                                        registered.retain(|(c, id)| {
                                            if *c == combo {
                                                unsafe {
                                                    let _ = UnregisterHotKey(None, *id);
                                                }
                                                false
                                            } else {
                                                true
                                            }
                                        });
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            })
            .expect("start the hotkey thread");
        let thread_id = id_rx.recv().unwrap_or(0);
        Self { thread_id, requests: Arc::new(Mutex::new(request_tx)) }
    }

    pub fn register(&self, combo: &str) -> Result<(), String> {
        parse(combo)?;
        let (reply_tx, reply_rx) = mpsc::channel();
        self.requests
            .lock()
            .map_err(|e| e.to_string())?
            .send(Request::Register(combo.to_owned(), reply_tx))
            .map_err(|e| e.to_string())?;
        unsafe { PostThreadMessageW(self.thread_id, WM_REGISTER, WPARAM(0), LPARAM(0)) }.map_err(|e| e.to_string())?;
        reply_rx.recv().map_err(|e| e.to_string())?
    }

    pub fn unregister(&self, combo: &str) {
        if let Ok(requests) = self.requests.lock() {
            let _ = requests.send(Request::Unregister(combo.to_owned()));
        }
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_UNREGISTER, WPARAM(0), LPARAM(0));
        }
    }
}
