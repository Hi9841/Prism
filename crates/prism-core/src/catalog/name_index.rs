//! In-memory name index for the fallback `files` catalog.
//!
//! The SQL candidate stages each stop at a `LIMIT` before ranking. On a
//! multi-million-row catalog that costs tens of milliseconds per keystroke
//! (about 200 ms for a common exact name such as `package.json`), and the best
//! match can fall outside the truncated pool. This index keeps every lowercase
//! name in one contiguous buffer, applies the same admission rules as the SQL
//! stages and the same scorer, scans all rows in parallel, and asks SQLite only
//! for the `display_path` of the top rows.
//!
//! Sync invariant: a row's `lower_name` never changes for a given id (upserts
//! conflict on the same `normalized_path`; renames delete and insert), and new
//! rows always get a larger id (`AUTOINCREMENT`). The index therefore stays
//! current by appending rows above `max_id` and by dropping ids that the by-id
//! fetch no longer finds.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, RwLock};

use neo_frizbee::{Config, Matcher, Pattern, PatternConfig, SortStrategy, SIMD_CHUNK_BYTES};

use super::db::Database;
use super::search::{is_strong_match, name_score, path_depth, path_penalty};
use super::types::{CandidateEntry, NameRow, RowLookup};

/// Rows per SQLite read while loading. One batch holds the reader connection
/// for tens of milliseconds, so searches on the SQL path keep running.
const LOAD_BATCH: usize = 100_000;
/// New rows a search appends inline before it answers. A larger backlog (a
/// full rescan) goes to the background loader and the search uses SQL.
const SEARCH_CATCH_UP: usize = 10_000;
/// Rewritten rows (a directory rename) a search applies inline. A larger
/// rename is applied by the background sync, outside the keystroke.
const SEARCH_REWRITES: usize = 2_000;
const MAX_SCAN_THREADS: usize = 8;
const MIN_ROWS_PER_THREAD: usize = 100_000;
const CANCEL_CHECK_INTERVAL: usize = 16_384;
/// Rows selected for the by-id fetch per result slot. The surplus covers rows
/// that the final disk-existence check rejects; the SQL candidate path kept
/// `max(limit * 15, 150)`, so the index keeps the same pool (the by-id fetch
/// is batched, so the larger pool stays cheap).
const CANDIDATES_PER_RESULT: usize = 15;
const MIN_CANDIDATES: usize = 150;
/// Deleted rows the index has not seen before it reloads: a share of its
/// rows, at least this many (bulk deletes leave dead rows it would otherwise
/// find only one search at a time).
const MIN_RELOAD_DELETIONS: u64 = 50_000;
/// Selection rounds when the by-id fetch finds deleted rows.
const MAX_SELECTION_ROUNDS: usize = 4;
/// The SQL subsequence stage probed only the first four tokens.
const PROBE_TOKENS: usize = 4;

/// Every location penalty in `search::path_penalty` is a multiple of this, so
/// the penalty fits in the low byte of the row flags.
const PENALTY_STEP: i32 = 50;
const FLAG_DIRECTORY: u16 = 1 << 15;
const FLAG_DELETED: u16 = 1 << 14;
const FLAG_PENALTY_MASK: u16 = 0xff;

/// Typo pass (FFF's frizbee matcher), for queries with this many letters or
/// digits. It runs only when no name is a strong match (see
/// `search::is_strong_match`), so a query with a strong match keeps exactly
/// the results it had.
const MIN_TYPO_QUERY_CHARS: usize = 3;
/// Chunk pointers per name: 768 bytes. An NTFS name is at most 255 UTF-16
/// units, so at most 765 UTF-8 bytes.
const TYPO_MAX_CHUNKS: usize = 48;
/// The typo pass keeps only close names: their start is at most one edit from
/// the query, two from eight characters. Close names (typo hits and scattered
/// subsequence matches alike) rank like `name_score` does: `CLOSE_TYPO_SCORE`
/// per token, plus `CLOSE_EDIT_STEP` per edit saved, plus the folder bonus,
/// minus the name length and the location penalty.
const CLOSE_TYPO_SCORE: i32 = 1_500;
const CLOSE_EDIT_STEP: i32 = 1_000;
/// Prefix edit distances from this value up count the same.
const TYPO_FAR: usize = 3;

pub struct NameIndex {
    names: RwLock<Names>,
    loaded: AtomicBool,
    loader_running: AtomicBool,
    reload_requested: AtomicBool,
    /// A search found more new rows than it may append inline.
    behind: AtomicBool,
    /// `Database::rows_deleted` when the current rows were loaded.
    seen_deleted: AtomicU64,
}

#[derive(Default)]
struct Names {
    /// All lowercase names, back to back.
    text: String,
    /// End offset of each name in `text`.
    ends: Vec<u32>,
    ids: Vec<u32>,
    /// Characters present in each name (see `char_mask`).
    masks: Vec<u32>,
    /// Directory flag, deleted flag, and location penalty / `PENALTY_STEP`.
    flags: Vec<u16>,
    /// Path depth (high 16 bits) and `display_path` byte length (low 16
    /// bits): the final sort's tie breaks after the name.
    places: Vec<u32>,
    max_id: i64,
    deleted: usize,
    /// Bumped by every reset. A search that read indexes or `max_id` from an
    /// older generation must not apply them to the new buffers.
    generation: u64,
}

/// A query parsed once per search: the scorer inputs plus the admission rules
/// of the SQL candidate stages that this index replaces.
pub struct NameQuery<'a> {
    full: &'a str,
    tokens: &'a [&'a str],
    mask: u32,
    /// The FTS5 trigram and subsequence stages ran only for 3+ characters.
    long: bool,
    /// FTS5 stage: every 3+ character token is a substring.
    substring_tokens: Vec<&'a str>,
    /// Subsequence stage: the name starts with a token's first character and
    /// holds the rest of that token in order (`LIKE 'r%p%t%'`).
    probe_tokens: &'a [&'a str],
}

#[derive(Clone, Copy)]
struct Hit {
    score: i32,
    index: u32,
    /// Found by the typo pass: `score` is its final rank.
    typo: bool,
}

/// What the index found for a query.
#[derive(Default)]
pub struct IndexCandidates {
    /// Names the admission rules accept, for the caller's scorer.
    pub entries: Vec<CandidateEntry>,
    /// Typo-tolerant fuzzy matches the rules reject, with their final rank
    /// (below every strong match).
    pub typos: Vec<(CandidateEntry, i32)>,
}

impl Default for NameIndex {
    fn default() -> Self {
        Self {
            names: RwLock::new(Names::default()),
            loaded: AtomicBool::new(false),
            loader_running: AtomicBool::new(false),
            reload_requested: AtomicBool::new(false),
            behind: AtomicBool::new(false),
            seen_deleted: AtomicU64::new(0),
        }
    }
}

impl<'a> NameQuery<'a> {
    pub fn new(full: &'a str, tokens: &'a [&'a str]) -> Self {
        Self {
            full,
            tokens,
            mask: tokens.iter().fold(0, |mask, token| mask | char_mask(token)),
            long: full.chars().count() >= 3,
            substring_tokens: tokens
                .iter()
                .copied()
                .filter(|token| token.chars().count() >= 3)
                .collect(),
            probe_tokens: &tokens[..tokens.len().min(PROBE_TOKENS)],
        }
    }

    fn wants_typos(&self) -> bool {
        self.full.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_TYPO_QUERY_CHARS
    }

    /// Typos a token may have: none up to three characters, one up to seven,
    /// then two.
    fn token_typos(token: &str) -> u16 {
        match token.chars().count() {
            0..=3 => 0,
            4..=7 => 1,
            _ => 2,
        }
    }

    /// Typos the whole query may have.
    fn typo_budget(&self) -> u32 {
        self.tokens
            .iter()
            .map(|token| u32::from(Self::token_typos(token)))
            .sum()
    }

    /// One frizbee pattern per token, with its `token_typos`.
    fn typo_matcher(&self) -> Matcher {
        let patterns = self
            .tokens
            .iter()
            .map(|token| {
                let typos = Self::token_typos(token);
                Pattern::new(token, PatternConfig::default().max_typos(Some(typos)))
            })
            .collect::<Vec<_>>();
        // The caller ranks the matches itself.
        let config = Config {
            sort: SortStrategy::Unsorted,
            ..Config::default()
        };
        Matcher::from_patterns(&patterns, &config)
    }

    /// True when one of the SQL stages (exact, prefix, FTS5 trigram, or
    /// subsequence probe) would have returned this name, before any `LIMIT`.
    fn admits(&self, name: &str) -> bool {
        if name.starts_with(self.full) {
            return true;
        }
        if !self.long {
            return false;
        }
        if !self.substring_tokens.is_empty()
            && self
                .substring_tokens
                .iter()
                .all(|token| name.contains(token))
        {
            return true;
        }
        self.probe_tokens
            .iter()
            .any(|token| starts_with_subsequence(name, token))
    }
}

/// `LIKE 'a%b%c%'` over characters: the name starts with the token's first
/// character and contains the remaining characters in order.
fn starts_with_subsequence(name: &str, token: &str) -> bool {
    let mut wanted = token.chars();
    let Some(first) = wanted.next() else {
        return false;
    };
    let mut name_chars = name.chars();
    if name_chars.next() != Some(first) {
        return false;
    }
    let mut next = wanted.next();
    for character in name_chars {
        match next {
            None => return true,
            Some(expected) if expected == character => next = wanted.next(),
            Some(_) => {}
        }
    }
    next.is_none()
}

impl NameIndex {
    pub fn is_loaded(&self) -> bool {
        self.loaded.load(AtomicOrdering::Acquire)
    }

    /// Loads or catches up the index on a background thread. A no-op while a
    /// load is already running. Searches use the SQL path until it finishes.
    pub fn spawn_sync(self: &Arc<Self>, db: Arc<Database>) {
        if self
            .loader_running
            .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
            .is_err()
        {
            return;
        }
        let index = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("prism-name-index".into())
            .spawn(move || {
                let result = index.load(&db);
                index.loader_running.store(false, AtomicOrdering::Release);
                match result {
                    // Searches made meanwhile used the SQL path, which has no
                    // substring stage: an open palette searches again.
                    Ok(()) => crate::host::emit("file-index-updated", serde_json::Value::Null),
                    Err(error) => eprintln!("[Prism Catalog] name index load failed: {error}"),
                }
            });
        if spawned.is_err() {
            self.loader_running.store(false, AtomicOrdering::Release);
        }
    }

    /// Drops every row. Used when the catalog is wiped; the next
    /// `spawn_sync` reloads from scratch.
    pub fn reset(&self) {
        self.loaded.store(false, AtomicOrdering::Release);
        let mut names = self.names.write().unwrap_or_else(|e| e.into_inner());
        let generation = names.generation.wrapping_add(1);
        *names = Names { generation, ..Names::default() };
    }

    /// True when the background loader has work: the first load, a backlog
    /// that a search deferred, or enough deleted rows that a fresh load is
    /// cheaper than carrying them.
    pub fn needs_sync(&self) -> bool {
        !self.is_loaded()
            || self.behind.load(AtomicOrdering::Acquire)
            || self.reload_requested.load(AtomicOrdering::Acquire)
    }

    /// `needs_sync`, plus the catalog's signals: a change too large to apply
    /// row by row, or enough deletions that a reload beats carrying them.
    pub fn needs_sync_with(&self, db: &Database) -> bool {
        if db.take_index_reload_request() {
            self.reload_requested.store(true, AtomicOrdering::Release);
        }
        if self.is_loaded() {
            let rows = self.names.read().unwrap_or_else(|e| e.into_inner()).ids.len() as u64;
            let unseen = db.rows_deleted().saturating_sub(self.seen_deleted.load(AtomicOrdering::Acquire));
            if unseen > (rows / 8).max(MIN_RELOAD_DELETIONS) {
                self.reload_requested.store(true, AtomicOrdering::Release);
            }
        }
        self.needs_sync()
    }

    /// Synchronous full load (or catch-up), batch by batch.
    pub fn load(&self, db: &Database) -> Result<(), String> {
        if self.reload_requested.swap(false, AtomicOrdering::AcqRel) || !self.is_loaded() {
            if self.is_loaded() {
                self.reset();
            }
            self.seen_deleted.store(db.rows_deleted(), AtomicOrdering::Release);
        }
        while !self.catch_up(db, LOAD_BATCH, usize::MAX)? {}
        self.behind.store(false, AtomicOrdering::Release);
        self.loaded.store(true, AtomicOrdering::Release);
        Ok(())
    }

    /// Applies up to `max_rewrites` rewritten rows and appends up to
    /// `max_rows` rows above `max_id`. Returns true when the index has caught
    /// up with the catalog.
    fn catch_up(&self, db: &Database, max_rows: usize, max_rewrites: usize) -> Result<bool, String> {
        if !self.refresh_rewritten(db, max_rewrites)? {
            return Ok(false);
        }
        let (after, generation) = {
            let names = self.names.read().unwrap_or_else(|e| e.into_inner());
            (names.max_id, names.generation)
        };
        let rows = db.file_names_after(after, max_rows)?;
        let caught_up = rows.len() < max_rows;
        if !rows.is_empty() {
            let mut names = self.names.write().unwrap_or_else(|e| e.into_inner());
            // A reset in between: these rows belong to the old buffers.
            if names.generation != generation {
                return Ok(false);
            }
            // A concurrent catch-up can have appended some of these rows.
            let fresh = rows.partition_point(|row| row.id <= names.max_id);
            let rows = &rows[fresh..];
            names.reserve(
                rows.len(),
                rows.iter().map(|row| row.lower_name.len()).sum(),
            );
            for row in rows {
                names.push(row)?;
            }
        }
        Ok(caught_up)
    }

    /// Rows whose path changed in place (a directory rename moved them):
    /// recompute their location penalty, depth, and path length. Applies up
    /// to `max` rows; returns false when more are left.
    fn refresh_rewritten(&self, db: &Database, max: usize) -> Result<bool, String> {
        let mut ids = db.take_rewritten_ids(max.saturating_add(1));
        if ids.is_empty() {
            return Ok(true);
        }
        let more = ids.len() > max;
        if more {
            db.restore_rewritten_ids(ids.split_off(max));
        }
        ids.sort_unstable();
        ids.dedup();
        // The database read runs before the write lock; on failure the ids
        // go back for the next call.
        let lookups = match db.file_rows_by_id(&ids) {
            Ok(lookups) => lookups,
            Err(error) => {
                db.restore_rewritten_ids(ids);
                return Err(error);
            }
        };
        let mut names = self.names.write().unwrap_or_else(|e| e.into_inner());
        for (id, lookup) in ids.iter().zip(lookups) {
            let Ok(id) = u32::try_from(*id) else { continue };
            // Ids are appended in ascending order.
            let Ok(index) = names.ids.binary_search(&id) else { continue };
            match lookup {
                RowLookup::Found(entry) => {
                    let penalty = path_penalty(&entry.display_path, &entry.lower_name);
                    let keep = names.flags[index] & (FLAG_DIRECTORY | FLAG_DELETED);
                    names.flags[index] = keep | ((penalty / PENALTY_STEP) as u16 & FLAG_PENALTY_MASK);
                    let depth = path_depth(&entry.display_path).min(0xffff) as u32;
                    let length = entry.display_path.len().min(0xffff) as u32;
                    names.places[index] = depth << 16 | length;
                }
                RowLookup::Missing => {
                    if names.flags[index] & FLAG_DELETED == 0 {
                        names.flags[index] |= FLAG_DELETED;
                        names.deleted += 1;
                    }
                }
                RowLookup::Excluded => {}
            }
        }
        Ok(!more)
    }

    /// Fallback-catalog candidates for a query, best first. `None` means the
    /// index is not loaded and the caller must use the SQL candidate path.
    /// With a large backlog of new rows, the index answers from the rows it
    /// has (the SQL path has no substring stage) and the background sync
    /// catches up. `typos` is false when another source has a strong match.
    pub fn candidates(
        &self,
        db: &Database,
        query: &NameQuery<'_>,
        limit: usize,
        typos: bool,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<IndexCandidates>, String> {
        if !self.is_loaded() {
            return Ok(None);
        }
        if !self.catch_up(db, SEARCH_CATCH_UP, SEARCH_REWRITES)? {
            self.behind.store(true, AtomicOrdering::Release);
        }
        let wanted = (limit * CANDIDATES_PER_RESULT).max(MIN_CANDIDATES);

        let mut found = IndexCandidates::default();
        for round in 0..MAX_SELECTION_ROUNDS {
            let (selected, generation) = {
                let names = self.names.read().unwrap_or_else(|e| e.into_inner());
                let mut hits = names.top_hits(query, wanted, is_cancelled);
                // No strong match: add typo matches, and rank every close name
                // (typo or scattered subsequence) above the rest.
                if typos
                    && query.wants_typos()
                    && !names.has_strong_hit(&hits, query)
                    && !is_cancelled()
                {
                    let mut closeness = Closeness::new(query);
                    for hit in &mut hits {
                        if let Some(rank) = names.close_rank(hit.index as usize, &mut closeness) {
                            *hit = Hit {
                                score: rank,
                                index: hit.index,
                                typo: true,
                            };
                        }
                    }
                    let typos = names.typo_hits(query, wanted, &hits, &mut closeness);
                    hits.extend(typos);
                    names.keep_best(&mut hits, wanted);
                    hits.sort_unstable_by(|a, b| names.compare(a, b));
                }
                let selected = hits
                    .into_iter()
                    .map(|hit| {
                        let typo_rank = hit.typo.then_some(hit.score);
                        (
                            hit.index,
                            i64::from(names.ids[hit.index as usize]),
                            typo_rank,
                        )
                    })
                    .collect::<Vec<_>>();
                (selected, names.generation)
            };
            if is_cancelled() {
                return Ok(Some(IndexCandidates::default()));
            }
            let ids = selected.iter().map(|&(_, id, _)| id).collect::<Vec<_>>();
            let lookups = db.file_rows_by_id(&ids)?;
            let mut missing = Vec::new();
            for ((index, _, typo_rank), lookup) in selected.into_iter().zip(lookups) {
                match (lookup, typo_rank) {
                    (RowLookup::Found(entry), None) => found.entries.push(entry),
                    (RowLookup::Found(entry), Some(rank)) => found.typos.push((entry, rank)),
                    (RowLookup::Missing, _) => missing.push(index),
                    (RowLookup::Excluded, _) => {}
                }
            }
            if missing.is_empty() {
                break;
            }
            // Deleted rows held slots in this round; drop them and select
            // again so live rows further down are not cut off. The last round
            // keeps its live rows: clearing them returned no results at all.
            self.mark_deleted(generation, &missing);
            if round + 1 < MAX_SELECTION_ROUNDS {
                found = IndexCandidates::default();
            }
        }
        Ok(Some(found))
    }

    fn mark_deleted(&self, generation: u64, indexes: &[u32]) {
        let mut names = self.names.write().unwrap_or_else(|e| e.into_inner());
        // Indexes from an older generation point into different rows.
        if names.generation != generation {
            return;
        }
        for &index in indexes {
            let Some(flags) = names.flags.get_mut(index as usize) else { continue };
            if *flags & FLAG_DELETED == 0 {
                *flags |= FLAG_DELETED;
                names.deleted += 1;
            }
        }
        if names.deleted > (names.ids.len() / 4).max(10_000) {
            self.reload_requested.store(true, AtomicOrdering::Release);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.names.read().unwrap().ids.len()
    }
}

impl Names {
    /// Grows every buffer with 1/8 headroom instead of doubling: the name
    /// buffer is over 100 MB on a large catalog, so doubling would leave up to
    /// half of it as dead capacity.
    fn reserve(&mut self, rows: usize, bytes: usize) {
        fn grow<T>(buffer: &mut Vec<T>, extra: usize) {
            if buffer.capacity() - buffer.len() < extra {
                buffer.reserve_exact(extra.max(buffer.len() / 8));
            }
        }
        if self.text.capacity() - self.text.len() < bytes {
            self.text.reserve_exact(bytes.max(self.text.len() / 8));
        }
        grow(&mut self.ends, rows);
        grow(&mut self.ids, rows);
        grow(&mut self.masks, rows);
        grow(&mut self.flags, rows);
        grow(&mut self.places, rows);
    }

    fn push(&mut self, row: &NameRow) -> Result<(), String> {
        let id = u32::try_from(row.id).map_err(|_| format!("row id {} exceeds u32", row.id))?;
        let end = u32::try_from(self.text.len() + row.lower_name.len())
            .map_err(|_| "name index exceeds 4 GiB".to_string())?;
        let penalty = path_penalty(&row.display_path, &row.lower_name);
        debug_assert_eq!(
            penalty % PENALTY_STEP,
            0,
            "penalty must be a multiple of the step"
        );
        let mut flags = (penalty / PENALTY_STEP) as u16 & FLAG_PENALTY_MASK;
        if row.is_directory {
            flags |= FLAG_DIRECTORY;
        }
        let depth = path_depth(&row.display_path).min(0xffff) as u32;
        let length = row.display_path.len().min(0xffff) as u32;
        self.masks.push(char_mask(&row.lower_name));
        self.text.push_str(&row.lower_name);
        self.ends.push(end);
        self.ids.push(id);
        self.flags.push(flags);
        self.places.push(depth << 16 | length);
        self.max_id = row.id;
        Ok(())
    }

    fn name(&self, index: usize) -> &str {
        let start = if index == 0 {
            0
        } else {
            self.ends[index - 1] as usize
        };
        &self.text[start..self.ends[index] as usize]
    }

    fn score(&self, index: usize, query: &NameQuery<'_>) -> Option<i32> {
        let name = self.name(index);
        if !query.admits(name) {
            return None;
        }
        let flags = self.flags[index];
        let penalty = i32::from(flags & FLAG_PENALTY_MASK) * PENALTY_STEP;
        name_score(name, flags & FLAG_DIRECTORY != 0, query.tokens, query.full)
            .map(|score| score - penalty)
    }

    /// The final sort in `search::search_with_generation` (score, name
    /// length, name, path depth, path length), with the id as the last tie
    /// break so the selection is deterministic.
    fn compare(&self, left: &Hit, right: &Hit) -> Ordering {
        let (a, b) = (left.index as usize, right.index as usize);
        right
            .score
            .cmp(&left.score)
            .then_with(|| self.name(a).len().cmp(&self.name(b).len()))
            .then_with(|| self.name(a).cmp(self.name(b)))
            .then_with(|| self.places[a].cmp(&self.places[b]))
            .then_with(|| self.ids[a].cmp(&self.ids[b]))
    }

    fn top_hits(
        &self,
        query: &NameQuery<'_>,
        wanted: usize,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Vec<Hit> {
        let total = self.ids.len();
        let threads = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(MAX_SCAN_THREADS)
            .min(total.div_ceil(MIN_ROWS_PER_THREAD))
            .max(1);
        let chunk = total.div_ceil(threads).max(1);

        let mut hits = if threads == 1 {
            self.scan(0..total, query, wanted, is_cancelled)
        } else {
            std::thread::scope(|scope| {
                let workers = (0..threads)
                    .map(|thread| {
                        let range = thread * chunk..((thread + 1) * chunk).min(total);
                        scope.spawn(move || self.scan(range, query, wanted, is_cancelled))
                    })
                    .collect::<Vec<_>>();
                workers
                    .into_iter()
                    .flat_map(|worker| worker.join().unwrap_or_default())
                    .collect::<Vec<_>>()
            })
        };
        self.keep_best(&mut hits, wanted);
        hits.sort_unstable_by(|a, b| self.compare(a, b));
        hits
    }

    fn scan(
        &self,
        range: std::ops::Range<usize>,
        query: &NameQuery<'_>,
        wanted: usize,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Vec<Hit> {
        let mut hits = Vec::new();
        for index in range {
            if index % CANCEL_CHECK_INTERVAL == 0 && is_cancelled() {
                return Vec::new();
            }
            // Every admission rule and scorer tier needs every query
            // character in the name, so the mask test is a necessary
            // condition that rejects most rows in one instruction.
            if self.masks[index] & query.mask != query.mask || self.flags[index] & FLAG_DELETED != 0
            {
                continue;
            }
            if let Some(score) = self.score(index, query) {
                hits.push(Hit {
                    score,
                    index: index as u32,
                    typo: false,
                });
                if hits.len() >= wanted * 8 {
                    self.keep_best(&mut hits, wanted);
                }
            }
        }
        hits
    }

    fn keep_best(&self, hits: &mut Vec<Hit>, wanted: usize) {
        if hits.len() > wanted {
            hits.select_nth_unstable_by(wanted - 1, |a, b| self.compare(a, b));
            hits.truncate(wanted);
        }
    }

    /// True when an admitted hit is a strong match (see
    /// `search::is_strong_match`).
    fn has_strong_hit(&self, hits: &[Hit], query: &NameQuery<'_>) -> bool {
        hits.iter().any(|hit| {
            let index = hit.index as usize;
            let directory = self.flags[index] & FLAG_DIRECTORY != 0;
            is_strong_match(self.name(index), directory, query.tokens, query.full)
        })
    }

    /// The rank of a close name (see `CLOSE_TYPO_SCORE`), or `None` when the
    /// name is not close.
    fn close_rank(&self, index: usize, closeness: &mut Closeness) -> Option<i32> {
        let name = self.name(index);
        let distance = closeness.distance(name)?;
        let flags = self.flags[index];
        let directory_bonus = if flags & FLAG_DIRECTORY != 0 { 20 } else { 0 };
        let penalty = i32::from(flags & FLAG_PENALTY_MASK) * PENALTY_STEP;
        Some(
            closeness.base + (TYPO_FAR - distance) as i32 * CLOSE_EDIT_STEP + directory_bonus
                - name.len().min(80) as i32
                - penalty,
        )
    }

    /// Close typo-tolerant matches (FFF's frizbee matcher, SIMD Smith-Waterman)
    /// over every live name except the `admitted` hits. Best first; each hit's
    /// score is its final rank (see `close_rank`).
    fn typo_hits(
        &self,
        query: &NameQuery<'_>,
        wanted: usize,
        admitted: &[Hit],
        closeness: &mut Closeness,
    ) -> Vec<Hit> {
        const CHUNK_SPAN: usize = TYPO_MAX_CHUNKS * SIMD_CHUNK_BYTES;
        let text = self.text.as_bytes();
        // The matcher reads every name in whole 16-byte chunks, so the last
        // chunk runs up to 15 bytes past the name. Names in the last
        // `CHUNK_SPAN` bytes read from a zero-padded copy, never past `text`.
        let tail_start = text.len().saturating_sub(CHUNK_SPAN);
        let mut tail = text[tail_start..].to_vec();
        tail.resize(tail.len() + SIMD_CHUNK_BYTES, 0);
        // Each character class of the query that a name lacks costs at least
        // one typo: a mask test rejects most names before frizbee reads them.
        let budget = query.typo_budget();
        let resolve = |index: u32, chunks: &mut [*const u8; TYPO_MAX_CHUNKS]| {
            let index = index as usize;
            if self.flags[index] & FLAG_DELETED != 0
                || (query.mask & !self.masks[index]).count_ones() > budget
            {
                return None;
            }
            let start = if index == 0 {
                0
            } else {
                self.ends[index - 1] as usize
            };
            let length = self.ends[index] as usize - start;
            let count = length.div_ceil(SIMD_CHUNK_BYTES);
            if count == 0 || count > TYPO_MAX_CHUNKS {
                return None;
            }
            // A name that starts before `tail_start` ends its last chunk at
            // most `CHUNK_SPAN` bytes later, inside `text`.
            let base = if start >= tail_start {
                tail[start - tail_start..].as_ptr()
            } else {
                text[start..].as_ptr()
            };
            for (chunk, pointer) in chunks.iter_mut().take(count).enumerate() {
                *pointer = base.wrapping_add(chunk * SIMD_CHUNK_BYTES);
            }
            Some((count, length as u16))
        };
        let threads = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(MAX_SCAN_THREADS);
        let started = std::time::Instant::now();
        let matches =
            query
                .typo_matcher()
                .match_range_parallel_resolved(self.ids.len(), &resolve, threads);
        let matched = started.elapsed();
        let match_count = matches.len();

        // Frizbee also accepts scattered matches ("chrmoe" in "chip-remove",
        // "crome" in "cargo.toml"). A misspelled name is the name's start
        // with a letter wrong: only those close names are kept.
        let mut admitted = admitted.iter().map(|hit| hit.index).collect::<Vec<_>>();
        admitted.sort_unstable();
        let mut hits = Vec::new();
        for found in matches {
            if admitted.binary_search(&found.index).is_ok() {
                continue;
            }
            let Some(score) = self.close_rank(found.index as usize, closeness) else {
                continue;
            };
            hits.push(Hit {
                score,
                index: found.index,
                typo: true,
            });
            if hits.len() >= wanted * 8 {
                self.keep_best(&mut hits, wanted);
            }
        }
        self.keep_best(&mut hits, wanted);
        hits.sort_unstable_by(|a, b| self.compare(a, b));
        if super::catalog_debug_enabled() {
            eprintln!(
                "[catalog] typo pass matches={match_count} match_ms={} rank_ms={}",
                matched.as_millis(),
                (started.elapsed() - matched).as_millis()
            );
        }
        hits
    }
}

/// Measures how close a name's start is to the query, with reused buffers
/// (it runs for every typo match).
struct Closeness {
    query: Vec<char>,
    /// `CLOSE_TYPO_SCORE` per query token.
    base: i32,
    /// Edits a close name may have.
    edits: usize,
    name: Vec<char>,
    cells: Vec<usize>,
}

impl Closeness {
    fn new(query: &NameQuery<'_>) -> Self {
        Self {
            query: query.full.chars().collect(),
            base: CLOSE_TYPO_SCORE * query.tokens.len() as i32,
            edits: if query.full.chars().count() >= 8 {
                2
            } else {
                1
            },
            name: Vec::new(),
            cells: Vec::new(),
        }
    }

    /// `prefix_distance` when at most `edits`. A close name starts
    /// with the query's first or second character, or has the first one
    /// second: other names are skipped without the full computation.
    fn distance(&mut self, name: &str) -> Option<usize> {
        let mut start = name.chars();
        let (first, second) = (start.next()?, start.next());
        let query = &self.query;
        let plausible = Some(&first) == query.first()
            || Some(&first) == query.get(1)
            || second.as_ref() == query.first();
        if !plausible {
            return None;
        }
        let distance = prefix_distance(query, name, &mut self.name, &mut self.cells);
        (distance <= self.edits).then_some(distance)
    }
}

/// Edit distance from `query` to the closest start of `name` (insertions,
/// deletions, substitutions, and swaps of two neighbours each cost one), up to
/// `TYPO_FAR`. "chrmoe" is 1 from "chrome.exe", "chip-remove" is 3.
/// `name_chars` and `cells` are scratch buffers.
fn prefix_distance(
    query: &[char],
    name: &str,
    name_chars: &mut Vec<char>,
    cells: &mut Vec<usize>,
) -> usize {
    name_chars.clear();
    name_chars.extend(name.chars().take(query.len() + 1));
    let name = &name_chars[..];
    let width = name.len() + 1;
    // Optimal string alignment over (query + 1) x (name + 1) cells.
    cells.clear();
    cells.resize((query.len() + 1) * width, 0);
    for (column, cell) in cells.iter_mut().take(width).enumerate() {
        *cell = column;
    }
    for row in 1..=query.len() {
        let wanted = query[row - 1];
        cells[row * width] = row;
        for column in 1..width {
            let cell = row * width + column;
            let found = name[column - 1];
            let mut best = (cells[cell - width - 1] + usize::from(wanted != found))
                .min(cells[cell - width] + 1)
                .min(cells[cell - 1] + 1);
            if row > 1 && column > 1 && wanted == name[column - 2] && query[row - 2] == found {
                best = best.min(cells[cell - 2 * width - 2] + 1);
            }
            cells[cell] = best;
        }
    }
    // The name may be one character shorter or longer than the query.
    let last = query.len() * width;
    (query.len().saturating_sub(1)..=query.len() + 1)
        .filter(|&column| column < width)
        .map(|column| cells[last + column])
        .min()
        .unwrap_or(TYPO_FAR)
        .min(TYPO_FAR)
}

/// One bit per lowercase letter, one for any digit, `.`, the common word
/// separators, other ASCII, and non-ASCII. A name can only match a query whose
/// mask is a subset of its own.
fn char_mask(text: &str) -> u32 {
    text.bytes().fold(0, |mask, byte| {
        mask | match byte {
            b'a'..=b'z' => 1 << (byte - b'a'),
            b'0'..=b'9' => 1 << 26,
            b'.' => 1 << 27,
            b'_' | b'-' | b' ' => 1 << 28,
            0x80..=0xff => 1 << 29,
            _ => 1 << 30,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::search::entry_score;
    use crate::catalog::types::ScannedItem;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db() -> (Database, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "prism-name-index-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let db = Database::open(&directory.join("catalog.db")).unwrap();
        (db, directory)
    }

    fn item(path: &str) -> ScannedItem {
        let path_buf = PathBuf::from(path);
        let name = path_buf.file_name().unwrap().to_string_lossy().into_owned();
        ScannedItem {
            normalized_path: path.to_lowercase(),
            display_path: path.into(),
            lower_name: name.to_lowercase(),
            name,
            parent: path_buf.parent().unwrap().to_string_lossy().into_owned(),
            is_directory: false,
            extension: None,
            modified_at: 0,
            size: 0,
        }
    }

    fn names(candidates: &[CandidateEntry]) -> Vec<&str> {
        candidates
            .iter()
            .map(|entry| entry.display_path.as_str())
            .collect()
    }

    fn query(index: &NameIndex, db: &Database, text: &str, limit: usize) -> Vec<CandidateEntry> {
        let tokens = text.split_whitespace().collect::<Vec<_>>();
        index
            .candidates(db, &NameQuery::new(text, &tokens), limit, true, &|| false)
            .unwrap()
            .expect("index answers once loaded")
            .entries
    }

    #[test]
    fn penalties_fit_the_flag_encoding() {
        for path in [
            r"C:\Windows\x",
            r"C:\$Recycle.Bin\x",
            r"C:\a\node_modules\x",
            r"C:\a\.git\x",
            r"C:\Users\me\AppData\Local\Temp\x",
            r"C:\Recovery\x",
            r"C:\pgdata\x",
            r"C:\desktop.ini",
        ] {
            let lower = PathBuf::from(path.to_lowercase());
            let name = lower.file_name().unwrap().to_string_lossy().into_owned();
            let penalty = path_penalty(path, &name);
            assert_eq!(penalty % PENALTY_STEP, 0, "{path}");
            assert!(penalty / PENALTY_STEP <= i32::from(FLAG_PENALTY_MASK));
        }
    }

    #[test]
    fn admits_exactly_what_the_sql_stages_returned() {
        for (text, name, admitted) in [
            // Exact and prefix stages: any length.
            ("pr", "pr", true),
            ("pr", "prism.exe", true),
            // Two characters: no FTS5 or subsequence stage.
            ("pr", "april.txt", false),
            ("pr", "p-r.txt", false),
            // FTS5 trigram stage: every 3+ character token is a substring.
            ("port", "report.txt", true),
            ("road 2026", "project roadmap 2026.pdf", true),
            // Short tokens are left out of the FTS5 query; the scorer
            // rejects the name later if "ab" does not match.
            ("road ab", "project roadmap.pdf", true),
            ("road maps", "project roadmap.pdf", false),
            // Subsequence stage: anchored on the token's first character.
            ("rpt", "report.txt", true),
            ("rl nt", "release-notes.txt", true),
            ("рпт", "репорт.txt", true),
            ("zzqx", "0yz0xbz8jflqxuug", false),
            ("rpt", "xreport.txt", false),
        ] {
            let tokens = text.split_whitespace().collect::<Vec<_>>();
            assert_eq!(
                NameQuery::new(text, &tokens).admits(name),
                admitted,
                "{text:?} -> {name:?}"
            );
        }
    }

    #[test]
    fn ranks_every_row_instead_of_a_truncated_pool() {
        let (db, directory) = temp_db();
        // Many noisy exact matches inserted first (low ids) used to fill the
        // SQL pools before the user's own file was reached.
        let mut rows = (0..500)
            .map(|n| item(&format!(r"C:\dev\node_modules\pkg{n}\readme.md")))
            .collect::<Vec<_>>();
        rows.push(item(r"C:\Users\me\Notes\readme.md"));
        db.insert_batch("vol", 1, &rows).unwrap();

        let index = NameIndex::default();
        index.load(&db).unwrap();
        let found = query(&index, &db, "readme.md", 5);

        assert_eq!(found[0].display_path, r"C:\Users\me\Notes\readme.md");
        assert_eq!(found.len(), MIN_CANDIDATES);
        drop(db);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn typo_matches_appear_only_without_a_strong_match() {
        let (db, directory) = temp_db();
        db.insert_batch(
            "vol",
            1,
            &[
                item(r"C:\Apps\Discord.exe"),
                item(r"C:\Docs\discount-codes.txt"),
                item(r"C:\Apps\chrome.exe"),
                item(r"C:\ui\chip-remove"),
                item(r"C:\Docs\deep_target.txt"),
                item(r"C:\Docs\appdata_target.txt"),
            ],
        )
        .unwrap();
        let index = NameIndex::default();
        index.load(&db).unwrap();
        let tokens = ["discrod"];
        let found = index
            .candidates(&db, &NameQuery::new("discrod", &tokens), 5, true, &|| false)
            .unwrap()
            .unwrap();

        // A transposition passes no admission rule; frizbee finds it.
        assert!(found.entries.is_empty());
        assert_eq!(found.typos[0].0.display_path, r"C:\Apps\Discord.exe");

        // A swapped letter at the name's start outranks a scattered
        // subsequence match ("c·h·r·m·o·e" in "chip-remove").
        let tokens = ["chrmoe"];
        let found = index
            .candidates(&db, &NameQuery::new("chrmoe", &tokens), 5, true, &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(names(&found.entries), [r"C:\ui\chip-remove"]);
        let scattered = entry_score(&found.entries[0], &tokens, "chrmoe").unwrap();
        let (typo, rank) = &found.typos[0];
        assert_eq!(typo.display_path, r"C:\Apps\chrome.exe");
        assert!(*rank > scattered);

        // A strong match: no typo matches at all.
        let tokens = ["deep_target"];
        let found = index
            .candidates(
                &db,
                &NameQuery::new("deep_target", &tokens),
                5,
                true,
                &|| false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(names(&found.entries), [r"C:\Docs\deep_target.txt"]);
        assert!(found.typos.is_empty());

        // A strong match in another source (an NTFS volume) turns typos off.
        let tokens = ["discrod"];
        let found = index
            .candidates(&db, &NameQuery::new("discrod", &tokens), 5, false, &|| {
                false
            })
            .unwrap()
            .unwrap();
        assert!(found.typos.is_empty());

        // Only close names: no scattered frizbee matches, no punctuation-only
        // queries.
        for (text, tokens) in [("crome", vec!["crome"]), ("_ _", vec!["_", "_"])] {
            let found = index
                .candidates(&db, &NameQuery::new(text, &tokens), 5, true, &|| false)
                .unwrap()
                .unwrap();
            let typos = found
                .typos
                .iter()
                .map(|(entry, _)| entry.display_path.as_str());
            assert!(
                typos.clone().all(|path| path == r"C:\Apps\chrome.exe"),
                "{text:?}: {:?}",
                typos.collect::<Vec<_>>()
            );
        }
        drop(db);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn prefix_distance_counts_edits_against_the_name_start() {
        let distance = |query: &str, name: &str| {
            let query = query.chars().collect::<Vec<_>>();
            prefix_distance(&query, name, &mut Vec::new(), &mut Vec::new())
        };
        assert_eq!(distance("chrome", "chrome.exe"), 0);
        assert_eq!(distance("chrmoe", "chrome.exe"), 1);
        assert_eq!(distance("notpead", "notepad.exe"), 1);
        assert_eq!(distance("spotfy", "spotify"), 1);
        assert_eq!(distance("photshop", "photoshop.exe"), 1);
        assert_eq!(distance("chrmoe", "chip-remove"), TYPO_FAR);
        assert_eq!(distance("abc", "x"), TYPO_FAR);
    }

    #[test]
    fn catches_up_inserts_and_drops_deleted_rows() {
        let (db, directory) = temp_db();
        db.insert_batch("vol", 1, &[item(r"C:\a\alpha-report.txt")])
            .unwrap();
        let index = NameIndex::default();
        index.load(&db).unwrap();

        db.insert_batch("vol", 1, &[item(r"C:\b\beta-report.txt")])
            .unwrap();
        // Shorter name, earlier substring position: beta outranks alpha.
        let found = query(&index, &db, "report", 10);
        assert_eq!(
            names(&found),
            vec![r"C:\b\beta-report.txt", r"C:\a\alpha-report.txt"]
        );

        db.remove_file("vol", r"c:\a\alpha-report.txt", false)
            .unwrap();
        let found = query(&index, &db, "report", 10);
        assert_eq!(names(&found), vec![r"C:\b\beta-report.txt"]);
        assert_eq!(index.len(), 2, "deleted rows are marked, not removed");
        drop(db);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn rename_is_found_under_the_new_name_only() {
        let (db, directory) = temp_db();
        db.insert_batch("vol", 1, &[item(r"C:\a\draft.txt")])
            .unwrap();
        let index = NameIndex::default();
        index.load(&db).unwrap();

        db.rename_file("vol", r"c:\a\draft.txt", &item(r"C:\a\final.txt"))
            .unwrap();

        assert!(query(&index, &db, "draft", 10).is_empty());
        assert_eq!(
            names(&query(&index, &db, "final", 10)),
            vec![r"C:\a\final.txt"]
        );
        drop(db);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn large_backlog_answers_from_the_loaded_rows() {
        let (db, directory) = temp_db();
        db.insert_batch("vol", 1, &[item(r"C:\docs\annual-report.txt")])
            .unwrap();
        let index = NameIndex::default();
        index.load(&db).unwrap();
        let rows = (0..SEARCH_CATCH_UP * 2)
            .map(|n| item(&format!(r"C:\bulk\file{n}.txt")))
            .collect::<Vec<_>>();
        db.insert_batch("vol", 1, &rows).unwrap();

        // The keystroke appends one batch only, and still finds substring
        // matches (the SQL path has no substring stage).
        let tokens = ["report"];
        let parsed = NameQuery::new("report", &tokens);
        let found = index.candidates(&db, &parsed, 10, true, &|| false).unwrap();
        assert_eq!(
            names(&found.unwrap().entries),
            [r"C:\docs\annual-report.txt"]
        );
        assert_eq!(index.len(), 1 + SEARCH_CATCH_UP);
        assert!(index.needs_sync(), "the background sync appends the rest");
        index.load(&db).unwrap();
        assert!(!index.needs_sync());
        assert_eq!(index.len(), 1 + SEARCH_CATCH_UP * 2);
        drop(db);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn char_mask_is_a_necessary_condition_for_a_match() {
        for (name, text) in [
            ("report.txt", "rpt"),
            ("release-notes.txt", "rl nt"),
            ("репорт.txt", "рпт"),
            ("a_b 2024.md", "ab24"),
        ] {
            let tokens = text.split_whitespace().collect::<Vec<_>>();
            let parsed = NameQuery::new(text, &tokens);
            assert!(name_score(name, false, &tokens, text).is_some());
            assert_eq!(
                char_mask(name) & parsed.mask,
                parsed.mask,
                "{name} / {text}"
            );
        }
    }

    /// Compares both candidate paths on a real catalog copy:
    /// `PRISM_BENCH_DB=<copy of catalog.db> cargo test --release live_catalog -- --ignored --nocapture`
    #[test]
    #[ignore = "needs PRISM_BENCH_DB pointing at a catalog copy"]
    fn live_catalog_benchmark() {
        use crate::catalog::search::search_with_generation;
        use std::sync::atomic::AtomicU64;
        use std::time::Instant;

        let path = std::env::var("PRISM_BENCH_DB").expect("PRISM_BENCH_DB");
        let db = Database::open(std::path::Path::new(&path)).unwrap();
        let index = NameIndex::default();
        let started = Instant::now();
        index.load(&db).unwrap();
        {
            let names = index.names.read().unwrap();
            let bytes = names.text.capacity()
                + names.ends.capacity() * 4
                + names.ids.capacity() * 4
                + names.masks.capacity() * 4
                + names.flags.capacity() * 2
                + names.places.capacity() * 4;
            eprintln!(
                "load: {} rows in {:.2}s, {:.0} MiB",
                names.ids.len(),
                started.elapsed().as_secs_f64(),
                bytes as f64 / 1_048_576.0
            );
        }
        let generation = AtomicU64::new(1);
        for text in [
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
        ] {
            let tokens = text.split_whitespace().collect::<Vec<_>>();
            let parsed = NameQuery::new(text, &tokens);
            let mut scan = (0..7)
                .map(|_| {
                    let started = Instant::now();
                    let names = index.names.read().unwrap();
                    names.top_hits(&parsed, 200, &|| false);
                    started.elapsed().as_secs_f64() * 1000.0
                })
                .collect::<Vec<_>>();
            scan.sort_by(|a, b| a.partial_cmp(b).unwrap());

            let mut timings = [Vec::new(), Vec::new()];
            let mut tops = [Vec::new(), Vec::new()];
            for (slot, names) in [None, Some(&index)].into_iter().enumerate() {
                for _ in 0..7 {
                    let started = Instant::now();
                    let response = search_with_generation(
                        text,
                        Some(50),
                        &db,
                        names,
                        &generation,
                        1,
                        &[],
                        0,
                        false,
                        true,
                    );
                    timings[slot].push(started.elapsed().as_secs_f64() * 1000.0);
                    tops[slot] = response
                        .items
                        .iter()
                        .take(10)
                        .map(|item| item.path.clone())
                        .collect::<Vec<_>>();
                }
                timings[slot].sort_by(|a, b| a.partial_cmp(b).unwrap());
            }
            let reference = exhaustive_top(&index, &db, &parsed, 10);
            eprintln!(
                "{text:>14}: sql {:6.1} ms | memory {:6.1} ms (scan {:5.1}) | top10 = reference: sql {} memory {}",
                timings[0][3],
                timings[1][3],
                scan[3],
                tops[0] == reference,
                tops[1] == reference
            );
            assert_eq!(tops[1], reference, "memory path diverged for {text:?}");
        }
    }

    /// The parity reference: every row the SQL stages could admit, scored and
    /// ranked with no `LIMIT` anywhere, then the same existence check and
    /// de-duplication as `search_with_generation`.
    fn exhaustive_top(
        index: &NameIndex,
        db: &Database,
        query: &NameQuery<'_>,
        limit: usize,
    ) -> Vec<String> {
        use crate::catalog::search::{entry_score, result_order};

        let ids = {
            let names = index.names.read().unwrap();
            (0..names.ids.len())
                .filter(|&i| {
                    names.flags[i] & FLAG_DELETED == 0
                        && query.admits(names.name(i))
                        && name_score(names.name(i), false, query.tokens, query.full).is_some()
                })
                .map(|i| i64::from(names.ids[i]))
                .collect::<Vec<_>>()
        };
        let mut scored = ids
            .chunks(10_000)
            .flat_map(|chunk| db.file_rows_by_id(chunk).unwrap())
            .filter_map(|lookup| match lookup {
                RowLookup::Found(entry) => {
                    entry_score(&entry, query.tokens, query.full).map(|score| (score, entry))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        scored.sort_by(result_order);
        let mut seen = std::collections::HashSet::new();
        scored
            .into_iter()
            .map(|(_, entry)| entry.display_path)
            .filter(|path| std::path::Path::new(path).exists())
            .filter(|path| seen.insert(path.to_lowercase()))
            .take(limit)
            .collect()
    }
}
