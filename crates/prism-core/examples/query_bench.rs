//! Headless phase-1 benchmark: what one keystroke costs before the file search
//! (apps, Windows settings, Windows tools, typed actions).
//!
//! `cargo run --release --example query_bench -- <data folder with an apps.json copy> [results.tsv | --verify]`
//!
//! The app list comes from the `apps.json` copy. The settings and tools
//! indexes are built from this PC, as the app builds them (read-only). Each
//! query runs `query::phase1` with no open windows: the window list changes
//! from run to run, so `list_open_windows` is timed alone. The last line is a
//! JSON summary (median of 21 runs per query, and their sum). With a results
//! path, the hits of each query go there (kind, id, score), so two builds can
//! be checked for identical results. With `--verify`, the scorer is compared
//! with a copy of the original algorithm for every title and keyword of the
//! three indexes and every prefix of every query (exit code 1 on a mismatch).

use std::time::Instant;

use prism_core::apps::AppEntry;
use prism_core::query::{self, IntentStore, Phase1Hit};

const QUERIES: [&str; 30] = [
    // Typing two words, a key at a time.
    "c", "ch", "chr", "chro", "chrom", "chrome",
    "s", "se", "set", "sett", "setti", "settings",
    // The E2E bench's J3 queries (`oploop/JOURNEYS.md`) that phase 1 answers.
    "notepad", "uac", "calc", "crhome",
    // Settings, tools, phrases, and a query with no match.
    "display", "bluetooth", "night light", "my eyes hurt", "device manager", "control panel",
    "wifi", "power", "vs code", "task", "e", "printers & scanners", "add or remove programs", "zzqx",
];
const RUNS: usize = 21;

/// The scorer as it was before `score::Query` (release 1.0.20), kept as the
/// reference for `--verify`.
fn reference_score(query: &str, target: &str) -> Option<i32> {
    if query.is_empty() || target.is_empty() {
        return None;
    }
    let q = query.to_lowercase();
    let t = target.to_lowercase();
    if t == q {
        return Some(1_000);
    }
    if t.starts_with(&q) {
        return Some(900 + q.len() as i32);
    }
    if t.contains(&q) {
        return Some(800 + q.len() as i32);
    }
    if q.len() > t.len() {
        return None;
    }
    let q_chars: Vec<char> = q.chars().collect();
    let t_chars: Vec<char> = t.chars().collect();
    let (mut qi, mut score, mut prev, mut consecutive) = (0usize, 0i32, isize::MIN, 0i32);
    for (ti, ch) in t_chars.iter().enumerate() {
        if qi >= q_chars.len() {
            break;
        }
        if *ch != q_chars[qi] {
            continue;
        }
        let mut s = 1i32;
        if prev == ti as isize - 1 {
            consecutive += 1;
            s += consecutive * 3;
        } else {
            consecutive = 0;
            if prev >= 0 {
                s -= ((ti as isize - prev - 1) as i32).min(8);
            }
        }
        if ti == 0 {
            s += 12;
        } else if matches!(t_chars[ti - 1], ' ' | '\t' | '-' | '_' | '.' | '/' | '\\') {
            s += 6;
        }
        score += s;
        prev = ti as isize;
        qi += 1;
    }
    if qi < q_chars.len() {
        return None;
    }
    let significant = q.chars().filter(|c| c.is_ascii_alphanumeric()).count();
    let minimum = if significant >= 3 { (significant as i32 * 3).min(24) } else { i32::MIN };
    score -= (t_chars.len() as i32) / 12;
    (score >= minimum).then_some(score)
}

/// Compares `score::score_text` and a prepared `score::Query` with the
/// reference for every target and every prefix of every query. Returns the
/// count of comparisons and of mismatches.
fn verify(apps: &[AppEntry]) -> (usize, usize) {
    let mut targets: Vec<String> = Vec::new();
    for app in apps {
        targets.push(app.name.clone());
        targets.extend(app.keywords.iter().cloned());
    }
    for setting in prism_core::settings_index::cached().expect("settings").iter() {
        targets.push(setting.title.clone());
        targets.extend(setting.keywords.iter().cloned());
    }
    for tool in prism_core::windows_tools::cached().expect("tools").iter() {
        targets.push(tool.title.clone());
        targets.extend(tool.keywords.iter().cloned());
    }
    // Case, other scripts, final sigma, a dotted capital I, full-width text, separators.
    for extra in [
        "", "A", "Größe ÄNDERN", "ΣΟΦΟΣ ΛΟΓΟΣ", "İstanbul Ayarları", "Ｆｕｌｌｗｉｄｔｈ Settings", "naïve café",
        "Wi-Fi_settings.cpl", "C:\\Windows\\System32\\control.exe", "night\tlight", "日本語の設定", "ǅ Ǆ ǆ",
    ] {
        targets.push(extra.to_string());
    }
    let mut queries: Vec<String> = Vec::new();
    for text in QUERIES.iter().copied().chain(["GRÖSSE", "größe", "σοφοσ", "σοφος", "i̇stanbul", "istanbul", "ｆｕｌｌ", "café", "設定", "ǆ", "Night Light", " "]) {
        let mut prefix = String::new();
        for character in text.chars() {
            prefix.push(character);
            queries.push(prefix.clone());
        }
    }
    queries.sort();
    queries.dedup();
    let (mut compared, mut mismatches) = (0, 0);
    for text in &queries {
        let prepared = prism_core::query::score::Query::new(text);
        for target in &targets {
            let expected = reference_score(text, target);
            let found = (prism_core::query::score::score_text(text, target), prepared.score(target));
            compared += 1;
            if found != (expected, expected) {
                mismatches += 1;
                if mismatches <= 10 {
                    eprintln!("MISMATCH query {text:?} target {target:?}: reference {expected:?}, scorer {found:?}");
                }
            }
        }
    }
    (compared, mismatches)
}

fn median(mut runs: Vec<f64>) -> f64 {
    runs.sort_by(f64::total_cmp);
    runs[runs.len() / 2]
}

fn hit_line(hits: &[Phase1Hit], out: &mut String) {
    for hit in hits {
        out.push_str(&format!("\t{:?}:{}:{}", hit.kind, hit.id, hit.score));
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let data_dir = args.next().expect("usage: query_bench <data folder> [results.tsv]");
    let results_path = args.next();
    let text = std::fs::read_to_string(std::path::Path::new(&data_dir).join("apps.json")).expect("read apps.json");
    let mut value: serde_json::Value = serde_json::from_str(&text).expect("parse apps.json");
    let apps: Vec<AppEntry> = serde_json::from_value(value["apps"].take()).expect("app list");
    let started = Instant::now();
    prism_core::settings_index::warm();
    prism_core::windows_tools::warm();
    let warm_ms = started.elapsed().as_secs_f64() * 1000.0;
    let settings = prism_core::settings_index::cached().map_or(0, |entries| entries.len());
    let tools = prism_core::windows_tools::cached().map_or(0, |entries| entries.len());
    let intent = IntentStore::new();
    if results_path.as_deref() == Some("--verify") {
        let (compared, mismatches) = verify(&apps);
        println!("{{\"compared\":{compared},\"mismatches\":{mismatches}}}");
        std::process::exit(i32::from(mismatches > 0));
    }

    let mut results = String::new();
    let mut hits_total = 0;
    for text in QUERIES {
        let response = query::phase1(text, &apps, &[], &intent);
        hits_total += response.apps.len() + response.actions.len();
        results.push_str(text);
        hit_line(&response.apps, &mut results);
        hit_line(&response.actions, &mut results);
        results.push('\n');
    }
    let medians = QUERIES
        .iter()
        .map(|text| {
            let runs = (0..RUNS)
                .map(|_| {
                    let started = Instant::now();
                    std::hint::black_box(query::phase1(text, &apps, &[], &intent).actions.len());
                    started.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            (*text, median(runs))
        })
        .collect::<Vec<_>>();
    let window_list_ms = median(
        (0..RUNS)
            .map(|_| {
                let started = Instant::now();
                std::hint::black_box(query::list_open_windows().len());
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );
    let windows = query::list_open_windows().len();
    if let Some(results_path) = results_path {
        std::fs::write(results_path, results).expect("write results");
    }
    println!(
        "{{\"phase1_total_ms\":{:.3},\"window_list_ms\":{window_list_ms:.3},\"windows\":{windows},\"apps\":{},\"settings\":{settings},\"tools\":{tools},\"hits\":{hits_total},\"warm_ms\":{warm_ms:.0},\"per_query_ms\":{{{}}}}}",
        medians.iter().map(|(_, ms)| ms).sum::<f64>(),
        apps.len(),
        medians
            .iter()
            .map(|(text, ms)| format!("\"{text}\":{ms:.3}"))
            .collect::<Vec<_>>()
            .join(","),
    );
}
