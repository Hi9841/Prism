use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use super::db::Database;
use super::name_index::{IndexCandidates, NameIndex, NameQuery};
use super::types::{
    CandidateEntry, FileEntry, FileSearchError, FileSearchErrorKind, FileSearchResponse,
    VolumeCoverage,
};

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 50;

pub fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

pub fn path_depth(path: &str) -> usize {
    path.bytes().filter(|&b| b == b'\\' || b == b'/').count()
}

#[allow(clippy::too_many_arguments)]
pub fn search_with_generation(
    query: &str,
    limit: Option<usize>,
    db: &Database,
    names: Option<&NameIndex>,
    search_generation: &AtomicU64,
    generation: u64,
    volumes: &[VolumeCoverage],
    total_indexed: u64,
    indexing: bool,
    ready: bool,
) -> FileSearchResponse {
    let query_trimmed = query.trim();
    let limit = clamp_limit(limit);

    // Direct absolute path browsing
    if is_cancelled(search_generation, generation) {
        return response(
            Vec::new(),
            ready,
            indexing,
            false,
            volumes,
            total_indexed,
            None,
        );
    }

    if let Some(result) = browse_path_with_errors(query_trimmed, limit) {
        return match result {
            Ok(items) => response(items, ready, indexing, true, volumes, total_indexed, None),
            Err(message) => response(
                Vec::new(),
                ready,
                indexing,
                true,
                volumes,
                total_indexed,
                Some(FileSearchError {
                    kind: FileSearchErrorKind::DirectoryAccess,
                    message,
                }),
            ),
        };
    }

    if query_trimmed.chars().count() < 2 {
        return response(
            Vec::new(),
            ready,
            indexing,
            false,
            volumes,
            total_indexed,
            None,
        );
    }

    let query_lower = query_trimmed.to_lowercase();
    let tokens: Vec<&str> = query_lower.split_whitespace().collect();
    let cancelled = || is_cancelled(search_generation, generation);

    let candidate_limit = (limit * 15).max(150);
    let IndexCandidates {
        entries: candidates,
        typos,
    } = match candidates(
        db,
        names,
        query_trimmed,
        &tokens,
        &query_lower,
        limit,
        candidate_limit,
        &cancelled,
    ) {
        Ok(c) => c,
        Err(message) => {
            return response(
                Vec::new(),
                ready,
                indexing,
                false,
                volumes,
                total_indexed,
                Some(FileSearchError {
                    kind: FileSearchErrorKind::IndexQuery,
                    message,
                }),
            );
        }
    };

    if cancelled() {
        return response(
            Vec::new(),
            ready,
            indexing,
            false,
            volumes,
            total_indexed,
            None,
        );
    }

    let mut scored: Vec<(i32, CandidateEntry)> = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        if is_cancelled(search_generation, generation) {
            return response(
                Vec::new(),
                ready,
                indexing,
                false,
                volumes,
                total_indexed,
                None,
            );
        }
        if let Some(score) = entry_score(&candidate, &tokens, &query_lower) {
            scored.push((score, candidate));
        }
    }
    // Typo matches carry their rank: below every scored name.
    scored.extend(typos.into_iter().map(|(candidate, rank)| (rank, candidate)));

    scored.sort_by(result_order);

    // Verify disk existence only for the items we are about to return. A row
    // can briefly outlive its file (the watcher removes it within seconds),
    // so the guarantee stays: results never point at missing files. Checking
    // every candidate instead cost one stat syscall per candidate (up to 300)
    // per keystroke.
    let mut items: Vec<FileEntry> = Vec::with_capacity(limit);
    let mut seen_paths = std::collections::HashSet::new();
    for (_, item) in scored {
        if is_cancelled(search_generation, generation) {
            return response(
                Vec::new(),
                ready,
                indexing,
                false,
                volumes,
                total_indexed,
                None,
            );
        }
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

    response(items, ready, indexing, false, volumes, total_indexed, None)
}

/// The ranking of scored candidates: score, then the shorter and
/// alphabetically first name, then the shallower and shorter path.
pub fn result_order(
    (score_a, item_a): &(i32, CandidateEntry),
    (score_b, item_b): &(i32, CandidateEntry),
) -> Ordering {
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
}

/// Fallback-catalog candidates come from the in-memory name index once it is
/// loaded, else from the SQL stages (no typo matches). NTFS volumes always use
/// SQL; a strong NTFS match turns the index's typo pass off.
#[allow(clippy::too_many_arguments)]
fn candidates(
    db: &Database,
    names: Option<&NameIndex>,
    query: &str,
    tokens: &[&str],
    query_lower: &str,
    limit: usize,
    candidate_limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<IndexCandidates, String> {
    if let Some(names) = names.filter(|names| names.is_loaded()) {
        let ntfs = db.ntfs_candidates_until(query, candidate_limit, cancelled)?;
        // Only an NTFS entry that is a result (every token matches) and a
        // strong one turns the typo pass off.
        let typos = !ntfs.iter().any(|entry| {
            let (name, directory) = (&entry.lower_name, entry.is_directory);
            name_score(name, directory, tokens, query_lower).is_some()
                && is_strong_match(name, directory, tokens, query_lower)
        });
        let parsed = NameQuery::new(query_lower, tokens);
        if let Some(mut found) = names.candidates(db, &parsed, limit, typos, cancelled)? {
            found.entries.extend(ntfs);
            return Ok(found);
        }
    }
    Ok(IndexCandidates {
        entries: db.search_candidates_until(query, candidate_limit, cancelled)?,
        typos: Vec::new(),
    })
}

fn is_cancelled(search_generation: &AtomicU64, generation: u64) -> bool {
    search_generation.load(AtomicOrdering::Acquire) != generation
}

fn response(
    items: Vec<FileEntry>,
    ready: bool,
    indexing: bool,
    path_browse: bool,
    volumes: &[VolumeCoverage],
    total_indexed: u64,
    error: Option<FileSearchError>,
) -> FileSearchResponse {
    FileSearchResponse {
        items,
        ready,
        indexing,
        path_browse,
        volumes: volumes.to_vec(),
        total_indexed,
        error,
    }
}

pub fn entry_score(entry: &CandidateEntry, tokens: &[&str], full_query: &str) -> Option<i32> {
    let penalty = path_penalty(&entry.display_path, &entry.lower_name);
    name_score(&entry.lower_name, entry.is_directory, tokens, full_query)
        .map(|score| score - penalty)
}

/// `target_score` from which a token matches strongly: the substring tier and
/// every tier above it score at least 2,800 (3,000 minus 2 per byte of
/// position up to 100); a scattered subsequence scores about 1,000 plus 8 per
/// query character.
pub const STRONG_TOKEN_SCORE: i32 = 2_000;

/// True for a strong match: the whole name or its stem is the query, or a
/// token is at least a substring of the name. Only a name that matches every
/// token as a scattered subsequence is weak ("annual rpt" in
/// "annual_report.docx" is strong: "annual" is a prefix).
pub fn is_strong_match(
    lower_name: &str,
    is_directory: bool,
    tokens: &[&str],
    full_query: &str,
) -> bool {
    // The exact-name, exact-stem, and near-exact tiers of `name_score`.
    name_score(lower_name, is_directory, tokens, full_query)
        .is_some_and(|score| score >= NEAR_EXACT_SCORE)
        || tokens.iter().any(|token| {
            target_score(token, lower_name).is_some_and(|score| score >= STRONG_TOKEN_SCORE)
        })
}

/// The name-only part of `entry_score`: everything except the location
/// penalty. The in-memory name index scores every row with this and adds the
/// penalty it stored at load time.
pub fn name_score(
    lower_name: &str,
    is_directory: bool,
    tokens: &[&str],
    full_query: &str,
) -> Option<i32> {
    let directory_bonus = if is_directory { 20 } else { 0 };

    // Highest priority: Exact filename match (e.g. "gemini_revisions_unverified.md")
    if lower_name == full_query {
        return Some(10_000 + directory_bonus);
    }

    // Exact filename without extension. The byte probe is a necessary
    // condition for `file_stem == full_query` that skips the path parse on the
    // hot path; `file_stem` still decides.
    if lower_name.len() > full_query.len()
        && lower_name.starts_with(full_query)
        && lower_name.as_bytes()[full_query.len()] == b'.'
        && Path::new(lower_name).file_stem().and_then(|s| s.to_str()) == Some(full_query)
    {
        return Some(9_500 + directory_bonus);
    }

    // The query with one more character (e.g. "agents.md" for "agent.md"):
    // below the exact tiers, far above every prefix match.
    if is_one_insertion(lower_name, full_query)
        && full_query.chars().count() >= MIN_NEAR_EXACT_QUERY_CHARS
    {
        return Some(NEAR_EXACT_SCORE + directory_bonus);
    }

    let mut total = directory_bonus;

    for token in tokens {
        total += target_score(token, lower_name)?;
    }

    Some(total - lower_name.len().min(80) as i32)
}

/// The `name_score` tier of a name that is the query with one more character.
const NEAR_EXACT_SCORE: i32 = 9_000;
/// Shorter queries change too much with one more character ("cat" -> "cart").
const MIN_NEAR_EXACT_QUERY_CHARS: usize = 4;

/// True when `name` is `query` with one character inserted before the query's
/// extension: "agents.md" for "agent.md", but not "agent.cmd", which is another
/// file type. The byte length test rejects almost every name before any
/// character is compared.
fn is_one_insertion(name: &str, query: &str) -> bool {
    let extra = name.len().wrapping_sub(query.len());
    if !(1..=4).contains(&extra) {
        return false;
    }
    let common = name
        .bytes()
        .zip(query.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    // Both prefixes are the same bytes, so a character boundary in `name` is
    // one in `query` too.
    let mut start = common;
    while !name.is_char_boundary(start) {
        start -= 1;
    }
    if query.rfind('.').is_some_and(|dot| start > dot) {
        return false;
    }
    name[start..]
        .chars()
        .next()
        .is_some_and(|inserted| inserted.len_utf8() == extra)
        && name[start + extra..] == query[start..]
}

/// High-noise locations and filenames. The catalog is full-coverage - system
/// directories, dependency trees, and temp folders are all indexed - so these
/// matches stay findable and merely rank below the same match in a
/// user-authored location. Penalties are sized to sit well under the gaps
/// between quality tiers (exact 10_000, stem 9_500, near-exact 9_000, prefix
/// 4_000, substring 3_000, subsequence 1_000): a great match in Windows still
/// beats a weak match in Documents, but a same-quality match never does.
/// Dependency trees and package caches hold many copies of files nobody edits
/// (64 `agent.md` in one profile), so their penalty is larger than the 1_000
/// from exact to near-exact: "agents.md" in a user folder ranks above an
/// exact `agent.md` copy there, and still far above any prefix match.
const LOCATION_PENALTIES: &[(&str, i32)] = &[
    ("\\node_modules\\", 1_200),
    ("\\.bun\\install\\cache\\", 1_200),
    ("\\npm-cache\\", 1_200),
    ("\\pnpm\\store\\", 1_200),
    ("\\.pnpm-store\\", 1_200),
    ("\\deno\\npm\\", 1_200),
    ("\\.cargo\\registry\\", 1_200),
    ("\\go\\pkg\\mod\\", 1_200),
    ("\\.nuget\\packages\\", 1_200),
    ("\\$recycle.bin\\", 700),
    ("\\system volume information\\", 700),
    ("\\program files\\windowsapps\\", 600),
    ("\\windows\\", 600),
    ("\\windows.old\\", 500),
    ("\\$windows.~bt\\", 500),
    ("\\$windows.~ws\\", 500),
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

/// The `LOCATION_PENALTIES` needles that can match at a separator, by the
/// ASCII-lowercase byte after it: one bit per needle.
const NEEDLES_BY_LEAD: [u32; 128] = {
    assert!(LOCATION_PENALTIES.len() <= 32);
    let mut table = [0u32; 128];
    let mut index = 0;
    while index < LOCATION_PENALTIES.len() {
        let lead = LOCATION_PENALTIES[index].0.as_bytes()[1].to_ascii_lowercase();
        table[lead as usize] |= 1 << index;
        index += 1;
    }
    table
};

pub fn path_penalty(display_path: &str, lower_name: &str) -> i32 {
    // Every location needle starts with a separator, so a match can only
    // begin at a backslash. Testing the needles there, instead of sliding them
    // over every byte, gives the same result for a fraction of the work - the
    // name index computes this once per catalog row at load (5.8 M rows on a
    // large catalog). Only the needles whose next byte matches are tested.
    // ASCII case-insensitive, with no lowercase copy of the path.
    let bytes = display_path.as_bytes();
    let mut penalty = 0i32;
    for start in (0..bytes.len()).filter(|&index| bytes[index] == b'\\') {
        let rest = &bytes[start..];
        let Some(&lead) = rest.get(1).filter(|lead| lead.is_ascii()) else {
            continue;
        };
        let mut candidates = NEEDLES_BY_LEAD[lead.to_ascii_lowercase() as usize];
        while candidates != 0 {
            let (needle, value) = LOCATION_PENALTIES[candidates.trailing_zeros() as usize];
            candidates &= candidates - 1;
            if value > penalty
                && rest.len() >= needle.len()
                && rest[..needle.len()].eq_ignore_ascii_case(needle.as_bytes())
            {
                penalty = value;
            }
        }
    }
    if NOISE_FILE_NAMES.contains(&lower_name) || lower_name.starts_with("ntuser.dat") {
        penalty = penalty.max(800);
    }
    penalty
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

pub fn browse_path_with_errors(
    query: &str,
    limit: usize,
) -> Option<Result<Vec<FileEntry>, String>> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let mut requested = expand_path_input(query)?;
    if !requested.is_absolute() {
        requested = resolve_named_folder_path(query)?;
    }

    match std::fs::metadata(&requested) {
        Ok(metadata) if metadata.is_dir() => {
            return Some(list_directory(&requested, None, limit));
        }
        Ok(metadata) if metadata.is_file() => {
            return Some(Ok(path_entry(&requested).into_iter().collect()));
        }
        Ok(_) => return Some(Ok(Vec::new())),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Some(Err(format!(
                "Could not access '{}': {error}",
                requested.display()
            )));
        }
        Err(_) => {}
    }

    let parent = requested.parent()?;
    match std::fs::metadata(parent) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Some(Ok(Vec::new())),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Some(Err(format!(
                "Could not access '{}': {error}",
                parent.display()
            )));
        }
        Err(_) => return Some(Ok(Vec::new())),
    }
    let needle = requested
        .file_name()
        .map(|name| name.to_string_lossy().trim().to_lowercase())
        .unwrap_or_default();
    Some(list_directory(parent, Some(&needle), limit))
}

fn list_directory(
    directory: &Path,
    needle: Option<&str>,
    limit: usize,
) -> Result<Vec<FileEntry>, String> {
    let children = std::fs::read_dir(directory)
        .map_err(|error| format!("Could not read '{}': {error}", directory.display()))?;
    let parent = directory.to_string_lossy().into_owned();
    let mut entries: Vec<BrowseCandidate> = Vec::with_capacity(limit);

    for child in children {
        let child = child.map_err(|error| {
            format!(
                "Could not finish reading '{}': {error}",
                directory.display()
            )
        })?;
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

    Ok(entries
        .into_iter()
        .map(|candidate| candidate.entry)
        .collect())
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

/// A relative path whose first part names a user folder: "desktop/skills/"
/// browses the Desktop's "skills" folder. The first part matches a known
/// folder by its English name or its folder name (a known folder can be
/// redirected, for example to OneDrive), then a folder directly in the user
/// profile. Anything else is no path, and the query is a name search.
fn resolve_named_folder_path(query: &str) -> Option<PathBuf> {
    let (first, rest) = query.trim().split_once(['/', '\\'])?;
    let first = first.trim().to_lowercase();
    if first.is_empty() || first == "." || first == ".." || first.contains(':') {
        return None;
    }
    let locations = super::known_locations();
    let named = locations.iter().find(|(_, path, kind)| {
        *kind == first
            || path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().to_lowercase() == first)
    });
    let base = match named {
        Some((_, path, _)) => path.clone(),
        None => {
            let (_, profile, _) = locations.iter().find(|(_, _, kind)| *kind == "home")?;
            let child = profile.join(&first);
            if !child.is_dir() {
                return None;
            }
            child
        }
    };
    // One separator style, so listed paths read like Explorer's.
    Some(base.join(rest.replace('/', "\\")))
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
