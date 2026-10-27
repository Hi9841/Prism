//! Launcher-grade fuzzy search. Strings are compared as UTF-16 code units,
//! exactly as the TypeScript indexes them.

use std::collections::HashMap;

use crate::js;
use crate::lib::types::AppEntry;

pub struct FuzzyHit<'a, T> {
    pub item: &'a T,
    pub score: f64,
}

/// Whitespace plus the common name separators (space, tab, -, _, .).
fn is_word_separator(code: u16) -> bool {
    matches!(code, 32 | 9 | 10 | 13 | 45 | 95 | 46)
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// `c !== c.toLowerCase() && c === c.toUpperCase()` for one UTF-16 code unit.
fn is_upper_boundary(unit: u16) -> bool {
    let Some(c) = char::from_u32(unit as u32) else {
        return false; // a lone surrogate maps to itself
    };
    let one = c.to_string();
    one != c.to_lowercase().collect::<String>() && one == c.to_uppercase().collect::<String>()
}

/// Returns a score (higher is better) when `query` is a subsequence of
/// `target`, or `None` when it isn't. Rewards prefix matches, camel-case
/// boundaries, word starts and consecutive runs; penalizes gaps.
pub fn fuzzy_score(query: &str, target: &str) -> Option<f64> {
    fuzzy_score_core(&utf16(&js::lower(query)), &utf16(&js::lower(target)), Some(&utf16(target)))
}

/// Core scorer over pre-lowercased inputs. `boundary_target` carries the
/// original-case target for camelCase detection, or `None` when the target is
/// already known lowercase and the check would be dead work.
fn fuzzy_score_core(q: &[u16], t: &[u16], boundary_target: Option<&[u16]>) -> Option<f64> {
    if q.is_empty() || q.len() > t.len() {
        return None;
    }
    let mut qi = 0usize;
    let mut score = 0.0f64;
    let mut prev: i64 = -1;
    let mut consecutive = 0.0f64;

    let mut ti = 0usize;
    while ti < t.len() && qi < q.len() {
        if t[ti] != q[qi] {
            ti += 1;
            continue;
        }
        let mut s = 1.0f64;
        if prev == ti as i64 - 1 {
            consecutive += 1.0;
            s += consecutive * 3.0;
        } else {
            consecutive = 0.0;
            s -= js::min(8.0, (ti as i64 - prev - 1) as f64 * 0.5);
        }

        // Boundary bonuses: string start > camelCase > word start.
        if ti == 0 {
            s += 12.0;
        } else if let Some(original) = boundary_target {
            // BUG(port): the original throws when lowercasing lengthened the
            // string and `ti` runs past the original; this treats it as no camel bonus.
            if original.get(ti).copied().is_some_and(is_upper_boundary) {
                s += 7.0; // camelCase boundary
            } else if is_word_separator(t[ti - 1]) {
                s += 6.0; // word start
            }
        } else if is_word_separator(t[ti - 1]) {
            s += 6.0; // word start
        }

        score += s;
        prev = ti as i64;
        qi += 1;
        ti += 1;
    }
    if qi < q.len() {
        return None;
    }
    // Prefer tighter targets when scores are close.
    score -= t.len() as f64 * 0.08;
    Some(score)
}

/// Items `fuzzy` can search: an id, a title, and optional keywords.
pub trait Searchable {
    fn title(&self) -> &str;
    fn keywords(&self) -> Option<&[String]>;
}

/// Fuzzy search over a pool of {title, keywords} items. Best-first, capped at `limit`.
pub fn fuzzy<'a, T: Searchable>(pool: &'a [T], query: &str, limit: usize) -> Vec<FuzzyHit<'a, T>> {
    let mut results = Vec::new();
    for item in pool {
        let mut score = fuzzy_score(query, item.title());
        if score.is_none()
            && let Some(keywords) = item.keywords()
        {
            let mut best: Option<f64> = None;
            for keyword in keywords {
                if let Some(s) = fuzzy_score(query, keyword)
                    && best.is_none_or(|b| s > b)
                {
                    best = Some(s);
                }
            }
            if let Some(best) = best {
                score = Some(best - 3.0); // keyword-only matches rank below titles
            }
        }
        if let Some(score) = score {
            results.push(FuzzyHit { item, score });
        }
    }
    // Stable, like the engine's Array.prototype.sort.
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);
    results
}

struct RankedHit<'a> {
    item: &'a AppEntry,
    score: f64,
    rank: i32,
}

/// Keeps `top` sorted best-first (score desc, then source rank desc) and capped at `limit`.
fn insert_top_k<'a>(top: &mut Vec<RankedHit<'a>>, hit: RankedHit<'a>, limit: usize) {
    let (mut lo, mut hi) = (0usize, top.len());
    while lo < hi {
        let mid = (lo + hi) >> 1;
        let existing = &top[mid];
        let existing_first =
            existing.score > hit.score || (existing.score == hit.score && existing.rank >= hit.rank);
        if existing_first {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo >= limit {
        return;
    }
    top.insert(lo, hit);
    if top.len() > limit {
        top.pop();
    }
}

pub fn fuzzy_apps<'a>(apps: &'a [AppEntry], query: &str, limit: usize, pre_deduped: bool) -> Vec<&'a AppEntry> {
    let q = js::trim(query);
    if q.is_empty() {
        return Vec::new();
    }
    let lower = js::lower(q);
    let tokens: Vec<Vec<u16>> = js::split_whitespace(&lower).into_iter().map(utf16).collect();
    let normalized_query = normalize_text(&lower);
    let significant_length = js::len(&normalized_query) as f64;
    let minimum_score =
        if significant_length >= 3.0 { js::min(24.0, significant_length * 3.0) } else { f64::NEG_INFINITY };

    let deduped;
    let pool: Vec<&AppEntry> = if pre_deduped {
        apps.iter().collect()
    } else {
        deduped = dedupe_indices(apps);
        deduped.iter().map(|&i| &apps[i]).collect()
    };
    let lower_query = utf16(&lower);
    let mut top: Vec<RankedHit> = Vec::new();
    for app in pool {
        if let Some(score) = app_score(app, &lower_query, &tokens, &normalized_query)
            && score >= minimum_score
        {
            insert_top_k(&mut top, RankedHit { item: app, score, rank: source_priority(app.source.as_deref()) }, limit);
        }
    }
    top.into_iter().map(|hit| hit.item).collect()
}

/// Indices of the preferred alias per normalized name, in first-seen key order (a JS `Map`).
fn dedupe_indices(apps: &[AppEntry]) -> Vec<usize> {
    let mut order: Vec<String> = Vec::new();
    let mut preferred: HashMap<String, usize> = HashMap::new();
    for (i, app) in apps.iter().enumerate() {
        let key = match app.normalized_name.as_deref() {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => normalize_text(&app.name),
        };
        if key.is_empty() {
            continue;
        }
        match preferred.get(&key) {
            None => {
                order.push(key.clone());
                preferred.insert(key, i);
            }
            Some(&current) => {
                if source_priority(app.source.as_deref()) > source_priority(apps[current].source.as_deref()) {
                    preferred.insert(key, i);
                }
            }
        }
    }
    order.iter().map(|key| preferred[key]).collect()
}

/// Collapse aliases discovered through several Windows app registries.
pub fn dedupe_apps(apps: &[AppEntry]) -> Vec<AppEntry> {
    dedupe_indices(apps).into_iter().map(|i| apps[i].clone()).collect()
}

fn source_priority(source: Option<&str>) -> i32 {
    match source {
        Some("taskbar") => 6,
        Some("startMenu") => 5,
        Some("desktop") => 4,
        Some("appsFolder") => 3,
        Some("registry" | "appPaths" | "applications") => 2,
        Some("programs") => 1,
        _ => 0,
    }
}

fn app_score(app: &AppEntry, lower_query: &[u16], tokens: &[Vec<u16>], normalized_query: &str) -> Option<f64> {
    // Punctuation-insensitive exact match: "visualstudiocode" matches
    // "Visual Studio Code" no matter how the user typed it.
    if let Some(normalized) = app.normalized_name.as_deref()
        && !normalized.is_empty()
        && normalized_query == normalized
    {
        return Some(100.0);
    }

    let lower_name = utf16(&js::lower(&app.name));
    if let Some(name_score) = match_target(&lower_name, tokens) {
        // Exact and prefix application-name matches rank first.
        if lower_name == lower_query {
            return Some(name_score + 40.0);
        }
        if lower_name.starts_with(lower_query) {
            return Some(name_score + 20.0);
        }
        return Some(name_score);
    }

    // Fall back to metadata: exe names, folder names, publisher, aliases.
    let mut best: Option<f64> = None;
    for keyword in app.keywords.iter().flatten() {
        if let Some(score) = match_target(&utf16(&js::lower(keyword)), tokens)
            && best.is_none_or(|b| score > b)
        {
            best = Some(score);
        }
    }
    best.map(|best| best - 4.0) // keyword-only matches rank below names
}

/// Whole-query subsequence match for single tokens; per-token summed
/// subsequence match for multi-word queries (every word must hit).
fn match_target(lower_target: &[u16], tokens: &[Vec<u16>]) -> Option<f64> {
    if tokens.len() == 1 {
        return fuzzy_score_core(&tokens[0], lower_target, None);
    }
    let mut total = 0.0;
    for token in tokens {
        total += fuzzy_score_core(token, lower_target, None)?;
    }
    Some(total)
}

/// `s.toLowerCase().replace(/[^a-z0-9]/g, "")`
fn normalize_text(s: &str) -> String {
    js::lower(s).chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect()
}

// PORT STATUS: confidence=high todos=0
