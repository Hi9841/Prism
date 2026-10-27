//! Headless file-search benchmark on a catalog copy, for memory and latency work.
//!
//! `cargo run --release --example catalog_bench -- <copy of catalog.db>`
//!
//! At each `MARK <phase>` line the process waits for one line on stdin, so a
//! runner can sample its memory and thread count at a known point. The last
//! line is a JSON summary of the search timings.

use std::io::BufRead;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use prism_core::catalog::db::Database;
use prism_core::catalog::name_index::NameIndex;
use prism_core::catalog::search::search_with_generation;

const QUERIES: [&str; 14] = [
    "report",
    "chrome",
    "readme",
    "rpt",
    "pr",
    "package.json",
    "notes",
    "invoice 2024",
    "setup.exe",
    "prism",
    "index.js",
    "zzqx",
    "downloads",
    "steam",
];

fn mark(phase: &str) {
    println!("MARK {phase}");
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: catalog_bench <catalog.db>");
    mark("start");
    let db = Database::open(Path::new(&path)).expect("open catalog");
    let index = NameIndex::default();
    let started = Instant::now();
    index.load(&db).expect("load name index");
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    mark("loaded");

    let generation = AtomicU64::new(1);
    let mut medians = Vec::new();
    for text in QUERIES {
        let mut runs = (0..7)
            .map(|_| {
                let started = Instant::now();
                let response = search_with_generation(
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
                );
                std::hint::black_box(response.items.len());
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect::<Vec<_>>();
        runs.sort_by(f64::total_cmp);
        medians.push((text, runs[runs.len() / 2]));
    }
    mark("searched");
    let total: f64 = medians.iter().map(|(_, ms)| ms).sum();
    let per_query = medians
        .iter()
        .map(|(q, ms)| format!("\"{q}\":{ms:.2}"))
        .collect::<Vec<_>>()
        .join(",");
    println!("{{\"load_ms\":{load_ms:.0},\"search_total_ms\":{total:.2},\"per_query_ms\":{{{per_query}}}}}");
}
