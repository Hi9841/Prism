//! Ranking audit on this PC: finds result lists where a keyword-only match
//! ranks above (or pushes out) an entry whose own title matches better.
//!
//! `cargo run --release --example rank_audit -- <out folder>`
//!
//! Scans apps into `<out>/apps.json` (a fresh cache), builds the settings and
//! tools indexes, runs `query::phase1` for queries made from every title, and
//! writes `<out>/findings.tsv` and `<out>/keywords.tsv`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::PathBuf;

use prism_core::query::score::Query;
use prism_core::query::{self, IntentStore, LaunchStore, Phase1Hit};

struct Entry {
    title: String,
    keywords: Vec<String>,
}

fn tier(score: Option<i32>) -> &'static str {
    match score {
        Some(1_000) => "exact",
        Some(s) if s >= 900 => "prefix",
        Some(s) if s >= 800 => "substring",
        Some(_) => "fuzzy",
        None => "none",
    }
}

/// The keyword that gives `entry` its score for `query`, when a keyword beats the title.
fn winning_keyword(query: &Query, entry: &Entry) -> Option<(String, i32)> {
    let title = query.score(&entry.title);
    entry
        .keywords
        .iter()
        .filter_map(|k| query.score(k).map(|s| (k.clone(), s - 50)))
        .filter(|(_, s)| *s > 0 && title.is_none_or(|t| *s > t))
        .max_by_key(|(_, s)| *s)
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("out folder"));
    std::fs::create_dir_all(&out).unwrap();
    let cache = out.join("apps.json");
    let _ = std::fs::remove_file(&cache);
    let apps = prism_core::apps::scan(&cache).expect("scan").apps;
    prism_core::settings_index::warm();
    prism_core::windows_tools::warm();
    let settings = prism_core::settings_index::cached().expect("settings");
    let tools = prism_core::windows_tools::cached().expect("tools");
    eprintln!("apps {} settings {} tools {}", apps.len(), settings.len(), tools.len());

    // id -> entry, for both sections.
    let mut by_id: HashMap<String, Entry> = HashMap::new();
    for app in &apps {
        by_id.insert(
            format!("app::{}", app.app_id),
            Entry { title: app.name.clone(), keywords: app.keywords.clone() },
        );
    }
    for s in settings.iter() {
        by_id.insert(format!("action::{}", s.id), Entry { title: s.title.clone(), keywords: s.keywords.clone() });
    }
    for t in tools.iter() {
        by_id.insert(format!("action::{}", t.id), Entry { title: t.title.clone(), keywords: t.keywords.clone() });
    }

    // Keyword report: keywords shared by entries with different titles.
    let mut shared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for app in &apps {
        for k in &app.keywords {
            shared.entry(k.to_lowercase()).or_default().insert(app.name.clone());
        }
    }
    let mut kw = String::from("keyword\tapps\texamples\n");
    let mut rows: Vec<_> = shared.iter().filter(|(_, n)| n.len() >= 3).collect();
    rows.sort_by_key(|(_, n)| std::cmp::Reverse(n.len()));
    for (k, names) in rows {
        let examples: Vec<_> = names.iter().take(6).cloned().collect();
        writeln!(kw, "{k}\t{}\t{}", names.len(), examples.join(" | ")).unwrap();
    }
    std::fs::write(out.join("keywords.tsv"), kw).unwrap();

    // Queries: every title, each word (3+ chars), and prefixes of each word.
    let mut queries = BTreeSet::new();
    let titles = apps.iter().map(|a| a.name.clone())
        .chain(tools.iter().map(|t| t.title.clone()))
        .chain(settings.iter().map(|s| s.title.clone()));
    for title in titles {
        let lower = title.to_lowercase();
        queries.insert(lower.clone());
        for word in lower.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3) {
            let chars: Vec<char> = word.chars().collect();
            for n in 3..=chars.len() {
                queries.insert(chars[..n].iter().collect::<String>());
            }
        }
    }
    eprintln!("queries {}", queries.len());

    let intent = IntentStore::new();
    let launches = LaunchStore::new();
    let mut findings = String::from("section\tquery\tkind\twinner\twinner_score\tvia_keyword\tkeyword_tier\tloser\tloser_title_tier\tloser_score\tword_start\tloser_actual\n");
    let mut count = 0usize;
    for q in &queries {
        let prepared = Query::new(q);
        let response = query::phase1(q, &apps, &[], &intent, &launches);
        for (section, hits) in [("apps", &response.apps), ("actions", &response.actions)] {
            let shown: BTreeSet<&str> = hits.iter().map(|h| h.id.as_str()).collect();
            let check = |winner: &Phase1Hit, loser_title: &str, loser_score: Option<i32>, loser_actual: i32, kind: &str, findings: &mut String| -> bool {
                if winner.score < loser_actual {
                    return false;
                }
                let Some(entry) = by_id.get(&winner.id) else { return false };
                let Some((keyword, _)) = winning_keyword(&prepared, entry) else { return false };
                let winner_title = prepared.score(&entry.title);
                // The loser's title must be a real match (substring or better) and beat the winner's title.
                let Some(loser_title_score) = loser_score else { return false };
                if loser_title_score < 800 || winner_title.is_some_and(|w| w >= loser_title_score) {
                    return false;
                }
                let lower_title = loser_title.to_lowercase();
                let word_start = lower_title.match_indices(q.as_str()).any(|(at, _)| {
                    at == 0 || !lower_title[..at].chars().next_back().is_some_and(char::is_alphanumeric)
                });
                writeln!(
                    findings,
                    "{section}\t{q}\t{kind}\t{}\t{}\t{keyword}\t{}\t{loser_title}\t{}\t{loser_title_score}\t{word_start}\t{loser_actual}",
                    winner.title, winner.score, tier(prepared.score(&keyword)), tier(loser_score)
                ).unwrap();
                true
            };
            // Inversions inside the shown list.
            for (i, upper) in hits.iter().enumerate() {
                for lower in &hits[i + 1..] {
                    let Some(lower_entry) = by_id.get(&lower.id) else { continue };
                    if upper.score > lower.score || upper.score == lower.score {
                        if check(upper, &lower_entry.title, prepared.score(&lower_entry.title), lower.score, "inversion", &mut findings) {
                            count += 1;
                        }
                    }
                }
            }
            // Entries pushed out of the list while a keyword-only hit stays.
            if section == "apps" {
                if hits.len() == 6 {
                    for app in &apps {
                        let id = format!("app::{}", app.app_id);
                        if shown.contains(id.as_str()) || hits.iter().any(|h| h.title == app.name) {
                            continue;
                        }
                        let title_score = prepared.score(&app.name);
                        if title_score.is_some() {
                            // A shown hit with a lower title score holds the slot.
                            for upper in hits {
                                if check(upper, &app.name, title_score, prepared.score_with_keywords(&app.name, &app.keywords).unwrap_or(0), "pushed-out", &mut findings) {
                                    count += 1;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    std::fs::write(out.join("findings.tsv"), findings).unwrap();
    eprintln!("findings {count}");
}
