//! Headless file-churn benchmark: what the catalog costs while files change.
//!
//! `cargo run --release --example churn_bench -- <data folder with a catalog.db copy> <test tree> [--settle SECONDS] [--generator EXE]`
//!
//! The process opens the catalog copy, registers the test tree as a volume of
//! its own (`FileIndex::sync_volume`: the real watcher, flusher, and scope
//! repairs), and reports as the hidden app does (`Host::palette_visible` is
//! false). A child process (`--generate`) then changes files in the tree the
//! way a build, a package install, and a clean do. The file writes of the
//! child do not count in this process: its CPU time and its I/O are the
//! catalog's cost. The window ends a fixed time after the child (`--settle`).
//! A second kind of child (`--verify`) compares the catalog rows of the tree
//! with the disk once a second: the time to the first match is the freshness,
//! and a difference at the end is an error (exit code 1).
//!
//! With `--recording <file>` the child replays recorded events instead (the
//! output of `oploop/bench/fsevents.py record`, cut to the slice to replay):
//! each added, removed and renamed entry at its recorded time, below
//! `<tree>\work`. The entries that the slice removes or renames but does not
//! add are made before the watcher starts. A path counts as a folder when
//! another recorded path lies below it.
//!
//! The last line is a JSON summary. At `MARK <phase>` lines the process waits
//! for one line on stdin, so a runner can sample it from outside.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use prism_core::catalog::types::VolumeInfo;
use prism_core::catalog::FileIndex;
use windows::Win32::Foundation::{FILETIME, POINT, RECT};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS,
};

const VOLUME_ID: &str = "churn-bench";
const DRIVE_FIXED: u32 = 3;

struct BenchHost {
    data_dir: PathBuf,
    events: AtomicU64,
}

impl prism_core::host::Host for BenchHost {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {
        self.events.fetch_add(1, Ordering::Relaxed);
    }
    fn app_data_dir(&self) -> Result<PathBuf, String> {
        Ok(self.data_dir.clone())
    }
    fn palette_visible(&self) -> bool {
        false
    }
    fn toggle_from_win(&self, _start_button: Option<RECT>) {}
    fn toggle_from_taskbar(&self, _click: POINT, _start_button: Option<RECT>) {}
    fn activate(&self) {}
    fn register_shortcut(&self, _combo: &str) -> Result<(), String> {
        Ok(())
    }
    fn unregister_shortcut(&self, _combo: &str) {}
}

/// Kernel and user CPU time of the process, in milliseconds.
fn cpu_ms() -> (f64, f64) {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the pseudo handle of the current process and valid out pointers.
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user);
    }
    let ticks = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime)) as f64;
    (ticks(kernel) / 10_000.0, ticks(user) / 10_000.0)
}

fn io() -> IO_COUNTERS {
    let mut counters = IO_COUNTERS::default();
    // SAFETY: the pseudo handle of the current process and a valid out pointer.
    unsafe {
        let _ = GetProcessIoCounters(GetCurrentProcess(), &mut counters);
    }
    counters
}

/// Returns when the process used less than 10 ms of CPU in half a second,
/// twice in a row (the name index load and the first walk are over).
fn wait_quiet() {
    let mut quiet = 0;
    let mut last = cpu_ms();
    while quiet < 2 {
        std::thread::sleep(Duration::from_millis(500));
        let now = cpu_ms();
        quiet = if (now.0 + now.1) - (last.0 + last.1) < 10.0 { quiet + 1 } else { 0 };
        last = now;
    }
}

fn mark(phase: &str) {
    println!("MARK {phase}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
}

fn write_file(path: &Path) {
    std::fs::write(path, b"0123456789abcdef").unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn make_dir(path: &Path) {
    std::fs::create_dir_all(path).unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// A fixed number per item, so every run makes the same names.
fn tag(number: u64) -> u64 {
    number.wrapping_add(1).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(23) ^ 0x5851_f42d_4c95_7f2d
}

/// One compiler session folder: 20 files in `...-working`, then the folder
/// gets its final name (rustc does this at the end of each crate).
fn session(root: &Path, name: &str, number: u64, round: u64) -> PathBuf {
    let working = root.join(format!("s-h{number:05}r{round}-working"));
    make_dir(&working);
    for file in ["dep-graph.bin", "query-cache.bin", "work-products.bin"] {
        write_file(&working.join(file));
    }
    for object in 0..17 {
        write_file(&working.join(format!("{name}.{:08x}.cgu.{object:02}.o", tag(number * 31 + object) as u32)));
    }
    pause(10);
    let done = root.join(format!("s-h{number:05}r{round}-{:08x}", tag(number + round * 977) as u32));
    std::fs::rename(&working, &done).expect("rename the session folder");
    done
}

/// The file changes of a build, a package install, and a clean, under `<tree>\work`.
fn generate(tree: &Path) {
    let work = tree.join("work");
    let debug = work.join("target").join("debug");
    for dir in ["deps", "build", "incremental", ".fingerprint"] {
        make_dir(&debug.join(dir));
    }
    // Documents and folders that later steps save again, download, and replace.
    let docs = work.join("docs");
    make_dir(&docs);
    for number in 0..100u64 {
        write_file(&docs.join(format!("report-{number:03}.txt")));
    }
    for number in 0..20u64 {
        let old = docs.join(format!("album-{number:02}"));
        make_dir(&old);
        for track in 0..5 {
            write_file(&old.join(format!("track-{track}.flac")));
        }
    }
    pause(1_500);
    // A build of 150 crates: fingerprints, outputs, a short-lived temp folder,
    // and a session folder that is renamed when the crate is done.
    let mut sessions = Vec::new();
    for number in 0..150u64 {
        let name = format!("crate{number:03}");
        let hash = format!("{:016x}", tag(number));
        let fingerprint = debug.join(".fingerprint").join(format!("{name}-{hash}"));
        make_dir(&fingerprint);
        for file in ["invoked.timestamp", "lib", "lib.json", "dep-lib"] {
            write_file(&fingerprint.join(format!("{file}-{name}")));
        }
        let temp = debug.join("deps").join(format!("rmeta{}", &hash[..6]));
        make_dir(&temp);
        write_file(&temp.join("full.rmeta"));
        for extension in ["rmeta", "rlib", "d"] {
            write_file(&debug.join("deps").join(format!("lib{name}-{hash}.{extension}")));
        }
        let root = debug.join("incremental").join(format!("{name}-{hash}"));
        make_dir(&root);
        sessions.push((root.clone(), session(&root, &name, number, 0)));
        std::fs::remove_dir_all(&temp).expect("remove the temp folder");
        pause(10);
    }
    // A second build of 50 crates: a new session, and the old one goes.
    for number in 0..50u64 {
        let (root, old) = &sessions[number as usize];
        session(root, &format!("crate{number:03}"), number, 1);
        std::fs::remove_dir_all(old).expect("remove the old session");
        pause(10);
    }
    pause(1_000);
    for number in 0..100u64 {
        // A safe save: the new content goes to a temp file that replaces the document.
        let (document, temp) = (docs.join(format!("report-{number:03}.txt")), docs.join(format!("report-{number:03}.txt.tmp")));
        write_file(&temp);
        std::fs::rename(&temp, &document).expect("replace the document");
        // A download: the file gets its name when it is complete.
        let partial = docs.join(format!("setup-{number:03}.exe.partial"));
        write_file(&partial);
        std::fs::rename(&partial, docs.join(format!("setup-{number:03}.exe"))).expect("name the download");
        pause(5);
    }
    for number in 0..20u64 {
        // A folder is emptied and removed, and a new folder takes its name.
        let (old, new) = (docs.join(format!("album-{number:02}")), docs.join(format!("album-{number:02}.new")));
        make_dir(&new);
        for track in 0..5 {
            write_file(&new.join(format!("track-{track}.flac")));
        }
        std::fs::remove_dir_all(&old).expect("remove the old album");
        std::fs::rename(&new, &old).expect("name the new album");
        pause(5);
    }
    pause(1_000);
    // A package install: 1,200 packages of 24 files, as fast as the disk allows.
    let modules = work.join("node_modules");
    for package in 0..1_200u64 {
        let root = modules.join(format!("pkg{package:04}"));
        make_dir(&root.join("lib"));
        for file in ["package.json", "index.js", "README.md", "LICENSE"] {
            write_file(&root.join(file));
        }
        for file in 0..20 {
            write_file(&root.join("lib").join(format!("module{file:02}.js")));
        }
    }
    pause(3_000);
    // A clean: 1,000 of the packages and the fingerprints go.
    for package in 0..1_000u64 {
        std::fs::remove_dir_all(modules.join(format!("pkg{package:04}"))).expect("remove a package");
    }
    pause(1_000);
    std::fs::remove_dir_all(debug.join(".fingerprint")).expect("remove the fingerprints");
}

/// One recorded event: milliseconds from the start, the action, and the path below the watched folder.
struct Recorded {
    at_ms: f64,
    action: String,
    path: PathBuf,
}

fn read_recording(recording: &Path) -> Vec<Recorded> {
    std::fs::read_to_string(recording)
        .expect("read the recording")
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|event| {
            Some(Recorded {
                at_ms: event.get("t")?.as_f64()?,
                action: event.get("a")?.as_str()?.to_owned(),
                path: PathBuf::from(event.get("p")?.as_str()?),
            })
        })
        .collect()
}

/// The recorded paths that have another recorded path below them.
fn recorded_folders(events: &[Recorded]) -> std::collections::HashSet<PathBuf> {
    let mut folders = std::collections::HashSet::new();
    for event in events {
        let mut parent = event.path.parent();
        while let Some(folder) = parent.filter(|folder| !folder.as_os_str().is_empty()) {
            if !folders.insert(folder.to_path_buf()) {
                break;
            }
            parent = folder.parent();
        }
    }
    folders
}

/// Makes what the slice needs before its first event: each entry that it
/// removes or renames but does not add, and each parent folder that it does
/// not add.
fn replay_setup(tree: &Path, recording: &Path) {
    let events = read_recording(recording);
    let folders = recorded_folders(&events);
    let work = tree.join("work");
    let mut made_in_slice = std::collections::HashSet::new();
    let mut needed: Vec<&Path> = Vec::new();
    for event in &events {
        match event.action.as_str() {
            "added" | "renamed_new" => {
                made_in_slice.insert(event.path.as_path());
            }
            _ if !made_in_slice.contains(event.path.as_path()) => needed.push(&event.path),
            _ => {}
        }
    }
    for folder in folders.iter().filter(|folder| !made_in_slice.contains(folder.as_path())) {
        make_dir(&work.join(folder));
    }
    for path in needed {
        let target = work.join(path);
        if folders.contains(path) {
            make_dir(&target);
        } else if !target.exists() {
            if let Some(parent) = target.parent() {
                make_dir(parent);
            }
            let _ = std::fs::File::create(&target);
        }
    }
}

/// Replays the recorded events at their recorded times. An operation that
/// fails (its parent is not there yet, the entry is already gone) is skipped,
/// as the event of an entry that the recording did not see whole.
fn replay(tree: &Path, recording: &Path) {
    let events = read_recording(recording);
    let folders = recorded_folders(&events);
    let work = tree.join("work");
    let started = Instant::now();
    let mut renamed_from: Option<PathBuf> = None;
    for event in &events {
        let due = Duration::from_secs_f64(event.at_ms / 1000.0);
        if let Some(wait) = due.checked_sub(started.elapsed()) {
            std::thread::sleep(wait);
        }
        let target = work.join(&event.path);
        match event.action.as_str() {
            "added" if folders.contains(&event.path) => {
                let _ = std::fs::create_dir(&target);
            }
            "added" => {
                let _ = std::fs::File::create(&target);
            }
            "removed" => {
                if std::fs::remove_file(&target).is_err() {
                    let _ = std::fs::remove_dir_all(&target);
                }
            }
            "renamed_old" => renamed_from = Some(target),
            "renamed_new" => {
                if let Some(from) = renamed_from.take() {
                    let _ = std::fs::rename(from, &target);
                }
            }
            _ => {}
        }
    }
}

/// Folder flag and lowercase parent of every entry under `root`, by lowercase path.
fn walk(root: &Path, entries: &mut BTreeMap<String, (bool, String)>) {
    let Ok(children) = std::fs::read_dir(root) else {
        return;
    };
    let parent = root.to_string_lossy().to_lowercase();
    for child in children.flatten() {
        let Ok(kind) = child.file_type() else {
            continue;
        };
        let path = child.path();
        entries.insert(path.to_string_lossy().to_lowercase(), (kind.is_dir(), parent.clone()));
        if kind.is_dir() {
            walk(&path, entries);
        }
    }
}

/// Prints the differences between the catalog rows of the tree and the disk
/// (path, folder flag, parent; the watcher does not follow sizes and write
/// times) and returns their count.
fn verify(data_dir: &Path, tree: &Path) -> usize {
    let mut disk = BTreeMap::new();
    walk(tree, &mut disk);
    let base = tree.to_string_lossy().to_lowercase();
    let (low, high) = (format!("{base}\\"), format!("{base}]"));
    let conn = rusqlite::Connection::open_with_flags(
        data_dir.join("catalog.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open catalog");
    let mut statement = conn
        .prepare(
            "SELECT normalized_path, is_directory, parent FROM files
             WHERE volume_id = ?1 AND normalized_path >= ?2 AND normalized_path < ?3;",
        )
        .expect("prepare");
    let catalog = statement
        .query_map([VOLUME_ID, &low, &high], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, i64>(1)? != 0, row.get::<_, String>(2)?.to_lowercase()),
            ))
        })
        .expect("query")
        .collect::<Result<BTreeMap<String, (bool, String)>, _>>()
        .expect("rows");
    let mut differences = 0;
    for (path, entry) in &disk {
        match catalog.get(path) {
            None => println!("MISSING {path}"),
            Some(stored) if stored != entry => println!("STALE {path}: catalog {stored:?}, disk {entry:?}"),
            Some(_) => continue,
        }
        differences += 1;
    }
    for path in catalog.keys().filter(|path| !disk.contains_key(*path)) {
        println!("EXTRA {path}");
        differences += 1;
    }
    println!("VERIFY {} {} {differences}", disk.len(), catalog.len());
    differences
}

/// Runs `--verify` in a child (its CPU time and reads stay out of this
/// process) and returns (entries on disk, differences, the first lines).
fn verify_in_child(data_dir: &Path, tree: &Path) -> (usize, usize, Vec<String>) {
    let output = Command::new(std::env::current_exe().expect("own path"))
        .arg("--verify")
        .arg(data_dir)
        .arg(tree)
        .output()
        .expect("run the verify child");
    let text = String::from_utf8_lossy(&output.stdout);
    let summary = text
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("VERIFY "))
        .map(|line| line.split(' ').filter_map(|part| part.parse().ok()).collect::<Vec<usize>>())
        .unwrap_or_default();
    let (Some(&entries), Some(&differences)) = (summary.first(), summary.get(2)) else {
        return (0, usize::MAX, vec!["the verify child gave no summary".into()]);
    };
    let lines = text.lines().filter(|line| !line.starts_with("VERIFY ")).take(8).map(str::to_owned).collect();
    (entries, differences, lines)
}

fn main() {
    let usage = "usage: churn_bench <data folder> <test tree> [--settle SECONDS] [--generator EXE] [--recording FILE]";
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("--generate") => return generate(Path::new(args.get(1).expect(usage))),
        Some("--replay") => {
            return replay(Path::new(args.get(1).expect(usage)), Path::new(args.get(2).expect(usage)))
        }
        Some("--verify") => {
            let differences = verify(Path::new(args.get(1).expect(usage)), Path::new(args.get(2).expect(usage)));
            std::process::exit(i32::from(differences > 0));
        }
        _ => {}
    }
    let mut settle = 30.0;
    let mut generator = std::env::current_exe().expect("own path");
    let mut recording: Option<PathBuf> = None;
    while args.len() > 2 {
        let value = args.pop().expect(usage);
        match args.pop().expect(usage).as_str() {
            "--settle" => settle = value.parse().expect(usage),
            "--generator" => generator = PathBuf::from(value),
            "--recording" => recording = Some(std::fs::canonicalize(&value).expect("recording")),
            _ => panic!("{usage}"),
        }
    }
    let [data_dir, tree] = args.as_slice() else { panic!("{usage}") };
    let data_dir = std::fs::canonicalize(data_dir).expect("data folder");
    // No `\\?\` prefix: the catalog stores plain paths.
    let data_dir = PathBuf::from(data_dir.to_string_lossy().trim_start_matches(r"\\?\"));
    make_dir(Path::new(tree));
    let tree = PathBuf::from(std::fs::canonicalize(tree).expect("tree").to_string_lossy().trim_start_matches(r"\\?\"));
    let work = tree.join("work");
    if work.exists() {
        std::fs::remove_dir_all(&work).expect("remove the old work folder");
    }
    // The watcher gives each top-level folder a recursive reader at its start.
    make_dir(&work);
    if let Some(recording) = &recording {
        replay_setup(&tree, recording);
    }

    let host = Arc::new(BenchHost { data_dir: data_dir.clone(), events: AtomicU64::new(0) });
    prism_core::host::install(host.clone());
    let index = FileIndex::default();
    assert!(index.init(&data_dir), "the catalog copy did not open");
    wait_quiet();
    let volume = VolumeInfo {
        volume_id: VOLUME_ID.into(),
        volume_guid_path: None,
        discovery_sources: Vec::new(),
        // No drive letter: the directory backend (watcher and sweeps), as on the owner's volumes.
        drive_letter: None,
        mount_paths: vec![tree.clone()],
        drive_type: DRIVE_FIXED,
        label: "bench".into(),
        fs_type: "NTFS".into(),
        accessible: true,
    };
    index.sync_volume(&volume, false);
    wait_quiet();
    let (_, start_differences, _) = verify_in_child(&data_dir, &tree);
    mark("ready");

    let (cpu_before, io_before, events_before) = (cpu_ms(), io(), host.events.load(Ordering::Relaxed));
    let started = Instant::now();
    let mut child = Command::new(&generator);
    match &recording {
        Some(recording) => child.arg("--replay").arg(&tree).arg(recording),
        None => child.arg("--generate").arg(&tree),
    };
    let status = child.status().expect("run the generator");
    assert!(status.success(), "the generator failed");
    let generated = Instant::now();
    let generator_ms = (generated - started).as_secs_f64() * 1000.0;

    let mut converged_ms = None;
    let deadline = generated + Duration::from_secs_f64(settle);
    while Instant::now() + Duration::from_secs(1) < deadline {
        std::thread::sleep(Duration::from_secs(1));
        if converged_ms.is_none() && verify_in_child(&data_dir, &tree).1 == 0 {
            converged_ms = Some((Instant::now() - generated).as_secs_f64() * 1000.0);
        }
    }
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    let (cpu_after, io_after) = (cpu_ms(), io());
    mark("settled");
    let (entries, differences, lines) = verify_in_child(&data_dir, &tree);
    for line in &lines {
        eprintln!("{line}");
    }
    println!(
        "{{\"kernel_ms\":{:.0},\"user_ms\":{:.0},\"write_mib\":{:.2},\"write_ops\":{},\"read_mib\":{:.2},\"read_ops\":{},\"generator_ms\":{generator_ms:.0},\"converged_ms\":{},\"entries\":{entries},\"differences\":{},\"start_differences\":{},\"events\":{}}}",
        cpu_after.0 - cpu_before.0,
        cpu_after.1 - cpu_before.1,
        (io_after.WriteTransferCount - io_before.WriteTransferCount) as f64 / 1_048_576.0,
        io_after.WriteOperationCount - io_before.WriteOperationCount,
        (io_after.ReadTransferCount - io_before.ReadTransferCount) as f64 / 1_048_576.0,
        io_after.ReadOperationCount - io_before.ReadOperationCount,
        converged_ms.map_or("null".to_string(), |ms| format!("{ms:.0}")),
        if differences == usize::MAX { -1 } else { differences as i64 },
        if start_differences == usize::MAX { -1 } else { start_differences as i64 },
        host.events.load(Ordering::Relaxed) - events_before,
    );
    // The watcher threads never end: leave without joining them.
    std::process::exit(i32::from(differences != 0));
}
