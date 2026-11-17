//! Offline memory and search bench of the file store: loads a snapshot copy
//! (no walk, no journal, no disk checks) and runs a fixed query set.
//!
//! `index_mem_bench <file-index.bin> <results.txt> [reps]`
//!
//! Prints the store's own buffer total (`memory_bytes`), the process private
//! bytes and working set after the load, and the median candidate time per
//! query over `reps` rounds. Writes the ranked candidates of each query to
//! `results.txt`, so two builds can be compared line by line.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use prism_core::catalog::store::{FileStore, NameQuery};

/// Typical names, names with many copies (tie breaks), short and long queries,
/// several tokens, and typos.
const QUERIES: &[&str] = &[
    "readme", "readme.md", "index.js", "package.json", "license", "main", "setup", "config", "png",
    "ui", "lib", "test", "chrome", "notepad", "desktop", "prism", "cargo", "node", "dll", "exe",
    "photo", "invoice", "2026", "report pdf", "src main", "settings json", "mod.rs", "cargo.toml",
    "app", "icon", "fle-0042", "progam files", "documnets", "screenshot", "download", "music",
    "video", "zip", "log", "temp",
];

#[repr(C)]
#[derive(Default)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_usage: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> isize;
    fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, size: u32) -> i32;
}

/// (private bytes, working set) of this process, in MiB.
fn process_memory() -> (f64, f64) {
    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ..Default::default()
    };
    // SAFETY: a valid out pointer with its size in `cb`.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    assert!(ok != 0, "GetProcessMemoryInfo failed");
    let mib = |bytes: usize| bytes as f64 / 1_048_576.0;
    (mib(counters.private_usage), mib(counters.working_set_size))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let snapshot = args.next().expect("usage: index_mem_bench <file-index.bin> <results.txt> [reps]");
    let results = args.next().expect("usage: index_mem_bench <file-index.bin> <results.txt> [reps]");
    let reps: usize = args.next().map_or(5, |r| r.parse().expect("reps is a number"));

    let (private_before, ws_before) = process_memory();
    let started = Instant::now();
    let store = FileStore::load(Path::new(&snapshot)).expect("load snapshot");
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    let (private_after, ws_after) = process_memory();
    println!("load {load_ms:.0} ms");
    println!("store buffers {:.1} MiB", store.memory_bytes() as f64 / 1_048_576.0);
    println!(
        "process private {:.1} MiB (+{:.1}), working set {:.1} MiB (+{:.1})",
        private_after,
        private_after - private_before,
        ws_after,
        ws_after - ws_before
    );

    if std::env::var_os("INDEX_BENCH_COMPACT").is_some() {
        let started = Instant::now();
        match store.compact() {
            Some(done) => println!(
                "compact: dropped {} kept {} in {:.0} ms; store buffers {:.1} MiB, process private {:.1} MiB",
                done.dropped,
                done.kept,
                started.elapsed().as_secs_f64() * 1000.0,
                store.memory_bytes() as f64 / 1_048_576.0,
                process_memory().0
            ),
            None => println!("compact: nothing to do"),
        }
    }

    let never = || false;
    let mut out = std::fs::File::create(&results).expect("create results file");
    let mut medians = Vec::new();
    for query in QUERIES {
        let lower = query.to_lowercase();
        let tokens: Vec<&str> = lower.split_whitespace().collect();
        let parsed = NameQuery::new(&lower, &tokens);
        let mut times = Vec::with_capacity(reps);
        let mut last = None;
        for _ in 0..reps {
            let started = Instant::now();
            let found = store.candidates(&parsed, 8, true, &never);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            last = Some(found);
        }
        times.sort_by(f64::total_cmp);
        let median = times[times.len() / 2];
        medians.push(median);
        let found = last.expect("at least one rep");
        writeln!(out, "## {query}").unwrap();
        for entry in &found.entries {
            writeln!(out, "{}", entry.display_path).unwrap();
        }
        for (entry, rank) in &found.typos {
            writeln!(out, "~{rank} {}", entry.display_path).unwrap();
        }
    }
    let total: f64 = medians.iter().sum();
    println!("queries {} median sum {total:.1} ms", QUERIES.len());
}
