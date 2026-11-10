//! Headless file-search benchmark on a catalog copy, for memory and latency work.
//!
//! `cargo run --release --example catalog_bench -- <copy of catalog.db> [results.tsv]`
//!
//! At each `MARK <phase>` line the process waits for one line on stdin, so a
//! runner can sample its memory and thread count at a known point. The last
//! line is a JSON summary of the load and search timings. With a results
//! path, the paths each query returned go there (one line per query), so two
//! builds can be checked for identical results.
//!
//! After the searches it adds 1,000 rows to the catalog copy (a volume id of
//! its own) and searches once more: the index then appends rows for the first
//! time after its load, as it does in the app when the watcher writes new
//! files. `append_ms` is that search; the memory at `MARK appended` shows
//! what the first append costs. The rows are removed again at the end.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use prism_core::catalog::db::Database;
use prism_core::catalog::name_index::NameIndex;
use prism_core::catalog::search::search_with_generation;
use prism_core::catalog::types::ScannedItem;

const APPEND_VOLUME: &str = "catalog-bench";
const APPEND_ROWS: usize = 1_000;

fn append_rows() -> Vec<ScannedItem> {
    (0..APPEND_ROWS)
        .map(|number| {
            let name = format!("Catalog-Bench-Append-{number:04}.txt");
            let parent = r"C:\catalog-bench\appended".to_string();
            let display_path = format!(r"{parent}\{name}");
            ScannedItem {
                normalized_path: display_path.to_lowercase(),
                display_path,
                lower_name: name.to_lowercase(),
                name,
                parent,
                is_directory: false,
                extension: Some("txt".into()),
                modified_at: 0,
                size: 0,
            }
        })
        .collect()
}

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
    // The paths of each query's first pass, to compare with later passes.
    let mut first_lists = Vec::new();
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
        first_lists.push(response.items.iter().map(|item| item.path.clone()).collect::<Vec<_>>());
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
    // The first append after the load.
    let rows = append_rows();
    db.insert_batch(APPEND_VOLUME, 0, &rows).expect("add rows");
    let started = Instant::now();
    let appended = search("catalog-bench-append").items.len();
    let append_ms = started.elapsed().as_secs_f64() * 1000.0;
    mark("appended");
    let keys = rows.iter().map(|row| (row.normalized_path.clone(), false)).collect::<Vec<_>>();
    let removed = db.remove_rows(APPEND_VOLUME, &keys).expect("remove rows");
    assert_eq!(removed, APPEND_ROWS, "the bench rows were not all removed");

    // A reload: the 26 queries run in rounds while `load` rebuilds the index.
    // A round counts only when it ran fully inside the reload.
    index.request_reload();
    let reload_started = Instant::now();
    let loading = AtomicBool::new(true);
    let mut reload_rounds = Vec::new();
    let mut reload_lists_equal = true;
    let round = || {
        let mut sum = 0.0;
        let mut equal = true;
        for (text, first) in QUERIES.iter().zip(&first_lists) {
            let started = Instant::now();
            let response = search(text);
            sum += started.elapsed().as_secs_f64() * 1000.0;
            equal &= response.items.iter().map(|item| &item.path).eq(first);
        }
        (sum, equal)
    };
    let reload_ms = std::thread::scope(|scope| {
        let loader = scope.spawn(|| {
            index.load(&db).expect("reload name index");
            loading.store(false, Ordering::Release);
            reload_started.elapsed().as_secs_f64() * 1000.0
        });
        while loading.load(Ordering::Acquire) {
            let (sum, equal) = round();
            // The load can end during the round.
            if loading.load(Ordering::Acquire) {
                reload_rounds.push(sum);
                reload_lists_equal &= equal;
            }
        }
        loader.join().expect("reload thread")
    });
    reload_rounds.sort_by(f64::total_cmp);
    let reload_search_total_ms = reload_rounds.get(reload_rounds.len() / 2).copied().unwrap_or(0.0);
    let after_reload_lists_equal = round().1;
    mark("reloaded");

    let total = |timings: &[(&str, f64)]| timings.iter().map(|(_, ms)| ms).sum::<f64>();
    let per_query = |timings: &[(&str, f64)]| {
        timings
            .iter()
            .map(|(q, ms)| format!("\"{q}\":{ms:.2}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    println!(
        "{{\"load_ms\":{load_ms:.0},\"append_ms\":{append_ms:.2},\"append_hits\":{appended},\"reload_ms\":{reload_ms:.0},\"reload_rounds\":{},\"reload_search_total_ms\":{reload_search_total_ms:.2},\"reload_lists_equal\":{reload_lists_equal},\"after_reload_lists_equal\":{after_reload_lists_equal},\"first_total_ms\":{:.2},\"search_total_ms\":{:.2},\"first_ms\":{{{}}},\"per_query_ms\":{{{}}}}}",
        reload_rounds.len(),
        total(&firsts),
        total(&medians),
        per_query(&firsts),
        per_query(&medians)
    );
}
