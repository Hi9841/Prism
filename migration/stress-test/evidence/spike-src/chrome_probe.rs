//! Chromium baseline for the manual checks. Chrome 154 uses the same Chromium
//! major as the installed WebView2 runtime (154), so its wheel deltas, key values,
//! and prefers-reduced-motion mapping stand in for the original app's WebView.
//! The owner drives it by hand; nothing here injects input.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;

const PAGE: &str = r#"<!doctype html><meta charset=utf-8><title>Prism Step 2 Chromium baseline</title>
<style>body{font:14px 'Segoe UI';background:#16161c;color:#eee;margin:16px}
#log{white-space:pre;font:12px 'Cascadia Code',monospace;height:360px;overflow:auto;background:#0008;padding:8px}</style>
<ol><li>Hold Ctrl. Turn the mouse wheel one notch up, then one notch down.</li>
<li>Turn the wheel one notch without Ctrl.</li>
<li>Press Ctrl+=, Ctrl+Shift+=, Ctrl+-, Ctrl+NumpadAdd, Ctrl+0, Ctrl+Up.</li>
<li>Open Windows Settings, Accessibility, Visual effects. Turn "Animation effects" off, then on. Watch the line below.</li>
<li>Close this window.</li></ol>
<div id=rm></div><div id=log></div>
<script>
const log=o=>{const s=JSON.stringify(o);document.getElementById('log').textContent+=s+'\n';fetch('/log',{method:'POST',body:s});};
const mq=matchMedia('(prefers-reduced-motion: reduce)');
const rm=()=>{document.getElementById('rm').textContent='prefers-reduced-motion: reduce = '+mq.matches;log({event:'reduced-motion',reduce:mq.matches});};
mq.addEventListener('change',rm);rm();
addEventListener('wheel',e=>{if(e.ctrlKey)e.preventDefault();log({event:'wheel',ctrl:e.ctrlKey,deltaY:e.deltaY,deltaMode:e.deltaMode,dpr:devicePixelRatio});},{capture:true,passive:false});
addEventListener('keydown',e=>{if(e.ctrlKey)e.preventDefault();log({event:'keydown',key:e.key,code:e.code,ctrl:e.ctrlKey,shift:e.shiftKey,alt:e.altKey,isComposing:e.isComposing});},true);
log({event:'ready',ua:navigator.userAgent,dpr:devicePixelRatio,language:navigator.language});
</script>"#;

pub fn run() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; content_length];
            let _ = reader.read_exact(&mut body);
            let mut stream = reader.into_inner();
            if request_line.starts_with("POST /log") {
                println!("{}", String::from_utf8_lossy(&body));
                let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
            } else {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    PAGE.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(PAGE.as_bytes());
            }
        }
    });

    let profile = std::env::current_dir().unwrap().join("tmp").join("chrome-profile");
    let chrome = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
    let status = Command::new(chrome)
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(["--no-first-run", "--no-default-browser-check", "--window-size=720,620"])
        .arg(format!("--app=http://127.0.0.1:{port}/"))
        .status();
    eprintln!("chrome exited: {status:?}");
}
