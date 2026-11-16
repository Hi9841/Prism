//! Runs the real file index (catalog v2) on this PC with its own data folder,
//! and checks it against the disk.
//!
//! `index_probe <data folder> <churn folder> [churn command ...]`
//! 1. Starts the index (`warm`); waits until it is ready; prints the time.
//! 2. Runs the churn command (if given), waits for the journal reader.
//! 3. Compares the entries below the churn folder with a `std::fs` walk.
//!
//! Run it twice: the second run loads the snapshot and replays the journal.
//! Exit code 2 when the index and the disk differ.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use prism_core::catalog::{warm, FileIndex};

/// Folder names that the default exclusion list leaves out (names only; the
/// churn folder holds no excluded absolute path).
const EXCLUDED_NAMES: &[&str] = &["node_modules", ".git", "__pycache__", ".gradle", "$recycle.bin"];

fn disk_paths(root: &Path) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let is_dir = entry.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false);
            if is_dir && EXCLUDED_NAMES.contains(&name.as_str()) {
                continue;
            }
            paths.insert(path.to_string_lossy().to_lowercase());
            if is_dir {
                stack.push(path);
            }
        }
    }
    paths
}

fn wait_ready(index: &FileIndex, started: Instant) {
    let mut last_print = Instant::now();
    loop {
        let generation = index.begin_search();
        let response = index.search_with_generation("zzqqxxprobe", Some(1), generation);
        if response.ready && !response.indexing && response.total_indexed > 0 {
            println!(
                "ready in {:.1} s: {} entries, volumes {:?}",
                started.elapsed().as_secs_f64(),
                response.total_indexed,
                response.volumes.iter().map(|v| (v.drive.clone(), v.indexed_count)).collect::<Vec<_>>()
            );
            return;
        }
        if last_print.elapsed() > Duration::from_secs(10) {
            last_print = Instant::now();
            println!("  ... {:.0} s, {} entries", started.elapsed().as_secs_f64(), response.total_indexed);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [data, churn, command @ ..] = args.as_slice() else {
        eprintln!("usage: index_probe <data folder> <churn folder> [churn command ...]");
        std::process::exit(1);
    };
    let data = PathBuf::from(data);
    let churn = PathBuf::from(churn);
    std::fs::create_dir_all(&data).expect("data folder");
    std::fs::create_dir_all(&churn).expect("churn folder");

    let index = FileIndex::default();
    let started = Instant::now();
    warm(index.clone(), data.clone());
    wait_ready(&index, started);

    // Let the first snapshot land (the build saves it).
    let snapshot = data.join("file-index.bin");
    let waited = Instant::now();
    while !snapshot.exists() && waited.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(250));
    }
    if let Ok(meta) = std::fs::metadata(&snapshot) {
        println!("snapshot {:.1} MB", meta.len() as f64 / 1_048_576.0);
    }
    let store = index.store_handle();
    println!("store memory {:.1} MB", store.memory_bytes() as f64 / 1_048_576.0);

    if let [program, rest @ ..] = command {
        let churn_started = Instant::now();
        let status = std::process::Command::new(program).args(rest).status().expect("churn command");
        println!("churn {status} in {:.1} s", churn_started.elapsed().as_secs_f64());
        // The reader polls once a second; folder walks follow the batches.
        std::thread::sleep(Duration::from_secs(15));
    }

    // `IDLE_SECS`: keep the index running with no work from the probe, so an
    // outside sampler can measure its idle cost.
    if let Some(seconds) = std::env::var("IDLE_SECS").ok().and_then(|v| v.parse::<u64>().ok()) {
        println!("idle {seconds} s");
        std::thread::sleep(Duration::from_secs(seconds));
    }

    let prefix = format!("{}\\", churn.to_string_lossy().trim_end_matches('\\'));
    let indexed: BTreeSet<String> = store
        .paths_below(&prefix)
        .into_iter()
        .map(|(path, _)| path.to_lowercase())
        .collect();
    let disk = disk_paths(&churn);
    let missing: Vec<&String> = disk.difference(&indexed).collect();
    let stale: Vec<&String> = indexed.difference(&disk).collect();
    println!(
        "verify {}: disk {} indexed {} missing {} stale {}",
        churn.display(),
        disk.len(),
        indexed.len(),
        missing.len(),
        stale.len()
    );
    for path in missing.iter().take(15) {
        println!("  MISSING {path}");
    }
    for path in stale.iter().take(15) {
        println!("  STALE   {path}");
    }

    let exit_started = Instant::now();
    index.flush_for_exit();
    println!("exit save {:.1} s", exit_started.elapsed().as_secs_f64());
    std::process::exit(if missing.is_empty() && stale.is_empty() { 0 } else { 2 });
}
