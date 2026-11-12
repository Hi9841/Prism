//! Always-on record of a stalled UI thread, as the user sees it: a palette
//! toggle (Win key, taskbar click, shortcut) that the UI thread does not take
//! within `STALL_AFTER`. While the toggle waits, a watchdog thread samples the
//! UI thread's scheduler state and wait reason, and when the UI thread takes
//! the toggle (or `MAX_WATCH` passes) it appends one line to
//! `%LOCALAPPDATA%\Prism\logs\ui-stall.log`:
//!
//! - `uiThread`: how often each state was seen (`Waiting/WrPageIn`: page
//!   faults; `Ready`: no free CPU; `Running`: own work; `Waiting/...`: a lock
//!   or a call into another process).
//! - `hardFaults`, `readMb`: the process's page reads and file reads meanwhile.
//! - `uiAt`: where the UI thread's instruction pointer was (module+offset):
//!   a system call stub names the call it waits in; an offset in prism.exe
//!   resolves with the build's pdb.
//! - `busyThreads`: the Prism threads that used the most CPU meanwhile.
//!
//! A stack sampler outside the app needed a reproduction to see this; the
//! freeze is rare and needs a heavy machine load, so the app records it
//! itself. The watchdog sleeps until a toggle is sent: no cost while idle.

use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, HMODULE, LocalFree};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, GetThreadDescription, OpenThread, ResumeThread, SuspendThread,
    THREAD_GET_CONTEXT, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};
use windows::core::PCWSTR;

/// A toggle that waits longer than this is a stall: a healthy UI thread takes
/// it in well under a frame.
const STALL_AFTER: Duration = Duration::from_millis(250);
const SAMPLE_EVERY: Duration = Duration::from_millis(50);
/// The watch ends here even if the UI thread never takes the toggle.
const MAX_WATCH: Duration = Duration::from_secs(30);
const BUSY_THREADS: usize = 4;
const UI_PLACES: usize = 5;

static UI_THREAD: AtomicU32 = AtomicU32::new(0);
static VERSION: OnceLock<&'static str> = OnceLock::new();
static WATCH: OnceLock<Watch> = OnceLock::new();

#[derive(Clone, Copy, PartialEq)]
struct Pending {
    at: Instant,
    input: &'static str,
}

struct Watch {
    pending: Mutex<Option<Pending>>,
    changed: Condvar,
}

/// Call once on the UI thread before it runs.
pub fn init(version: &'static str) {
    UI_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    let _ = VERSION.set(version);
}

/// A toggle went to the UI thread (any thread). Only the oldest waiting toggle
/// counts: presses that queue behind it are part of the same stall.
pub fn input_sent(input: &'static str) {
    if UI_THREAD.load(Ordering::Acquire) == 0 {
        return;
    }
    let watch = WATCH.get_or_init(|| {
        let spawned = std::thread::Builder::new().name("prism-ui-stall".into()).spawn(run);
        if spawned.is_err() {
            eprintln!("[Prism] cannot start the UI stall watchdog");
        }
        Watch { pending: Mutex::new(None), changed: Condvar::new() }
    });
    let mut pending = watch.pending.lock().unwrap_or_else(|e| e.into_inner());
    if pending.is_none() {
        *pending = Some(Pending { at: Instant::now(), input });
        watch.changed.notify_all();
    }
}

/// The UI thread took a toggle.
pub fn input_handled() {
    if let Some(watch) = WATCH.get() {
        let mut pending = watch.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.take().is_some() {
            watch.changed.notify_all();
        }
    }
}

fn run() {
    // `input_sent` sets WATCH right after the spawn; wait for it.
    let watch = loop {
        if let Some(watch) = WATCH.get() {
            break watch;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut buffer = Vec::new();
    loop {
        let current = {
            let mut pending = watch.pending.lock().unwrap_or_else(|e| e.into_inner());
            while pending.is_none() {
                pending = watch.changed.wait(pending).unwrap_or_else(|e| e.into_inner());
            }
            pending.unwrap()
        };
        if !wait_while_pending(watch, current, current.at + STALL_AFTER) {
            continue;
        }
        let stall = observe(watch, current, &mut buffer);
        if let Some(dir) = crate::logs::dir() {
            crate::logs::append(&dir, "ui-stall", &stall);
        }
    }
}

/// Waits until `until` while `current` is still the waiting toggle. True when
/// it still waits at `until`.
fn wait_while_pending(watch: &Watch, current: Pending, until: Instant) -> bool {
    let mut pending = watch.pending.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if *pending != Some(current) {
            return false;
        }
        let now = Instant::now();
        if now >= until {
            return true;
        }
        pending = watch
            .changed
            .wait_timeout(pending, until - now)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Stall {
    unix_ms: u128,
    version: &'static str,
    input: &'static str,
    delay_ms: u64,
    /// False when the UI thread had not taken the toggle after `MAX_WATCH`.
    taken: bool,
    ui_thread: Vec<StateCount>,
    ui_at: Vec<StateCount>,
    hard_faults: u32,
    page_faults: u32,
    read_mb: f64,
    working_set_mb: u64,
    busy_threads: Vec<BusyThread>,
}

#[derive(Serialize)]
struct StateCount {
    state: String,
    samples: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BusyThread {
    name: String,
    cpu_ms: u64,
}

fn observe(watch: &Watch, current: Pending, buffer: &mut Vec<u8>) -> Stall {
    let pid = unsafe { GetCurrentProcessId() };
    let ui = UI_THREAD.load(Ordering::Acquire);
    let first = process_info(pid, buffer);
    let mut states: BTreeMap<String, u32> = BTreeMap::new();
    let mut places: BTreeMap<u64, u32> = BTreeMap::new();
    // SAFETY: a handle to a thread of this process; closed below. Never this
    // thread: a thread that suspends itself does not wake.
    let thread = (ui != unsafe { GetCurrentThreadId() })
        .then(|| unsafe {
            OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION, false, ui).ok()
        })
        .flatten();
    let taken = loop {
        if let Some(thread) = process_info(pid, buffer).and_then(|info| info.threads.into_iter().find(|t| t.tid == ui)) {
            *states.entry(thread.state_label()).or_default() += 1;
        }
        if let Some(address) = thread.and_then(instruction_pointer) {
            *places.entry(address).or_default() += 1;
        }
        let elapsed = current.at.elapsed();
        if elapsed >= MAX_WATCH {
            break false;
        }
        if !wait_while_pending(watch, current, Instant::now() + SAMPLE_EVERY.min(MAX_WATCH - elapsed)) {
            break true;
        }
    };
    let delay_ms = current.at.elapsed().as_millis() as u64;
    if let Some(thread) = thread {
        let _ = unsafe { CloseHandle(thread) };
    }
    let last = process_info(pid, buffer);
    let mut ui_at: Vec<StateCount> = places
        .into_iter()
        .map(|(address, samples)| StateCount { state: module_offset(address), samples })
        .collect();
    ui_at.sort_by(|a, b| b.samples.cmp(&a.samples));
    ui_at.truncate(UI_PLACES);
    let mut ui_thread: Vec<StateCount> = states
        .into_iter()
        .map(|(state, samples)| StateCount { state, samples })
        .collect();
    ui_thread.sort_by(|a, b| b.samples.cmp(&a.samples));
    let mut stall = Stall {
        unix_ms: crate::logs::unix_ms(),
        version: VERSION.get().copied().unwrap_or_default(),
        input: current.input,
        delay_ms,
        taken,
        ui_thread,
        ui_at,
        hard_faults: 0,
        page_faults: 0,
        read_mb: 0.0,
        working_set_mb: 0,
        busy_threads: Vec::new(),
    };
    if let (Some(first), Some(last)) = (first, last) {
        stall.hard_faults = last.hard_faults.wrapping_sub(first.hard_faults);
        stall.page_faults = last.page_faults.wrapping_sub(first.page_faults);
        stall.read_mb = (last.read_bytes - first.read_bytes).max(0) as f64 / 1_048_576.0;
        stall.working_set_mb = (last.working_set / 1_048_576) as u64;
        let before: HashMap<u32, i64> = first.threads.iter().map(|t| (t.tid, t.cpu_100ns)).collect();
        let mut busy: Vec<(u32, i64)> = last
            .threads
            .iter()
            .map(|t| (t.tid, t.cpu_100ns - before.get(&t.tid).copied().unwrap_or(0)))
            .filter(|&(_, cpu)| cpu > 0)
            .collect();
        busy.sort_by(|a, b| b.1.cmp(&a.1));
        stall.busy_threads = busy
            .into_iter()
            .take(BUSY_THREADS)
            .map(|(tid, cpu)| BusyThread {
                name: if tid == ui { "ui".into() } else { thread_name(tid) },
                cpu_ms: (cpu / 10_000) as u64,
            })
            .collect();
    }
    stall
}

fn thread_name(tid: u32) -> String {
    unsafe {
        let Ok(thread) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) else {
            return format!("thread {tid}");
        };
        let name = match GetThreadDescription(thread) {
            Ok(text) => {
                let name = text.to_string().unwrap_or_default();
                let _ = LocalFree(Some(HLOCAL(text.0 as *mut c_void)));
                name
            }
            Err(_) => String::new(),
        };
        let _ = CloseHandle(thread);
        if name.is_empty() { format!("thread {tid}") } else { name }
    }
}

/// The thread's instruction pointer. The thread stops for the read only: the
/// buffer exists before the suspend, and nothing between the suspend and the
/// resume allocates or takes a lock (a lock that the stopped thread holds
/// would deadlock this thread and then the app).
#[cfg(target_arch = "x86_64")]
fn instruction_pointer(thread: HANDLE) -> Option<u64> {
    // The x64 `CONTEXT`: 1232 bytes, 16-byte aligned; `ContextFlags` at 0x30,
    // `Rip` at 0xF8.
    #[repr(C, align(16))]
    struct Context([u8; 1232]);
    const CONTEXT_CONTROL_AMD64: u32 = 0x0010_0001;
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetThreadContext(thread: HANDLE, context: *mut Context) -> i32;
    }
    let mut context = Box::new(Context([0; 1232]));
    context.0[0x30..0x34].copy_from_slice(&CONTEXT_CONTROL_AMD64.to_le_bytes());
    // SAFETY: `thread` has suspend and context rights; the context buffer is
    // aligned and large enough; the thread resumes on every path.
    unsafe {
        if SuspendThread(thread) == u32::MAX {
            return None;
        }
        let read = GetThreadContext(thread, &mut *context);
        ResumeThread(thread);
        if read == 0 {
            return None;
        }
    }
    Some(u64::from_le_bytes(context.0[0xF8..0x100].try_into().ok()?))
}

#[cfg(not(target_arch = "x86_64"))]
fn instruction_pointer(_thread: HANDLE) -> Option<u64> {
    None
}

/// `module+0xoffset` for an address, read after the thread runs again.
fn module_offset(address: u64) -> String {
    let mut module = HMODULE::default();
    // SAFETY: FROM_ADDRESS reads the address as a pointer into a module; the
    // reference count does not change, so nothing needs a release.
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(address as usize as *const u16),
            &mut module,
        )
        .is_ok()
    };
    if !found {
        return format!("0x{address:x}");
    }
    let mut path = [0u16; 260];
    let length = unsafe { GetModuleFileNameW(Some(module), &mut path) } as usize;
    let path = String::from_utf16_lossy(&path[..length.min(path.len())]);
    let name = path.rsplit('\\').next().unwrap_or_default();
    format!("{name}+0x{:x}", address - module.0 as u64)
}

// --- `NtQuerySystemInformation(SystemProcessInformation)` ---------------
//
// The only API that gives another thread's scheduler state and wait reason
// without a debugger. The layouts are the x64 `SYSTEM_PROCESS_INFORMATION` and
// `SYSTEM_THREAD_INFORMATION`; the offsets are stable since Windows 7.

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buffer: *mut c_void, length: u32, returned: *mut u32) -> i32;
}

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
const PROCESS_ENTRY: usize = 256;
const THREAD_ENTRY: usize = 80;

struct ProcessInfo {
    hard_faults: u32,
    page_faults: u32,
    working_set: usize,
    read_bytes: i64,
    threads: Vec<ThreadInfo>,
}

struct ThreadInfo {
    tid: u32,
    state: u32,
    wait_reason: u32,
    cpu_100ns: i64,
}

impl ThreadInfo {
    fn state_label(&self) -> String {
        const STATES: [&str; 8] =
            ["Initialized", "Ready", "Running", "Standby", "Terminated", "Waiting", "Transition", "DeferredReady"];
        let state = STATES.get(self.state as usize).copied().unwrap_or("Other");
        if self.state == 5 {
            format!("{state}/{}", wait_reason_name(self.wait_reason))
        } else {
            state.to_string()
        }
    }
}

fn wait_reason_name(reason: u32) -> &'static str {
    const NAMES: [&str; 42] = [
        "Executive", "FreePage", "PageIn", "PoolAllocation", "DelayExecution", "Suspended", "UserRequest",
        "WrExecutive", "WrFreePage", "WrPageIn", "WrPoolAllocation", "WrDelayExecution", "WrSuspended",
        "WrUserRequest", "WrEventPair", "WrQueue", "WrLpcReceive", "WrLpcReply", "WrVirtualMemory",
        "WrPageOut", "WrRendezvous", "WrKeyedEvent", "WrTerminated", "WrProcessInSwap", "WrCpuRateControl",
        "WrCalloutStack", "WrKernel", "WrResource", "WrPushLock", "WrMutex", "WrQuantumEnd", "WrDispatchInt",
        "WrPreempted", "WrYieldExecution", "WrFastMutex", "WrGuardedMutex", "WrRundown", "WrAlertByThreadId",
        "WrDeferredPreempt", "WrPhysicalFault", "WrIoRing", "WrMdlCache",
    ];
    NAMES.get(reason as usize).copied().unwrap_or("Unknown")
}

fn read<T: Copy>(buffer: &[u8], offset: usize) -> Option<T> {
    let end = offset.checked_add(std::mem::size_of::<T>())?;
    let bytes = buffer.get(offset..end)?;
    // SAFETY: `bytes` holds `size_of::<T>()` initialized bytes; the read is
    // unaligned, and `T` is a plain integer.
    Some(unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const T) })
}

fn process_info(pid: u32, buffer: &mut Vec<u8>) -> Option<ProcessInfo> {
    if buffer.len() < (1 << 20) {
        buffer.resize(1 << 20, 0);
    }
    let returned = loop {
        let mut returned = 0u32;
        // SAFETY: the buffer is valid for `len` bytes; the call writes at most that.
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_INFORMATION,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut returned,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH && buffer.len() < (64 << 20) {
            let grown = (returned as usize).max(buffer.len()) * 2;
            buffer.resize(grown, 0);
            continue;
        }
        if status < 0 {
            return None;
        }
        break (returned as usize).min(buffer.len());
    };
    let buffer = &buffer[..returned];
    let mut offset = 0usize;
    loop {
        let next: u32 = read(buffer, offset)?;
        let count: u32 = read(buffer, offset + 4)?;
        let id: usize = read(buffer, offset + 80)?;
        if id as u32 == pid {
            let threads = (0..count as usize)
                .filter_map(|index| {
                    let base = offset + PROCESS_ENTRY + index * THREAD_ENTRY;
                    let kernel: i64 = read(buffer, base)?;
                    let user: i64 = read(buffer, base + 8)?;
                    Some(ThreadInfo {
                        tid: read::<usize>(buffer, base + 48)? as u32,
                        state: read(buffer, base + 68)?,
                        wait_reason: read(buffer, base + 72)?,
                        cpu_100ns: kernel + user,
                    })
                })
                .collect();
            return Some(ProcessInfo {
                hard_faults: read(buffer, offset + 16)?,
                page_faults: read(buffer, offset + 128)?,
                working_set: read(buffer, offset + 144)?,
                read_bytes: read(buffer, offset + 232)?,
                threads,
            });
        }
        if next == 0 {
            return None;
        }
        offset += next as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_process_and_finds_the_calling_thread() {
        let mut buffer = Vec::new();
        let info = process_info(unsafe { GetCurrentProcessId() }, &mut buffer).expect("own process");
        let me = unsafe { GetCurrentThreadId() };
        let thread = info.threads.iter().find(|t| t.tid == me).expect("calling thread listed");
        // The calling thread runs while it asks.
        assert_eq!(thread.state_label(), "Running");
        assert!(info.working_set > 0);
    }

    #[test]
    fn a_blocked_thread_shows_its_wait_and_a_named_thread_its_name() {
        let (sender, receiver) = std::sync::mpsc::channel::<()>();
        let (tid_sender, tid_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("ui-stall-test".into())
            .spawn(move || {
                tid_sender.send(unsafe { GetCurrentThreadId() }).unwrap();
                let _ = receiver.recv();
            })
            .unwrap();
        let tid = tid_receiver.recv().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let mut buffer = Vec::new();
        let info = process_info(unsafe { GetCurrentProcessId() }, &mut buffer).unwrap();
        let thread = info.threads.iter().find(|t| t.tid == tid).unwrap();
        assert!(thread.state_label().starts_with("Waiting/"), "{}", thread.state_label());
        assert_eq!(thread_name(tid), "ui-stall-test");
        sender.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn a_toggle_taken_late_is_observed_until_taken() {
        let watch = Watch { pending: Mutex::new(None), changed: Condvar::new() };
        UI_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
        let current = Pending { at: Instant::now(), input: "test" };
        *watch.pending.lock().unwrap() = Some(current);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                watch.pending.lock().unwrap().take();
                watch.changed.notify_all();
            });
            let stall = observe(&watch, current, &mut Vec::new());
            assert!(stall.taken);
            assert!((150..2_000).contains(&stall.delay_ms), "{}", stall.delay_ms);
            assert!(stall.ui_thread.iter().map(|s| s.samples).sum::<u32>() >= 2);
        });
    }

    #[test]
    fn the_instruction_pointer_of_a_waiting_thread_is_in_a_system_module() {
        let (sender, receiver) = std::sync::mpsc::channel::<()>();
        let (tid_sender, tid_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tid_sender.send(unsafe { GetCurrentThreadId() }).unwrap();
            let _ = receiver.recv();
        });
        let tid = tid_receiver.recv().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT, false, tid).unwrap() };
        let address = instruction_pointer(thread).expect("context");
        let place = module_offset(address).to_ascii_lowercase();
        // A blocked receive waits in a system call stub.
        assert!(place.starts_with("ntdll.dll+") || place.starts_with("kernelbase.dll+"), "{place}");
        unsafe { CloseHandle(thread).unwrap() };
        sender.send(()).unwrap();
        worker.join().unwrap();
    }
}
