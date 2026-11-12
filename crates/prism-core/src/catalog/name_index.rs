//! In-memory name index for the fallback `files` catalog.
//!
//! The SQL candidate stages each stop at a `LIMIT` before ranking. On a
//! multi-million-row catalog that costs tens of milliseconds per keystroke
//! (about 200 ms for a common exact name such as `package.json`), and the best
//! match can fall outside the truncated pool. This index keeps every distinct
//! lowercase name once in one contiguous buffer, applies the same admission
//! rules as the SQL stages and the same scorer, scans all names in parallel,
//! expands each match to its rows, and asks SQLite only for the
//! `display_path` of the top rows.
//!
//! Most names repeat (`index.js`, `readme.md`, `.gitignore`): on a large
//! catalog only about one row in five has a name no other row has. One copy
//! per name makes the index smaller and the scan shorter by that factor.
//!
//! Sync invariant: a row's `lower_name` never changes for a given id (upserts
//! conflict on the same `normalized_path`; renames delete and insert), and new
//! rows always get a larger id (`AUTOINCREMENT`). The index therefore stays
//! current by appending rows above `max_id`, by marking the ids that the
//! incremental deletes report (`Database::take_deleted_ids`), and by dropping
//! ids that the by-id fetch no longer finds.
//!
//! File churn never forces a reload. Marked rows stay in the buffers as
//! tombstones until they are many; then `compact` builds new buffers from the
//! live ones, with no read of the catalog. Only a change that the catalog
//! cannot report by id (a whole-volume clear, a full deleted-id queue) makes
//! the index reload.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, RwLock};

use neo_frizbee::{Config, Matcher, Pattern, PatternConfig, SortStrategy, SIMD_CHUNK_BYTES};

use super::db::Database;
use super::search::{is_strong_match, name_score, path_depth, path_penalty};
use super::types::{CandidateEntry, NameRow, RowLookup};

/// Rows per SQLite read while loading. One batch holds the reader connection
/// for tens of milliseconds, so searches on the SQL path keep running.
const LOAD_BATCH: usize = 100_000;
/// Ids per span of the parallel full load: about 20,000 rows on a catalog
/// whose ids are a third dense.
const LOAD_SPAN_IDS: usize = 1 << 16;
const MAX_LOAD_THREADS: usize = 8;
/// New rows a search appends inline before it answers. A larger backlog (a
/// full rescan) goes to the background loader and the search uses SQL.
const SEARCH_CATCH_UP: usize = 10_000;
/// Rewritten rows (a directory rename) a search applies inline. A larger
/// rename is applied by the background sync, outside the keystroke.
const SEARCH_REWRITES: usize = 2_000;
/// Deleted ids a search applies inline (a binary search each, under the write
/// lock). The background sync applies the rest.
const SEARCH_DELETIONS: usize = 10_000;
/// Entries (or names) that `compact` copies under one read lock. Between two
/// slices the lock is free, so a writer that waits (a catch-up of a search)
/// never holds the searches back for long.
const COMPACT_SLICE: usize = 1 << 18;
const MAX_SCAN_THREADS: usize = 8;
const MIN_ROWS_PER_THREAD: usize = 100_000;
const CANCEL_CHECK_INTERVAL: usize = 16_384;
/// Rows selected for the by-id fetch per result slot. The surplus covers rows
/// that the final disk-existence check rejects; the SQL candidate path kept
/// `max(limit * 15, 150)`, so the index keeps the same pool (the by-id fetch
/// is batched, so the larger pool stays cheap).
const CANDIDATES_PER_RESULT: usize = 15;
const MIN_CANDIDATES: usize = 150;
/// Deleted rows that the catalog did not report by id and the index has not
/// seen before it reloads: a share of its rows, at least this many (bulk
/// deletes leave dead rows it would otherwise find only one search at a time).
const MIN_RELOAD_DELETIONS: u64 = 50_000;
/// Tombstones that start a compaction: a share of the entries, at least this
/// many.
const COMPACT_SHARE: usize = 4;
const MIN_COMPACT_TOMBSTONES: usize = 10_000;
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
/// End of a row chain (see `Names::heads`).
const NO_ENTRY: u32 = u32::MAX;
/// The name table grows when more than this share of its slots is in use.
/// Linear probing stays short below it, and the table stays at four bytes a
/// slot.
const TABLE_LOAD_NUMERATOR: usize = 5;
const TABLE_LOAD_DENOMINATOR: usize = 8;
const MIN_TABLE_SLOTS: usize = 1 << 10;
/// Headroom of each buffer, as a share of its length: 1/32 is about 4 MiB on
/// a large catalog and holds about 180,000 new rows. The load leaves that
/// much free, and a full buffer grows by that much.
const GROWTH_SHARE: usize = 32;

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
/// Typo hits per query: near-identical neighbours ("file-0043" for
/// "fle-0042") would otherwise fill the list.
const MAX_TYPO_HITS: usize = 5;
/// Prefix edit distances from this value up count the same.
const TYPO_FAR: usize = 3;

pub struct NameIndex {
    names: RwLock<Names>,
    loaded: AtomicBool,
    loader_running: AtomicBool,
    reload_requested: AtomicBool,
    /// A search found more new rows than it may append inline.
    behind: AtomicBool,
    /// There are enough tombstones that `compact` should run.
    compact_requested: AtomicBool,
    /// `Database::rows_deleted` when the current rows were loaded.
    seen_deleted: AtomicU64,
    /// `Some` while new buffers are built beside the live index (a reload or
    /// a compaction). It holds the ids that live catch-ups took from the
    /// database meanwhile: the new buffers may have read those rows before the
    /// change, so the swap applies the ids to them again.
    reload_log: Mutex<Option<ChangeLog>>,
}

/// The ids that live catch-ups took while new buffers were built (see
/// `NameIndex::reload_log`).
#[derive(Default)]
struct ChangeLog {
    rewritten: Vec<i64>,
    deleted: Vec<i64>,
}

/// How many rewritten and deleted ids one catch-up applies.
#[derive(Clone, Copy)]
struct CatchUpLimits {
    rows: usize,
    rewrites: usize,
    deletions: usize,
}

impl CatchUpLimits {
    /// A search: a few rows and ids, so the keystroke stays short.
    const SEARCH: Self = Self {
        rows: SEARCH_CATCH_UP,
        rewrites: SEARCH_REWRITES,
        deletions: SEARCH_DELETIONS,
    };
    /// The background sync: everything.
    const ALL: Self = Self {
        rows: LOAD_BATCH,
        rewrites: usize::MAX,
        deletions: usize::MAX,
    };
}

/// What `NameIndex::compact` did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compaction {
    /// Tombstones the new buffers do not hold.
    pub dropped: usize,
    /// Entries of the new buffers.
    pub kept: usize,
    /// Time to build the new buffers (searches run meanwhile).
    pub build: std::time::Duration,
    /// Time the swap held the write lock.
    pub locked: std::time::Duration,
}

/// Two levels: distinct names (indexed by a name number) and rows (indexed by
/// an entry number, in id order). A `Hit` carries both numbers, so no
/// per-row name number is stored.
#[derive(Default)]
struct Names {
    /// Each distinct lowercase name once, back to back.
    text: String,
    /// End offset of each distinct name in `text`.
    ends: Vec<u32>,
    /// Characters present in each distinct name (see `char_mask`).
    masks: Vec<u32>,
    /// Newest row of each distinct name. The older rows with that name
    /// follow `next`: a push links a row in front, so appends after the load
    /// cost no more than rows of the load.
    heads: Vec<u32>,
    /// Open-addressing hash table over the distinct names: name number + 1,
    /// or 0 for a free slot. Its length is a power of two. Probes compare the
    /// name text, so no hash is stored.
    table: Vec<u32>,
    /// Row id of each entry.
    ids: Vec<u32>,
    /// Directory flag, deleted flag, and location penalty / `PENALTY_STEP`.
    flags: Vec<u16>,
    /// Path depth (high 16 bits) and `display_path` byte length (low 16
    /// bits): the final sort's tie breaks after the name.
    places: Vec<u32>,
    /// Next older entry with the same name, or `NO_ENTRY`.
    next: Vec<u32>,
    max_id: i64,
    deleted: usize,
    /// Deleted ids above `max_id`, ascending: a catch-up read these rows but
    /// had not appended them when the ids came. `push_rows` applies them.
    deleted_ahead: Vec<u32>,
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
    /// Entry number.
    index: u32,
    /// Name number.
    name: u32,
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
            compact_requested: AtomicBool::new(false),
            seen_deleted: AtomicU64::new(0),
            reload_log: Mutex::new(None),
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
                    // Searches during a load from empty used the SQL path,
                    // which has no substring stage: an open palette searches
                    // again. Catch-ups do not: the watcher reports changes.
                    Ok(true) => crate::host::emit("file-index-updated", serde_json::Value::Null),
                    Ok(false) => {}
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
        self.compact_requested.store(false, AtomicOrdering::Release);
        let mut names = self.names.write().unwrap_or_else(|e| e.into_inner());
        let generation = names.generation.wrapping_add(1);
        *names = Names { generation, ..Names::default() };
    }

    /// True when the background loader has work: the first load, a backlog
    /// that a search deferred, a compaction, or enough deleted rows that a
    /// fresh load is cheaper than carrying them.
    pub fn needs_sync(&self) -> bool {
        !self.is_loaded()
            || self.behind.load(AtomicOrdering::Acquire)
            || self.reload_requested.load(AtomicOrdering::Acquire)
            || self.compact_requested.load(AtomicOrdering::Acquire)
    }

    /// `needs_sync`, plus the catalog's signals: a change too large to apply
    /// row by row, a backlog of deleted ids, or enough deletions that a
    /// reload beats carrying them.
    pub fn needs_sync_with(&self, db: &Database) -> bool {
        if db.take_index_reload_request() {
            self.reload_requested.store(true, AtomicOrdering::Release);
        }
        if self.is_loaded() {
            let rows = self.names.read().unwrap_or_else(|e| e.into_inner()).ids.len() as u64;
            // Only deletions that were not reported by id count here: the
            // others are marked row by row.
            let unseen = db.rows_deleted().saturating_sub(self.seen_deleted.load(AtomicOrdering::Acquire));
            if unseen > (rows / 8).max(MIN_RELOAD_DELETIONS) {
                self.reload_requested.store(true, AtomicOrdering::Release);
            }
            // More ids than a search applies inline: the loader takes them.
            if db.pending_deleted_ids() > SEARCH_DELETIONS {
                self.behind.store(true, AtomicOrdering::Release);
            }
        }
        self.needs_sync()
    }

    /// Synchronous full load (or catch-up), batch by batch. Returns true when
    /// it loaded from an empty index (the first load or a full reload).
    pub fn load(&self, db: &Database) -> Result<bool, String> {
        // No low memory priority here: this thread fills the live buffers,
        // and their pages must stay in memory for searches. Only the catalog
        // reads (the prefetch and the span readers) run at low priority.
        let cold = !self.is_loaded();
        let from_empty = self.reload_requested.swap(false, AtomicOrdering::AcqRel) || cold;
        if !from_empty {
            let loaded = self.load_rows(db, false)?;
            // The catch-up may have marked many rows deleted.
            if self.compact_requested.swap(false, AtomicOrdering::AcqRel) {
                self.compact(db)?;
                // Rows that came during the compaction.
                return self.load_rows(db, false);
            }
            return Ok(loaded);
        }
        // New buffers hold no tombstones.
        self.compact_requested.store(false, AtomicOrdering::Release);
        if !cold {
            // The catalog file is in the cache already, and a prefetch of
            // 4 GB would push the app's pages out.
            return self.load_rows(db, true);
        }
        // The first load reads most of the catalog from a cold cache: bring
        // the file into the cache with large reads beside it, so its
        // scattered page reads hit.
        std::thread::scope(|scope| {
            let _ = std::thread::Builder::new()
                .name("prism-catalog-prefetch".into())
                .spawn_scoped(scope, || db.prefetch_file());
            self.load_rows(db, true)
        })
    }

    /// Asks the next `load` to rebuild the index from the catalog. For tools
    /// and tests: the app sets this flag itself.
    pub fn request_reload(&self) {
        self.reload_requested.store(true, AtomicOrdering::Release);
    }

    fn load_rows(&self, db: &Database, from_empty: bool) -> Result<bool, String> {
        if from_empty {
            if self.is_loaded() {
                return self.reload(db);
            }
            self.seen_deleted.store(db.rows_deleted(), AtomicOrdering::Release);
            // Size the buffers once. Growing them batch by batch copied the
            // name text (over 100 MB on a large catalog) about 30 times, with
            // the old and new copies alive together at each step. The text
            // total counts repeated names too, so `shrink_names` gives the
            // surplus back when the load ends.
            // A retry after a failed load keeps the rows it already has.
            if let Ok((rows, bytes)) = db.file_name_totals() {
                let mut names = self.names.write().unwrap_or_else(|e| e.into_inner());
                let (held_rows, held_bytes) = (names.ids.len(), names.text.len());
                // With the headroom: the first appends after the load (new
                // files, on the next search) then move no buffer.
                names.reserve(
                    (rows + rows / GROWTH_SHARE).saturating_sub(held_rows),
                    bytes.saturating_sub(held_bytes),
                );
            }
            Self::load_spans(&self.names, db)?;
        }
        while !self.catch_up(db, CatchUpLimits::ALL)? {}
        if from_empty {
            self.names
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .shrink_names();
        }
        self.behind.store(false, AtomicOrdering::Release);
        self.loaded.store(true, AtomicOrdering::Release);
        Ok(from_empty)
    }

    /// Full load of an index that is already loaded. The rows go into new
    /// buffers, so searches keep using the live index meanwhile; the new
    /// buffers replace it in one swap. Returns false when a reset during the
    /// load made the new rows obsolete.
    fn reload(&self, db: &Database) -> Result<bool, String> {
        let generation = self.names.read().unwrap_or_else(|e| e.into_inner()).generation;
        self.seen_deleted.store(db.rows_deleted(), AtomicOrdering::Release);
        *self.reload_log.lock().unwrap_or_else(|e| e.into_inner()) = Some(ChangeLog::default());
        let result = self.load_beside(db, generation);
        // The swap stops the log; an abort or an error stops it here.
        *self.reload_log.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if result.is_err() {
            // The live index stays; try again on the next sync.
            self.reload_requested.store(true, AtomicOrdering::Release);
        }
        result
    }

    /// `reload` after the log starts. `generation` is the live generation at
    /// the start.
    fn load_beside(&self, db: &Database, generation: u64) -> Result<bool, String> {
        let fresh = Self::build_beside(db, generation)?;
        if self.swap_in(db, fresh, generation).is_none() {
            return Ok(false);
        }
        self.behind.store(false, AtomicOrdering::Release);
        self.loaded.store(true, AtomicOrdering::Release);
        Ok(true)
    }

    /// New buffers with every row of the catalog. `generation` is the live
    /// generation at the start.
    fn build_beside(db: &Database, generation: u64) -> Result<Names, String> {
        // The new generation, so a search that read indexes from the old
        // buffers cannot apply them to the new ones (see `mark_deleted`).
        let fresh = RwLock::new(Names {
            generation: generation.wrapping_add(1),
            ..Names::default()
        });
        // Size the buffers as the first load does (see `load_rows`).
        if let Ok((rows, bytes)) = db.file_name_totals() {
            fresh
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .reserve(rows + rows / GROWTH_SHARE, bytes);
        }
        Self::load_spans(&fresh, db)?;
        while !Self::catch_up_in(&fresh, None, db, CatchUpLimits::ALL)? {}
        let mut fresh = fresh.into_inner().unwrap_or_else(|e| e.into_inner());
        fresh.shrink_names();
        Ok(fresh)
    }

    /// Replaces the live buffers with `fresh`, which was built beside them
    /// from the live generation `generation`. Returns how long the write lock
    /// was held, or `None` when a reset in between made `fresh` obsolete.
    /// Stops the log.
    fn swap_in(
        &self,
        db: &Database,
        fresh: Names,
        generation: u64,
    ) -> Option<std::time::Duration> {
        let locked_at = std::time::Instant::now();
        let (old, logged, locked) = {
            let mut live = self.names.write().unwrap_or_else(|e| e.into_inner());
            // Under the write lock: a live catch-up that takes ids after this
            // point applies them to the new buffers.
            let logged = self.reload_log.lock().unwrap_or_else(|e| e.into_inner()).take();
            // A reset in between: the new rows belong to a wiped catalog.
            if live.generation != generation {
                return None;
            }
            let mut old = std::mem::replace(&mut *live, fresh);
            // Ids that wait for rows the new buffers may hold now.
            let ahead = std::mem::take(&mut old.deleted_ahead);
            live.deleted_ahead.extend(ahead);
            live.deleted_ahead.sort_unstable();
            live.deleted_ahead.dedup();
            live.settle_deleted_ahead();
            (old, logged.unwrap_or_default(), locked_at.elapsed())
        };
        // The old buffers are large: free them outside the lock.
        drop(old);
        // Rows that the new buffers read before a rewrite or a delete
        // committed are stale now, and the live index took their ids. Apply
        // them again (the apply is idempotent). Ids that the new buffers' own
        // catch-up took were applied to them.
        // On a read error the ids go back to the database queue, and the next
        // catch-up applies them: the swap itself succeeded.
        Self::apply_logged(&self.names, db, logged);
        Some(locked)
    }

    /// Applies the ids that a log kept (see `reload_log`) to the buffers
    /// that replaced the ones the log followed.
    fn apply_logged(shared: &RwLock<Names>, db: &Database, logged: ChangeLog) {
        Self::apply_deleted(shared, None, logged.deleted);
        let _ = Self::apply_rewritten(shared, None, db, logged.rewritten);
    }

    /// Appends every row up to the highest id at the start, read on several
    /// threads (see `Database::file_names_in_spans`). Rows added later and
    /// rewritten rows are left to `catch_up`.
    fn load_spans(shared: &RwLock<Names>, db: &Database) -> Result<(), String> {
        let Some((first, last)) = db.file_id_bounds()? else {
            return Ok(());
        };
        let (after, generation) = {
            let names = shared.read().unwrap_or_else(|e| e.into_inner());
            (names.max_id.max(first - 1), names.generation)
        };
        let spans = (after..last)
            .step_by(LOAD_SPAN_IDS)
            .map(|start| (start, start.saturating_add(LOAD_SPAN_IDS as i64).min(last)))
            .collect::<Vec<_>>();
        let threads = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(MAX_LOAD_THREADS);
        let mut reset = false;
        let loaded = db.file_names_in_spans(&spans, threads, |rows| {
            let mut names = shared.write().unwrap_or_else(|e| e.into_inner());
            // A reset in between: these rows belong to the old buffers, and
            // `catch_up` loads the new ones.
            if names.generation != generation {
                reset = true;
                return Err("the name index was reset during the load".into());
            }
            let fresh = rows.partition_point(|row| row.id <= names.max_id);
            names.push_rows(&rows[fresh..])
        });
        match loaded {
            Err(_) if reset => Ok(()),
            loaded => loaded,
        }
    }

    /// Marks the deleted ids, applies up to `rewrites` rewritten rows, and
    /// appends up to `rows` rows above `max_id`. Returns true when the index
    /// has caught up with the catalog.
    fn catch_up(&self, db: &Database, limits: CatchUpLimits) -> Result<bool, String> {
        let caught_up = Self::catch_up_in(&self.names, Some(&self.reload_log), db, limits)?;
        let names = self.names.read().unwrap_or_else(|e| e.into_inner());
        if names.has_many_tombstones() {
            self.compact_requested.store(true, AtomicOrdering::Release);
        }
        Ok(caught_up)
    }

    /// `catch_up` on any buffers. `log` is the live index's reload log (see
    /// `reload_log`); the buffers of a reload pass `None`.
    fn catch_up_in(
        shared: &RwLock<Names>,
        log: Option<&Mutex<Option<ChangeLog>>>,
        db: &Database,
        limits: CatchUpLimits,
    ) -> Result<bool, String> {
        let max_rows = limits.rows;
        if !Self::refresh_deleted(shared, log, db, limits.deletions) {
            return Ok(false);
        }
        if !Self::refresh_rewritten(shared, log, db, limits.rewrites)? {
            return Ok(false);
        }
        let (after, generation) = {
            let names = shared.read().unwrap_or_else(|e| e.into_inner());
            (names.max_id, names.generation)
        };
        let rows = db.file_names_after(after, max_rows)?;
        let caught_up = rows.len() < max_rows;
        if !rows.is_empty() {
            let mut names = shared.write().unwrap_or_else(|e| e.into_inner());
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
            names.push_rows(rows)?;
        }
        Ok(caught_up)
    }

    /// Rows that incremental deletes removed: marks them deleted by id, with
    /// no read of the catalog. Applies up to `max` ids; returns false when
    /// more are left.
    fn refresh_deleted(
        shared: &RwLock<Names>,
        log: Option<&Mutex<Option<ChangeLog>>>,
        db: &Database,
        max: usize,
    ) -> bool {
        let mut ids = db.take_deleted_ids(max.saturating_add(1));
        if ids.is_empty() {
            return true;
        }
        let more = ids.len() > max;
        if more {
            db.restore_deleted_ids(ids.split_off(max));
        }
        Self::apply_deleted(shared, log, ids);
        !more
    }

    /// Marks the rows behind `ids` deleted. An id below `max_id` that the
    /// index does not hold (a row added and deleted before the index saw it)
    /// is skipped; an id above it waits in `deleted_ahead` for its row.
    ///
    /// `log`: the change log of a reload or a compaction in progress. The ids
    /// go into it under the write lock: a build that starts after this lock
    /// copies the marks, and one that started before gets the ids from the log.
    fn apply_deleted(shared: &RwLock<Names>, log: Option<&Mutex<Option<ChangeLog>>>, mut ids: Vec<i64>) {
        if ids.is_empty() {
            return;
        }
        ids.sort_unstable();
        ids.dedup();
        let mut names = shared.write().unwrap_or_else(|e| e.into_inner());
        if let Some(log) = log {
            if let Some(logged) = log.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                logged.deleted.extend_from_slice(&ids);
            }
        }
        let ids: Vec<u32> = ids.into_iter().filter_map(|id| u32::try_from(id).ok()).collect();
        let max_id = names.max_id;
        let held = ids.partition_point(|&id| i64::from(id) <= max_id);
        names.mark_deleted_ids(&ids[..held]);
        if held < ids.len() {
            names.deleted_ahead.extend_from_slice(&ids[held..]);
            names.deleted_ahead.sort_unstable();
            names.deleted_ahead.dedup();
        }
    }

    /// Rows whose path changed in place (a directory rename moved them):
    /// recompute their location penalty, depth, and path length. Applies up
    /// to `max` rows; returns false when more are left.
    fn refresh_rewritten(
        shared: &RwLock<Names>,
        log: Option<&Mutex<Option<ChangeLog>>>,
        db: &Database,
        max: usize,
    ) -> Result<bool, String> {
        let mut ids = db.take_rewritten_ids(max.saturating_add(1));
        if ids.is_empty() {
            return Ok(true);
        }
        let more = ids.len() > max;
        if more {
            db.restore_rewritten_ids(ids.split_off(max));
        }
        Self::apply_rewritten(shared, log, db, ids)?;
        Ok(!more)
    }

    /// Reads the rows behind `ids` and applies their new location data. On a
    /// read error the ids go back to the database for the next call. `log`:
    /// as in `apply_deleted`.
    fn apply_rewritten(
        shared: &RwLock<Names>,
        log: Option<&Mutex<Option<ChangeLog>>>,
        db: &Database,
        mut ids: Vec<i64>,
    ) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
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
        let mut names = shared.write().unwrap_or_else(|e| e.into_inner());
        if let Some(log) = log {
            if let Some(logged) = log.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                logged.rewritten.extend_from_slice(&ids);
            }
        }
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
                    names.mark_deleted(index);
                }
                RowLookup::Excluded => {}
            }
        }
        Ok(())
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
        if !self.catch_up(db, CatchUpLimits::SEARCH)? {
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
                        if let Some(rank) = names.close_name_rank(hit.name as usize, &mut closeness)
                        {
                            *hit = Hit {
                                score: names.close_entry_rank(rank, hit.index as usize),
                                typo: true,
                                ..*hit
                            };
                        }
                    }
                    let typos = names.typo_hits(query, MAX_TYPO_HITS, &hits, &mut closeness);
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

    /// Marks rows that a search found missing in the catalog.
    fn mark_deleted(&self, generation: u64, indexes: &[u32]) {
        let mut names = self.names.write().unwrap_or_else(|e| e.into_inner());
        // Indexes from an older generation point into different rows.
        if names.generation != generation {
            return;
        }
        let mut marked = Vec::new();
        for &index in indexes {
            if (index as usize) < names.ids.len() && names.mark_deleted(index as usize) {
                marked.push(i64::from(names.ids[index as usize]));
            }
        }
        // New buffers that are built beside this index may hold these rows
        // live: the swap marks them again (see `reload_log`).
        if !marked.is_empty() {
            if let Some(logged) = self.reload_log.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                logged.deleted.extend(marked);
            }
        }
        if names.has_many_tombstones() {
            self.compact_requested.store(true, AtomicOrdering::Release);
        }
    }

    /// Builds new buffers from the live ones without the tombstones, and
    /// swaps them in. No catalog read: the live buffers hold every name and
    /// every flag. Searches run during the build (it holds the read lock in
    /// slices). Returns `None` when there is nothing to do or a reset made
    /// the build obsolete.
    pub fn compact(&self, db: &Database) -> Result<Option<Compaction>, String> {
        if !self.is_loaded() {
            return Ok(None);
        }
        let started = std::time::Instant::now();
        // A log of the ids that live catch-ups take from now on (see
        // `reload_log`). A reload and a compaction both run on the loader
        // thread: the log is free.
        {
            let mut log = self.reload_log.lock().unwrap_or_else(|e| e.into_inner());
            if log.is_some() {
                return Ok(None);
            }
            *log = Some(ChangeLog::default());
        }
        let built = Names::compacted(&self.names);
        let Some((fresh, dropped)) = built else {
            *self.reload_log.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Ok(None);
        };
        let build = started.elapsed();
        let kept = fresh.ids.len();
        let generation = fresh.generation.wrapping_sub(1);
        Ok(self
            .swap_in(db, fresh, generation)
            .map(|locked| Compaction { dropped, kept, build, locked }))
    }

}

impl Names {
    /// Marks entry `index` deleted. Returns true when it was live.
    fn mark_deleted(&mut self, index: usize) -> bool {
        let Some(flags) = self.flags.get_mut(index) else { return false };
        if *flags & FLAG_DELETED != 0 {
            return false;
        }
        *flags |= FLAG_DELETED;
        self.deleted += 1;
        true
    }

    /// Marks the entries of `ids` (ascending, all at or below `max_id`).
    fn mark_deleted_ids(&mut self, ids: &[u32]) {
        // Both lists ascend: each search starts where the last one ended.
        let mut from = 0;
        for &id in ids {
            match self.ids[from..].binary_search(&id) {
                Ok(offset) => {
                    self.mark_deleted(from + offset);
                    from += offset + 1;
                }
                Err(offset) => from += offset,
            }
        }
    }

    /// Applies the waiting deleted ids that `max_id` has reached. An id that
    /// is reached but not held belongs to a row the catalog never gave the
    /// index (added and deleted between two catch-ups): it is dropped.
    fn settle_deleted_ahead(&mut self) {
        if self.deleted_ahead.is_empty() {
            return;
        }
        let max_id = self.max_id;
        let reached = self.deleted_ahead.partition_point(|&id| i64::from(id) <= max_id);
        if reached == 0 {
            return;
        }
        let ids: Vec<u32> = self.deleted_ahead.drain(..reached).collect();
        self.mark_deleted_ids(&ids);
    }

    /// True when the tombstones are many enough to compact.
    fn has_many_tombstones(&self) -> bool {
        self.deleted > (self.ids.len() / COMPACT_SHARE).max(MIN_COMPACT_TOMBSTONES)
    }

    /// Grows every buffer with a small headroom (`GROWTH_SHARE`) instead of
    /// doubling: the name buffer is tens of MB on a large catalog, so
    /// doubling would leave up to half of it as dead capacity. `rows` and
    /// `bytes` are upper bounds: a row whose name is already present adds no
    /// name.
    fn reserve(&mut self, rows: usize, bytes: usize) {
        fn grow<T>(buffer: &mut Vec<T>, extra: usize) {
            if buffer.capacity() - buffer.len() < extra {
                buffer.reserve_exact(extra.max(buffer.len() / GROWTH_SHARE));
            }
        }
        if self.text.capacity() - self.text.len() < bytes {
            self.text.reserve_exact(bytes.max(self.text.len() / GROWTH_SHARE));
        }
        grow(&mut self.ends, rows);
        grow(&mut self.masks, rows);
        grow(&mut self.heads, rows);
        grow(&mut self.ids, rows);
        grow(&mut self.flags, rows);
        grow(&mut self.places, rows);
        grow(&mut self.next, rows);
    }

    /// Gives back the name capacity that `reserve` took for repeated names,
    /// down to the headroom. A full load reserves the text of every row; the
    /// distinct names need about a third of it.
    fn shrink_names(&mut self) {
        self.text.shrink_to(self.text.len() + self.text.len() / GROWTH_SHARE);
        fn fit<T>(buffer: &mut Vec<T>) {
            buffer.shrink_to(buffer.len() + buffer.len() / GROWTH_SHARE);
        }
        fit(&mut self.ends);
        fit(&mut self.masks);
        fit(&mut self.heads);
    }

    /// Pushes `rows` in order. Most rows repeat a name whose table slot, end
    /// offset, row chain head, and text are cold in the cache: each is
    /// prefetched some rows ahead, so the cache misses of several rows
    /// overlap. The prefetches read the first slot only; `intern` decides.
    fn push_rows(&mut self, rows: &[NameRow]) -> Result<(), String> {
        const AHEAD: usize = 16;
        let hashes = rows
            .iter()
            .map(|row| name_hash(&row.lower_name))
            .collect::<Vec<_>>();
        for (index, row) in rows.iter().enumerate() {
            if let Some(&hash) = hashes.get(index + AHEAD) {
                if let Some(slot) = self
                    .table
                    .get(slot_of(hash, self.table.len().wrapping_sub(1)))
                {
                    prefetch(slot);
                }
            }
            if let Some(&hash) = hashes.get(index + AHEAD / 2) {
                if let Some(name) = self.first_candidate(hash) {
                    prefetch(&self.ends[name.saturating_sub(1)]);
                    prefetch(&self.heads[name]);
                }
            }
            if let Some(&hash) = hashes.get(index + AHEAD / 4) {
                if let Some(name) = self.first_candidate(hash) {
                    let start = if name == 0 {
                        0
                    } else {
                        self.ends[name - 1] as usize
                    };
                    if let Some(byte) = self.text.as_bytes().get(start) {
                        prefetch(byte);
                    }
                }
            }
            self.push(row, hashes[index])?;
        }
        self.settle_deleted_ahead();
        Ok(())
    }

    /// The name in the first slot for `hash`, if any.
    fn first_candidate(&self, hash: u64) -> Option<usize> {
        let stored = *self
            .table
            .get(slot_of(hash, self.table.len().wrapping_sub(1)))?;
        (stored != 0).then(|| stored as usize - 1)
    }

    fn push(&mut self, row: &NameRow, hash: u64) -> Result<(), String> {
        let id = u32::try_from(row.id).map_err(|_| format!("row id {} exceeds u32", row.id))?;
        let entry = u32::try_from(self.ids.len())
            .ok()
            .filter(|&entry| entry != NO_ENTRY)
            .ok_or_else(|| "name index exceeds u32 rows".to_string())?;
        let penalty = row.penalty;
        debug_assert_eq!(
            penalty % PENALTY_STEP,
            0,
            "penalty must be a multiple of the step"
        );
        let mut flags = (penalty / PENALTY_STEP) as u16 & FLAG_PENALTY_MASK;
        if row.is_directory {
            flags |= FLAG_DIRECTORY;
        }
        let depth = row.depth.min(0xffff) as u32;
        let length = row.path_len.min(0xffff) as u32;
        let name = self.intern(&row.lower_name, hash)? as usize;
        self.ids.push(id);
        self.flags.push(flags);
        self.places.push(depth << 16 | length);
        self.next.push(self.heads[name]);
        self.heads[name] = entry;
        self.max_id = row.id;
        Ok(())
    }

    /// The name number of `name` (whose `name_hash` is `hash`), added when
    /// it is new.
    fn intern(&mut self, name: &str, hash: u64) -> Result<u32, String> {
        if (self.ends.len() + 1) * TABLE_LOAD_DENOMINATOR > self.table.len() * TABLE_LOAD_NUMERATOR
        {
            self.grow_table();
        }
        let mask = self.table.len() - 1;
        let mut slot = slot_of(hash, mask);
        loop {
            match self.table[slot] {
                0 => break,
                stored if self.name(stored as usize - 1) == name => return Ok(stored - 1),
                _ => slot = (slot + 1) & mask,
            }
        }
        let end = u32::try_from(self.text.len() + name.len())
            .map_err(|_| "name index exceeds 4 GiB".to_string())?;
        let number = self.ends.len() as u32;
        self.text.push_str(name);
        self.ends.push(end);
        self.masks.push(char_mask(name));
        self.heads.push(NO_ENTRY);
        self.table[slot] = number + 1;
        Ok(number)
    }

    /// Doubles the name table and places every name again.
    fn grow_table(&mut self) {
        self.place_names((self.table.len() * 2).max(MIN_TABLE_SLOTS));
    }

    /// Fills a new name table of `slots` slots (a power of two). The text
    /// holds the names, so the hashes are computed again instead of stored.
    fn place_names(&mut self, slots: usize) {
        let mut table = vec![0u32; slots];
        let mask = slots - 1;
        for number in 0..self.ends.len() {
            let mut slot = slot_of(name_hash(self.name(number)), mask);
            while table[slot] != 0 {
                slot = (slot + 1) & mask;
            }
            table[slot] = number as u32 + 1;
        }
        self.table = table;
    }

    /// New buffers with the live entries of `shared` only, in id order, and
    /// the number of tombstones left out. `None` when there are no
    /// tombstones or a reset ran during the build. The new generation is the
    /// old one plus one, so a search that read indexes from the old buffers
    /// cannot mark rows in the new ones (the entry numbers moved).
    ///
    /// The build copies a slice of entries (or names) under one read lock and
    /// frees the lock between slices. What changes in between is safe: the
    /// entries below the snapshot keep their place (only their flags and
    /// places change; the caller applies the logged ids to the result), and
    /// new entries lie above it (`max_id` of the result is the snapshot's, so
    /// the next catch-up reads them again).
    fn compacted(shared: &RwLock<Names>) -> Option<(Names, usize)> {
        let (generation, entries, name_count, text_bytes, max_id) = {
            let names = shared.read().unwrap_or_else(|e| e.into_inner());
            if names.deleted == 0 {
                return None;
            }
            (
                names.generation,
                names.ids.len(),
                names.ends.len(),
                names.text.len(),
                names.max_id,
            )
        };
        let slices = |count: usize| (0..count).step_by(COMPACT_SLICE).map(move |start| start..(start + COMPACT_SLICE).min(count));
        let current = |names: &Names| names.generation == generation;

        // The name number of each entry, from the row chains.
        let mut name_of = vec![0u32; entries];
        for slice in slices(name_count) {
            let names = shared.read().unwrap_or_else(|e| e.into_inner());
            if !current(&names) {
                return None;
            }
            for name in slice {
                let mut entry = names.heads[name];
                while entry != NO_ENTRY {
                    let index = entry as usize;
                    // Rows added after the snapshot head the chain.
                    if index < entries {
                        name_of[index] = name as u32;
                    }
                    entry = names.next[index];
                }
            }
        }
        // The names that keep a live entry.
        let mut name_live = vec![false; name_count];
        let mut live_entries = 0usize;
        for slice in slices(entries) {
            let names = shared.read().unwrap_or_else(|e| e.into_inner());
            if !current(&names) {
                return None;
            }
            for entry in slice {
                if names.flags[entry] & FLAG_DELETED == 0 {
                    name_live[name_of[entry] as usize] = true;
                    live_entries += 1;
                }
            }
        }
        let live_names = name_live.iter().filter(|&&live| live).count();

        let mut fresh = Names {
            generation: generation.wrapping_add(1),
            max_id,
            ..Names::default()
        };
        fn with_headroom<T>(count: usize) -> Vec<T> {
            Vec::with_capacity(count + count / GROWTH_SHARE)
        }
        // The headroom is for the rows that come next (see `GROWTH_SHARE`).
        // The text counts repeated names too: `shrink_names` gives the
        // surplus back.
        fresh.text = String::with_capacity(text_bytes);
        fresh.ends = with_headroom(live_names);
        fresh.masks = with_headroom(live_names);
        fresh.heads = with_headroom(live_names);

        // The live names, in the old order.
        let mut new_number = vec![NO_ENTRY; name_count];
        for slice in slices(name_count) {
            let names = shared.read().unwrap_or_else(|e| e.into_inner());
            if !current(&names) {
                return None;
            }
            for name in slice {
                if !name_live[name] {
                    continue;
                }
                new_number[name] = fresh.ends.len() as u32;
                fresh.text.push_str(names.name(name));
                fresh.ends.push(fresh.text.len() as u32);
                fresh.masks.push(names.masks[name]);
                fresh.heads.push(NO_ENTRY);
            }
        }
        fresh.shrink_names();
        let mut slots = MIN_TABLE_SLOTS;
        while (live_names + 1) * TABLE_LOAD_DENOMINATOR > slots * TABLE_LOAD_NUMERATOR {
            slots *= 2;
        }
        fresh.place_names(slots);

        // The live entries, in id order.
        fresh.ids = with_headroom(live_entries);
        fresh.flags = with_headroom(live_entries);
        fresh.places = with_headroom(live_entries);
        fresh.next = with_headroom(live_entries);
        for slice in slices(entries) {
            let names = shared.read().unwrap_or_else(|e| e.into_inner());
            if !current(&names) {
                return None;
            }
            for entry in slice {
                let flags = names.flags[entry];
                if flags & FLAG_DELETED != 0 {
                    continue;
                }
                // A live entry now was live when its name was counted.
                let name = new_number[name_of[entry] as usize] as usize;
                let number = fresh.ids.len() as u32;
                fresh.ids.push(names.ids[entry]);
                fresh.flags.push(flags);
                fresh.places.push(names.places[entry]);
                fresh.next.push(fresh.heads[name]);
                fresh.heads[name] = number;
            }
        }
        let dropped = entries - fresh.ids.len();
        Some((fresh, dropped))
    }

    /// The text of name number `name`.
    fn name(&self, name: usize) -> &str {
        let start = if name == 0 {
            0
        } else {
            self.ends[name - 1] as usize
        };
        &self.text[start..self.ends[name] as usize]
    }

    /// The entries of name number `name`, newest first.
    fn entries(&self, name: usize) -> impl Iterator<Item = usize> + '_ {
        let mut entry = self.heads[name];
        std::iter::from_fn(move || {
            (entry != NO_ENTRY).then(|| {
                let current = entry as usize;
                entry = self.next[current];
                current
            })
        })
    }

    /// The final sort in `search::search_with_generation` (score, name
    /// length, name, path depth, path length), with the id as the last tie
    /// break so the selection is deterministic.
    fn compare(&self, left: &Hit, right: &Hit) -> Ordering {
        let (a, b) = (left.index as usize, right.index as usize);
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                // One number per distinct name: equal numbers, equal names.
                if left.name == right.name {
                    return Ordering::Equal;
                }
                let (left, right) = (
                    self.name(left.name as usize),
                    self.name(right.name as usize),
                );
                left.len().cmp(&right.len()).then_with(|| left.cmp(right))
            })
            .then_with(|| self.places[a].cmp(&self.places[b]))
            .then_with(|| self.ids[a].cmp(&self.ids[b]))
    }

    fn top_hits(
        &self,
        query: &NameQuery<'_>,
        wanted: usize,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Vec<Hit> {
        let total = self.ends.len();
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

    /// Scans the names in `range` (name numbers) and expands each admitted
    /// name to its live entries.
    fn scan(
        &self,
        range: std::ops::Range<usize>,
        query: &NameQuery<'_>,
        wanted: usize,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Vec<Hit> {
        let mut hits = Vec::new();
        // The score of the worst hit that `keep_best` kept. `wanted` hits
        // beat it, so an entry with a lower score cannot reach the top.
        let mut floor = i32::MIN;
        for name in range {
            if name % CANCEL_CHECK_INTERVAL == 0 && is_cancelled() {
                return Vec::new();
            }
            // Every admission rule and scorer tier needs every query
            // character in the name, so the mask test is a necessary
            // condition that rejects most names in one instruction.
            if self.masks[name] & query.mask != query.mask {
                continue;
            }
            let text = self.name(name);
            if !query.admits(text) {
                continue;
            }
            // `name_score` per kind (file, directory), computed when the
            // first live entry of that kind comes up.
            let mut scores: [Option<Option<i32>>; 2] = [None, None];
            for entry in self.entries(name) {
                let flags = self.flags[entry];
                if flags & FLAG_DELETED != 0 {
                    continue;
                }
                let directory = flags & FLAG_DIRECTORY != 0;
                let score = *scores[usize::from(directory)]
                    .get_or_insert_with(|| name_score(text, directory, query.tokens, query.full));
                let Some(score) = score else { continue };
                let score = score - i32::from(flags & FLAG_PENALTY_MASK) * PENALTY_STEP;
                if score < floor {
                    continue;
                }
                hits.push(Hit {
                    score,
                    index: entry as u32,
                    name: name as u32,
                    typo: false,
                });
                if hits.len() >= wanted * 8 {
                    self.keep_best(&mut hits, wanted);
                    floor = hits.iter().map(|hit| hit.score).min().unwrap_or(i32::MIN);
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
            let directory = self.flags[hit.index as usize] & FLAG_DIRECTORY != 0;
            is_strong_match(
                self.name(hit.name as usize),
                directory,
                query.tokens,
                query.full,
            )
        })
    }

    /// The rank of a close name (see `CLOSE_TYPO_SCORE`) before the entry's
    /// folder bonus and location penalty (see `close_entry_rank`), or `None`
    /// when the name is not close.
    fn close_name_rank(&self, name: usize, closeness: &mut Closeness) -> Option<i32> {
        let text = self.name(name);
        let distance = closeness.distance(text)?;
        Some(
            closeness.base + (TYPO_FAR - distance) as i32 * CLOSE_EDIT_STEP
                - text.len().min(80) as i32,
        )
    }

    /// `close_name_rank` plus the entry's folder bonus, minus its location
    /// penalty.
    fn close_entry_rank(&self, name_rank: i32, entry: usize) -> i32 {
        let flags = self.flags[entry];
        let directory_bonus = if flags & FLAG_DIRECTORY != 0 { 20 } else { 0 };
        let penalty = i32::from(flags & FLAG_PENALTY_MASK) * PENALTY_STEP;
        name_rank + directory_bonus - penalty
    }

    /// Close typo-tolerant matches (FFF's frizbee matcher, SIMD Smith-Waterman)
    /// over every distinct name, expanded to every live entry except the
    /// `admitted` hits. Best first; each hit's score is its final rank (see
    /// `close_name_rank`).
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
        // A name whose entries are all deleted can match; it expands to no
        // hit.
        let budget = query.typo_budget();
        let resolve = |name: u32, chunks: &mut [*const u8; TYPO_MAX_CHUNKS]| {
            let name = name as usize;
            if (query.mask & !self.masks[name]).count_ones() > budget {
                return None;
            }
            let start = if name == 0 {
                0
            } else {
                self.ends[name - 1] as usize
            };
            let length = self.ends[name] as usize - start;
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
                .match_range_parallel_resolved(self.ends.len(), &resolve, threads);
        let matched = started.elapsed();
        let match_count = matches.len();

        // Frizbee also accepts scattered matches ("chrmoe" in "chip-remove",
        // "crome" in "cargo.toml"). A misspelled name is the name's start
        // with a letter wrong: only those close names are kept.
        let mut admitted = admitted.iter().map(|hit| hit.index).collect::<Vec<_>>();
        admitted.sort_unstable();
        let mut hits = Vec::new();
        for found in matches {
            let name = found.index as usize;
            let Some(name_rank) = self.close_name_rank(name, closeness) else {
                continue;
            };
            for entry in self.entries(name) {
                if self.flags[entry] & FLAG_DELETED != 0
                    || admitted.binary_search(&(entry as u32)).is_ok()
                {
                    continue;
                }
                hits.push(Hit {
                    score: self.close_entry_rank(name_rank, entry),
                    index: entry as u32,
                    name: found.index,
                    typo: true,
                });
                if hits.len() >= wanted * 8 {
                    self.keep_best(&mut hits, wanted);
                }
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

/// A multiplicative hash over eight bytes at a time, for the name table.
fn name_hash(name: &str) -> u64 {
    const MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut hash = name.len() as u64;
    let (words, rest) = name.as_bytes().as_chunks::<8>();
    for &word in words {
        hash = (hash.rotate_left(26) ^ u64::from_le_bytes(word)).wrapping_mul(MULTIPLIER);
    }
    if !rest.is_empty() {
        let mut word = [0u8; 8];
        word[..rest.len()].copy_from_slice(rest);
        hash = (hash.rotate_left(26) ^ u64::from_le_bytes(word)).wrapping_mul(MULTIPLIER);
    }
    (hash ^ (hash >> 29)).wrapping_mul(MULTIPLIER)
}

/// The first slot to probe for a `name_hash` in a table of `mask + 1` slots.
/// It comes from the high bits, which every input bit reaches.
fn slot_of(hash: u64, mask: usize) -> usize {
    (hash >> 32) as usize & mask
}

/// Asks the CPU to load the cache line of `item` while other work runs.
#[inline(always)]
fn prefetch<T>(item: &T) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: a prefetch is a hint: it reads no value, cannot fault, and
    // `item` is a valid reference.
    unsafe {
        use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
        _mm_prefetch::<_MM_HINT_T0>((item as *const T).cast());
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = item;
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
    use crate::catalog::types::ScannedItem;

    const VOLUME: &str = "vol-test";

    /// A catalog in a folder of its own (under the temp folder) and an index.
    struct Fixture {
        dir: std::path::PathBuf,
        db: Database,
        index: NameIndex,
    }

    impl Fixture {
        fn new() -> Self {
            static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "prism-name-index-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("create the test folder");
            let db = Database::open(&dir.join("catalog.db")).expect("open the test catalog");
            Self { dir, db, index: NameIndex::default() }
        }

        fn add(&self, items: &[ScannedItem]) {
            self.db.insert_batch(VOLUME, 0, items).expect("add rows");
        }

        fn remove(&self, items: &[ScannedItem]) {
            let keys = items
                .iter()
                .map(|item| (item.normalized_path.clone(), item.is_directory))
                .collect::<Vec<_>>();
            let removed = self.db.remove_rows(VOLUME, &keys).expect("remove rows");
            assert_eq!(removed, items.len());
        }

        /// The ids of the rows of `items` in the catalog (before a removal).
        fn ids_of(&self, items: &[ScannedItem]) -> Vec<u32> {
            let rows = self.db.file_names_after(0, usize::MAX).expect("read rows");
            items
                .iter()
                .map(|item| {
                    let row = rows.iter().find(|row| {
                        row.lower_name == item.lower_name && row.path_len == item.display_path.len()
                    });
                    row.expect("the row is in the catalog").id as u32
                })
                .collect()
        }

        fn names(&self) -> std::sync::RwLockReadGuard<'_, Names> {
            self.index.names.read().unwrap()
        }

        /// The (id, path) pairs a query finds.
        fn find(&self, text: &str) -> Vec<(i64, String)> {
            let tokens = text.split_whitespace().collect::<Vec<_>>();
            let query = NameQuery::new(text, &tokens);
            let found = self
                .index
                .candidates(&self.db, &query, 500, false, &|| false)
                .expect("search")
                .expect("the index is loaded");
            found.entries.into_iter().map(|entry| (entry.id, entry.display_path)).collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn item(folder: &str, name: &str) -> ScannedItem {
        ScannedItem {
            normalized_path: format!("c:\\{folder}\\{}", name.to_lowercase()),
            display_path: format!("C:\\{folder}\\{name}"),
            name: name.to_string(),
            lower_name: name.to_lowercase(),
            parent: format!("C:\\{folder}"),
            is_directory: false,
            extension: None,
            modified_at: 1,
            size: 1,
        }
    }

    /// `count` files in `folder`; the names repeat across folders.
    fn files(folder: &str, count: usize) -> Vec<ScannedItem> {
        (0..count).map(|n| item(folder, &format!("Report-{n:05}.txt"))).collect()
    }

    #[test]
    fn deletions_by_id_mark_the_index_without_a_reload() {
        let fixture = Fixture::new();
        let rows = files("docs", 200);
        fixture.add(&rows);
        fixture.index.load(&fixture.db).expect("load");
        assert!(!fixture.index.needs_sync_with(&fixture.db));

        let gone = &rows[10..20];
        let gone_ids = fixture.ids_of(gone);
        fixture.remove(gone);
        // The catalog reported the ids: nothing counts toward a reload.
        assert_eq!(fixture.db.rows_deleted(), 0);
        assert_eq!(fixture.db.pending_deleted_ids(), 10);
        assert!(!fixture.index.needs_sync_with(&fixture.db));

        assert!(fixture.index.catch_up(&fixture.db, CatchUpLimits::SEARCH).unwrap());
        {
            let names = fixture.names();
            assert_eq!(names.deleted, 10);
            for id in &gone_ids {
                let entry = names.ids.binary_search(id).expect("the row stays as a tombstone");
                assert_ne!(names.flags[entry] & FLAG_DELETED, 0);
            }
        }
        assert!(!fixture.index.reload_requested.load(AtomicOrdering::Acquire));
        assert!(!fixture.index.needs_sync_with(&fixture.db));
        let found = fixture.find("report");
        assert_eq!(found.len(), 190);
        assert!(found.iter().all(|(id, _)| !gone_ids.contains(&(*id as u32))));
    }

    #[test]
    fn a_rename_reports_the_deleted_old_entry() {
        let fixture = Fixture::new();
        let rows = files("docs", 20);
        fixture.add(&rows);
        fixture.index.load(&fixture.db).expect("load");
        let old_id = fixture.ids_of(&rows[3..4])[0];
        fixture
            .db
            .rename_file(VOLUME, &rows[3].normalized_path, &item("docs", "Renamed.txt"))
            .expect("rename");
        assert_eq!(fixture.db.rows_deleted(), 0);
        assert!(fixture.index.catch_up(&fixture.db, CatchUpLimits::SEARCH).unwrap());
        let names = fixture.names();
        let entry = names.ids.binary_search(&old_id).unwrap();
        assert_ne!(names.flags[entry] & FLAG_DELETED, 0);
        // The new row came in as an append.
        assert_eq!(names.ids.len(), 21);
    }

    #[test]
    fn churn_of_adds_then_deletes_needs_no_sync() {
        let fixture = Fixture::new();
        fixture.add(&files("docs", 100));
        fixture.index.load(&fixture.db).expect("load");
        let churn = files("build", 500);
        fixture.add(&churn);
        assert!(fixture.index.catch_up(&fixture.db, CatchUpLimits::SEARCH).unwrap());
        fixture.remove(&churn);
        assert!(fixture.index.catch_up(&fixture.db, CatchUpLimits::SEARCH).unwrap());
        assert!(!fixture.index.needs_sync_with(&fixture.db));
        assert_eq!(fixture.db.rows_deleted(), 0);
        assert_eq!(fixture.names().deleted, 500);
    }

    #[test]
    fn heavy_churn_compacts_in_memory_and_never_reloads() {
        let fixture = Fixture::new();
        fixture.add(&files("docs", 100));
        fixture.index.load(&fixture.db).expect("load");
        let churn = files("build", 12_000);
        fixture.add(&churn);
        fixture.index.load(&fixture.db).expect("catch up");
        assert_eq!(fixture.names().ids.len(), 12_100);
        fixture.remove(&churn);
        assert_eq!(fixture.db.rows_deleted(), 0);
        // More ids than a search applies inline: the loader takes them.
        assert!(fixture.index.needs_sync_with(&fixture.db));
        let generation = fixture.names().generation;

        // The loader marks the ids, then compacts.
        let reloaded = fixture.index.load(&fixture.db).expect("sync");
        assert!(!reloaded, "a catch-up is not a full load");
        assert!(!fixture.index.needs_sync_with(&fixture.db));
        {
            let names = fixture.names();
            assert_eq!(names.ids.len(), 100);
            assert_eq!(names.deleted, 0);
            assert_eq!(names.generation, generation.wrapping_add(1));
        }
        assert_eq!(fixture.db.rows_deleted(), 0);
        assert_eq!(fixture.find("report-00007").len(), 1);
    }

    #[test]
    fn compaction_drops_tombstones_and_keeps_the_results() {
        let fixture = Fixture::new();
        // Names repeat across folders, so the name chains have several rows.
        let mut rows = files("a", 300);
        rows.extend(files("b", 300));
        rows.extend((0..100).map(|n| item("c", &format!("unique-{n}.md"))));
        fixture.add(&rows);
        fixture.index.load(&fixture.db).expect("load");
        let mut gone = rows.iter().step_by(3).take(100).cloned().collect::<Vec<_>>();
        gone.extend(rows[300..310].iter().cloned());
        gone.extend(rows[500..520].iter().cloned());
        gone.sort_by(|a, b| a.normalized_path.cmp(&b.normalized_path));
        gone.dedup_by(|a, b| a.normalized_path == b.normalized_path);
        fixture.remove(&gone);
        assert!(fixture.index.catch_up(&fixture.db, CatchUpLimits::ALL).unwrap());
        assert_eq!(fixture.names().deleted, gone.len());

        let queries = ["report", "report-0001", "unique", "uniq 5", "txt", "rpt", ".md", "zzz"];
        let before = queries.map(|query| fixture.find(query));
        let (entries, names_before, max_id) = {
            let names = fixture.names();
            (names.ids.len(), names.ends.len(), names.max_id)
        };

        let compaction = fixture.index.compact(&fixture.db).unwrap().expect("compacted");
        assert_eq!(compaction.dropped, gone.len());
        assert_eq!(compaction.kept, entries - gone.len());
        // The swap holds the write lock for a moment only.
        assert!(compaction.locked < std::time::Duration::from_millis(50));

        let after = queries.map(|query| fixture.find(query));
        assert_eq!(before, after);
        {
            let names = fixture.names();
            assert_eq!(names.deleted, 0);
            assert_eq!(names.max_id, max_id);
            assert!(names.ids.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(names.ends.len() <= names_before);
            // Every entry sits in exactly one name chain.
            let chained = (0..names.ends.len()).map(|name| names.entries(name).count()).sum::<usize>();
            assert_eq!(chained, names.ids.len());
            assert!(names.flags.iter().all(|flags| flags & FLAG_DELETED == 0));
        }

        // A row that comes after the compaction is appended.
        fixture.add(&[item("d", "after-compaction.txt")]);
        assert_eq!(fixture.find("after-compaction").len(), 1);
        // Nothing to compact now.
        assert!(fixture.index.compact(&fixture.db).unwrap().is_none());
    }

    #[test]
    fn a_reload_in_progress_applies_the_deleted_ids_queued_meanwhile() {
        let fixture = Fixture::new();
        let rows = files("docs", 300);
        fixture.add(&rows);
        fixture.index.load(&fixture.db).expect("load");
        let generation = fixture.names().generation;

        // The reload reads every row: they are live in the new buffers.
        *fixture.index.reload_log.lock().unwrap() = Some(ChangeLog::default());
        let fresh = NameIndex::build_beside(&fixture.db, generation).expect("build");
        assert_eq!(fresh.deleted, 0);

        // Rows go away while the new buffers wait for the swap. A live
        // catch-up takes their ids and marks the live index.
        let gone = &rows[40..60];
        let gone_ids = fixture.ids_of(gone);
        fixture.remove(gone);
        assert!(fixture.index.catch_up(&fixture.db, CatchUpLimits::SEARCH).unwrap());
        assert_eq!(fixture.db.pending_deleted_ids(), 0);
        assert_eq!(fixture.names().deleted, 20);

        assert!(fixture.index.swap_in(&fixture.db, fresh, generation).is_some());
        {
            let names = fixture.names();
            assert_eq!(names.generation, generation.wrapping_add(1));
            assert_eq!(names.deleted, 20);
            for id in &gone_ids {
                let entry = names.ids.binary_search(id).unwrap();
                assert_ne!(names.flags[entry] & FLAG_DELETED, 0);
            }
        }
        assert!(fixture.index.reload_log.lock().unwrap().is_none());
        assert_eq!(fixture.find("report").len(), 280);
    }

    #[test]
    fn a_compaction_in_progress_keeps_the_deletes_that_came_meanwhile() {
        let fixture = Fixture::new();
        let rows = files("docs", 300);
        fixture.add(&rows);
        fixture.index.load(&fixture.db).expect("load");
        fixture.remove(&rows[..30]);
        assert!(fixture.index.catch_up(&fixture.db, CatchUpLimits::SEARCH).unwrap());

        // The build copies the entries; rows go away before the swap.
        let generation = fixture.names().generation;
        *fixture.index.reload_log.lock().unwrap() = Some(ChangeLog::default());
        let (fresh, dropped) = Names::compacted(&fixture.index.names).expect("built");
        assert_eq!(dropped, 30);
        let later = &rows[100..105];
        let later_ids = fixture.ids_of(later);
        fixture.remove(later);
        assert!(fixture.index.catch_up(&fixture.db, CatchUpLimits::SEARCH).unwrap());
        // A search that found a row missing logs it too.
        let missing = fixture.ids_of(&rows[200..201])[0];
        let entry = fixture.names().ids.binary_search(&missing).unwrap();
        fixture.index.mark_deleted(generation, &[entry as u32]);

        assert!(fixture.index.swap_in(&fixture.db, fresh, generation).is_some());
        let names = fixture.names();
        assert_eq!(names.ids.len(), 270);
        assert_eq!(names.deleted, 6);
        for id in later_ids.iter().chain(&[missing]) {
            let entry = names.ids.binary_search(id).unwrap();
            assert_ne!(names.flags[entry] & FLAG_DELETED, 0);
        }
    }

    #[test]
    fn a_reset_makes_the_new_buffers_obsolete() {
        let fixture = Fixture::new();
        fixture.add(&files("docs", 50));
        fixture.index.load(&fixture.db).expect("load");
        fixture.remove(&files("docs", 5));
        assert!(fixture.index.catch_up(&fixture.db, CatchUpLimits::ALL).unwrap());
        *fixture.index.reload_log.lock().unwrap() = Some(ChangeLog::default());
        let generation = fixture.names().generation;
        let (fresh, _) = Names::compacted(&fixture.index.names).expect("built");
        fixture.index.reset();
        assert!(fixture.index.swap_in(&fixture.db, fresh, generation).is_none());
        assert!(fixture.names().ids.is_empty());
    }

    #[test]
    fn a_reload_of_a_loaded_index_keeps_the_results() {
        let fixture = Fixture::new();
        let rows = files("docs", 100);
        fixture.add(&rows);
        fixture.index.load(&fixture.db).expect("load");
        fixture.remove(&rows[..10]);
        let before = fixture.find("report");
        fixture.index.request_reload();
        assert!(fixture.index.load(&fixture.db).expect("reload"));
        assert_eq!(before, fixture.find("report"));
        assert_eq!(fixture.names().ids.len(), 90);
    }

    #[test]
    fn a_deletion_that_comes_before_its_row_waits_for_the_row() {
        let fixture = Fixture::new();
        fixture.add(&files("docs", 50));
        fixture.index.load(&fixture.db).expect("load");
        let max_id = fixture.names().max_id;

        // A catch-up reads the new rows; before it appends them, a search's
        // catch-up takes their deleted ids.
        let late = files("late", 20);
        fixture.add(&late);
        let read = fixture.db.file_names_after(max_id, usize::MAX).expect("read rows");
        assert_eq!(read.len(), 20);
        fixture.remove(&late);
        assert!(NameIndex::refresh_deleted(&fixture.index.names, None, &fixture.db, usize::MAX));
        assert_eq!(fixture.names().deleted_ahead.len(), 20);
        assert_eq!(fixture.names().deleted, 0);

        fixture.index.names.write().unwrap().push_rows(&read).expect("append");
        let names = fixture.names();
        assert_eq!(names.deleted, 20, "the appended rows are marked deleted");
        assert!(names.deleted_ahead.is_empty());
    }

    #[test]
    fn deleted_ids_enter_an_active_change_log() {
        let fixture = Fixture::new();
        let rows = files("docs", 30);
        fixture.add(&rows);
        fixture.index.load(&fixture.db).expect("load");
        let gone_ids = fixture.ids_of(&rows[..5]);
        fixture.remove(&rows[..5]);
        // A reload or a compaction is in progress.
        *fixture.index.reload_log.lock().unwrap() = Some(ChangeLog::default());
        assert!(NameIndex::refresh_deleted(
            &fixture.index.names,
            Some(&fixture.index.reload_log),
            &fixture.db,
            usize::MAX
        ));
        let logged = fixture.index.reload_log.lock().unwrap().take().unwrap();
        let mut logged_ids: Vec<u32> = logged.deleted.iter().map(|&id| id as u32).collect();
        logged_ids.sort_unstable();
        let mut expected = gone_ids;
        expected.sort_unstable();
        assert_eq!(logged_ids, expected);
        assert_eq!(fixture.names().deleted, 5);
    }
}
