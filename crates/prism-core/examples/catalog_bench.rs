//! Headless file-search benchmark on a catalog copy, for memory and latency work.
//!
//! `cargo run --release --example catalog_bench -- <copy of catalog.db> [results.tsv]`
//!
//! At each `MARK <phase>` line the process waits for one line on stdin, so a
//! runner can sample its memory and thread count at a known point. The last
//! line is a JSON summary of the load and search timings. With a results
//! path, the paths each query returned go there (one line per query), so two
//! builds can be checked for identical results.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use prism_core::catalog::db::Database;
use prism_core::catalog::name_index::NameIndex;
use prism_core::catalog::search::search_with_generation;

/// The first runs of the session: what a user types right after the index loads.
const QUERIES: [&str; 26] = [
    // The E2E bench's J3 queries (`oploop/JOURNEYS.md`).
    "ch",
    "chrome",
    "notepad",
    "uac",
    "settings",
    "calc",
    "crhome",
    "hypertype-snippets.json",
    "apply_rose_pine_moon_accent.ps1",
    "problem solving practice.excalidraw",
    // Typing one word, a key at a time.
    "pa",
    "pac",
    "pack",
    "packa",
    "packag",
    "package",
    // The earlier catalog queries.
    "report",
    "readme",
    "rpt",
    "pr",
    "package.json",
    "notes",
    "invoice 2024",
    "setup.exe",
    "index.js",
    "zzqx",
];
const RUNS: usize = 7;

fn mark(phase: &str) {
    println!("MARK {phase}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: catalog_bench <catalog.db> [results.tsv]");
    let results_path = args.next();
    mark("start");
    let db = Database::open(Path::new(&path)).expect("open catalog");
    let index = NameIndex::default();
    let started = Instant::now();
    index.load(&db).expect("load name index");
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    mark("loaded");

    let generation = AtomicU64::new(1);
    let search = |text: &str| {
        search_with_generation(
            text,
            Some(50),
            &db,
            Some(&index),
            &generation,
            1,
            &[],
            0,
            false,
            true,
        )
    };
    let mut results = String::new();
    let mut firsts = Vec::new();
    let mut medians = Vec::new();
    for text in QUERIES {
        let started = Instant::now();
        let response = search(text);
        firsts.push((text, started.elapsed().as_secs_f64() * 1000.0));
        results.push_str(text);
        for item in &response.items {
            results.push('\t');
            results.push_str(&item.path);
        }
        results.push('\n');
    }
    for text in QUERIES {
        let mut runs = (0..RUNS)
            .map(|_| {
                let started = Instant::now();
                std::hint::black_box(search(text).items.len());
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect::<Vec<_>>();
        runs.sort_by(f64::total_cmp);
        medians.push((text, runs[runs.len() / 2]));
    }
    mark("searched");
    if let Some(results_path) = results_path {
        std::fs::write(results_path, results).expect("write results");
    }
    let total = |timings: &[(&str, f64)]| timings.iter().map(|(_, ms)| ms).sum::<f64>();
    let per_query = |timings: &[(&str, f64)]| {
        timings
            .iter()
            .map(|(q, ms)| format!("\"{q}\":{ms:.2}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    println!(
        "{{\"load_ms\":{load_ms:.0},\"first_total_ms\":{:.2},\"search_total_ms\":{:.2},\"first_ms\":{{{}}},\"per_query_ms\":{{{}}}}}",
        total(&firsts),
        total(&medians),
        per_query(&firsts),
        per_query(&medians)
    );
}
