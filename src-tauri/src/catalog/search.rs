use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use super::db::Database;
use super::types::{CandidateEntry, FileEntry, FileSearchResponse, VolumeCoverage};

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 50;

pub(crate) fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn path_depth(path: &str) -> usize {
    path.chars().filter(|&c| c == '\\' || c == '/').count()
}

#[allow(clippy::too_many_arguments)]
pub fn search(
    query: &str,
    limit: Option<usize>,
    db: &Database,
    search_generation: &AtomicU64,
    volumes: &[VolumeCoverage],
    total_indexed: u64,
    indexing: bool,
    ready: bool,
) -> FileSearchResponse {
    let query_trimmed = query.trim();
    let limit = clamp_limit(limit);

    // Direct absolute path browsing
    if let Some(items) = browse_path(query_trimmed, limit) {
        return FileSearchResponse {
            items,
            ready,
            indexing,
            path_browse: true,
            volumes: volumes.to_vec(),
            total_indexed,
        };
    }

    if query_trimmed.chars().count() < 2 {
        return FileSearchResponse {
            items: Vec::new(),
            ready,
            indexing,
            path_browse: false,
            volumes: volumes.to_vec(),
            total_indexed,
        };
    }

    let generation = search_generation.fetch_add(1, AtomicOrdering::AcqRel) + 1;

    let candidate_limit = (limit * 15).max(150);
    let candidates = match db.search_candidates(query_trimmed, candidate_limit) {
        Ok(c) => c,
        Err(_) => {
            return FileSearchResponse {
                items: Vec::new(),
                ready,
                indexing,
                path_browse: false,
                volumes: volumes.to_vec(),
                total_indexed,
            };
        }
    };

    if search_generation.load(AtomicOrdering::Acquire) != generation {
        return FileSearchResponse {
            items: Vec::new(),
            ready,
            indexing,
            path_browse: false,
            volumes: volumes.to_vec(),
            total_indexed,
        };
    }

    let query_lower = query_trimmed.to_lowercase();
    let tokens: Vec<&str> = query_lower.split_whitespace().collect();

    let mut scored: Vec<(i32, CandidateEntry)> = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        if let Some(score) = entry_score(&candidate, &tokens, &query_lower) {
            scored.push((score, candidate));
        }
    }

    scored.sort_by(|(score_a, item_a), (score_b, item_b)| {
        score_b
            .cmp(score_a)
            .then_with(|| item_a.lower_name.len().cmp(&item_b.lower_name.len()))
            .then_with(|| item_a.lower_name.cmp(&item_b.lower_name))
            .then_with(|| path_depth(&item_a.display_path).cmp(&path_depth(&item_b.display_path)))
            .then_with(|| item_a.display_path.len().cmp(&item_b.display_path.len()))
            .then_with(|| {
                item_a
                    .display_path
                    .to_lowercase()
                    .cmp(&item_b.display_path.to_lowercase())
            })
    });

    // Verify disk existence only for the items we are about to return. A row
    // can briefly outlive its file (the watcher removes it within seconds),
    // so the guarantee stays: results never point at missing files. Checking
    // every candidate instead cost one stat syscall per candidate (up to 300)
    // per keystroke.
    let mut items: Vec<FileEntry> = Vec::with_capacity(limit);
    let mut seen_paths = std::collections::HashSet::new();
    for (_, item) in scored {
        if items.len() >= limit {
            break;
        }
        let path = Path::new(&item.display_path);
        let exists = path.exists();
        if super::catalog_debug_enabled() && item.lower_name == query_lower {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "search_path_check",
                    "query": query_trimmed,
                    "display_path": item.display_path,
                    "path_exists": exists,
                    "accepted": exists,
                })
            );
        }
        if !exists {
            continue;
        }
        if !seen_paths.insert(item.display_path.to_lowercase()) {
            continue;
        }
        let parent = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| item.lower_name.clone());

        items.push(FileEntry {
            name,
            path: item.display_path,
            parent,
            is_directory: item.is_directory,
            thumbnail: None,
        });
    }

    FileSearchResponse {
        items,
        ready,
        indexing,
        path_browse: false,
        volumes: volumes.to_vec(),
        total_indexed,
    }
}

pub fn entry_score(entry: &CandidateEntry, tokens: &[&str], full_query: &str) -> Option<i32> {
    let lower_name = &entry.lower_name;
    let penalty = path_penalty(&entry.display_path, lower_name);

    // Highest priority: Exact filename match (e.g. "gemini_revisions_unverified.md")
    if lower_name == full_query {
        return Some(10_000 - penalty + if entry.is_directory { 20 } else { 0 });
    }

    // Exact filename without extension
    if let Some(stem) = Path::new(lower_name).file_stem().and_then(|s| s.to_str()) {
        if stem == full_query {
            return Some(9_500 - penalty + if entry.is_directory { 20 } else { 0 });
        }
    }

    let mut total = if entry.is_directory { 20 } else { 0 };

    for token in tokens {
        total += target_score(token, lower_name)?;
    }

    Some(total - lower_name.len().min(80) as i32 - penalty)
}

/// High-noise locations and filenames. The catalog is full-coverage - system
/// directories, dependency trees, and temp folders are all indexed - so these
/// matches stay findable and merely rank below the same match in a
/// user-authored location. Penalties are sized to sit well under the gaps
/// between quality tiers (exact 10_000, stem 9_500, prefix 4_000, substring
/// 3_000, subsequence 1_000): a great match in Windows still beats a weak
/// match in Documents, but a same-quality match never does.
const LOCATION_PENALTIES: &[(&str, i32)] = &[
    ("\\$recycle.bin\\", 700),
    ("\\system volume information\\", 700),
    ("\\program files\\windowsapps\\", 600),
    ("\\windows\\", 600),
    ("\\windows.old\\", 500),
    ("\\$windows.~bt\\", 500),
    ("\\$windows.~ws\\", 500),
    ("\\node_modules\\", 500),
    ("\\.git\\", 400),
    ("\\.svn\\", 400),
    ("\\.hg\\", 400),
    ("\\appdata\\local\\temp\\", 300),
    ("\\recovery\\", 250),
    ("\\perflogs\\", 250),
    ("\\postgres_data\\", 200),
    ("\\pgdata\\", 200),
];

/// Per-file noise that no location rule catches: shell view metadata, memory
/// manager spill files, and the roaming profile backing store.
const NOISE_FILE_NAMES: &[&str] = &[
    "desktop.ini",
    "thumbs.db",
    "pagefile.sys",
    "hiberfil.sys",
    "swapfile.sys",
];

fn path_penalty(display_path: &str, lower_name: &str) -> i32 {
    let mut penalty = 0i32;
    for (needle, value) in LOCATION_PENALTIES {
        if contains_ascii_ci(display_path, needle) {
            penalty = penalty.max(*value);
        }
    }
    if NOISE_FILE_NAMES.contains(&lower_name) || lower_name.starts_with("ntuser.dat") {
        penalty = penalty.max(800);
    }
    penalty
}

/// ASCII case-insensitive substring probe without allocating a lowercase copy
/// of the path - this runs for every candidate on every keystroke.
fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

pub fn target_score(query: &str, target: &str) -> Option<i32> {
    if target == query {
        return Some(5_000);
    }
    if target.starts_with(query) {
        return Some(4_000);
    }
    if let Some(position) = target.find(query) {
        let boundary = position == 0
            || target
                .as_bytes()
                .get(position.wrapping_sub(1))
                .is_some_and(|byte| matches!(*byte, b' ' | b'-' | b'_' | b'.' | b'\\' | b'/'));
        return Some(3_000 - (position.min(100) as i32 * 2) + if boundary { 500 } else { 0 });
    }

    // Byte-level subsequence matching
    let query_bytes = query.as_bytes();
    let target_bytes = target.as_bytes();
    let query_length = query_bytes.len() as i32;
    let mut current = *query_bytes.first()?;
    let mut matched = 0i32;
    let mut gaps = 0i32;

    for &byte in target_bytes {
        if byte == current {
            matched += 1;
            if let Some(&next) = query_bytes.get(matched as usize) {
                current = next;
            } else {
                if query_length >= 3 && gaps > (query_length * 2).max(6) {
                    return None;
                }
                return Some(1_000 + matched * 8 - gaps.min(120));
            }
        } else if matched > 0 {
            gaps += 1;
        }
    }

    None
}

/// Absolute paths are browsed directly without waiting for index
pub fn browse_path(query: &str, limit: usize) -> Option<Vec<FileEntry>> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let requested = expand_path_input(query)?;
    if !requested.is_absolute() {
        return None;
    }

    if requested.is_dir() {
        return Some(list_directory(&requested, None, limit));
    }
    if requested.is_file() {
        return Some(path_entry(&requested).into_iter().collect());
    }

    let parent = requested.parent()?;
    if !parent.is_dir() {
        return Some(Vec::new());
    }
    let needle = requested
        .file_name()
        .map(|name| name.to_string_lossy().trim().to_lowercase())
        .unwrap_or_default();
    Some(list_directory(parent, Some(&needle), limit))
}

fn list_directory(directory: &Path, needle: Option<&str>, limit: usize) -> Vec<FileEntry> {
    let Ok(children) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let parent = directory.to_string_lossy().into_owned();
    let mut entries: Vec<BrowseCandidate> = Vec::with_capacity(limit);

    for child in children.flatten() {
        let name = child.file_name().to_string_lossy().trim().to_string();
        if name.is_empty() {
            continue;
        }
        let lower_name = name.to_lowercase();
        let is_directory = child.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let score = match needle {
            Some(value) if !value.is_empty() => match target_score(value, &lower_name) {
                Some(score) => score,
                None => continue,
            },
            _ => 0,
        };
        let candidate = BrowseCandidate {
            score,
            lower_name,
            entry: FileEntry {
                name,
                path: child.path().to_string_lossy().into_owned(),
                parent: parent.clone(),
                is_directory,
                thumbnail: None,
            },
        };

        // Keep only the best `limit` entries while walking. This avoids
        // materializing/sorting millions of children in a large or remote
        // directory just to return twenty rows.
        let insertion =
            entries.partition_point(|existing| browse_cmp(existing, &candidate) == Ordering::Less);
        if insertion < entries.len() || entries.len() < limit {
            entries.insert(insertion, candidate);
            if entries.len() > limit {
                entries.pop();
            }
        }
    }

    entries
        .into_iter()
        .map(|candidate| candidate.entry)
        .collect()
}

struct BrowseCandidate {
    score: i32,
    lower_name: String,
    entry: FileEntry,
}

fn browse_cmp(left: &BrowseCandidate, right: &BrowseCandidate) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| right.entry.is_directory.cmp(&left.entry.is_directory))
        .then_with(|| left.lower_name.cmp(&right.lower_name))
        .then_with(|| left.entry.path.cmp(&right.entry.path))
}

fn expand_path_input(query: &str) -> Option<PathBuf> {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return None;
    }

    let expanded = if let Some(rest) = trimmed.strip_prefix('%') {
        rest.find('%')
            .and_then(|end| {
                std::env::var(&rest[..end])
                    .ok()
                    .map(|value| format!("{value}{}", &rest[end + 1..]))
            })
            .unwrap_or_else(|| trimmed.to_string())
    } else if trimmed == "~" || trimmed.starts_with("~\\") || trimmed.starts_with("~/") {
        std::env::var("USERPROFILE")
            .ok()
            .map(|profile| format!("{profile}{}", &trimmed[1..]))
            .unwrap_or_else(|| trimmed.to_string())
    } else {
        trimmed.to_string()
    };

    Some(PathBuf::from(expanded))
}

fn path_entry(path: &Path) -> Option<FileEntry> {
    let name = path.file_name()?.to_string_lossy().trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(FileEntry {
        name,
        path: path.to_string_lossy().into_owned(),
        parent: path
            .parent()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default(),
        is_directory: path.is_dir(),
        thumbnail: None,
    })
}
