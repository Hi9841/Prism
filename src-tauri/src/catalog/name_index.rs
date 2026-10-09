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
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, RwLock};

use super::db::Database;
use super::search::{name_score, path_depth, path_penalty};
use super::types::{CandidateEntry, NameRow, RowLookup};

/// Rows per SQLite read while loading. One batch holds the reader connection
/// for tens of milliseconds, so searches on the SQL path keep running.
const LOAD_BATCH: usize = 100_000;
/// New rows a search appends inline before it answers. A larger backlog (a
/// full rescan) goes to the background loader and the search uses SQL.
const SEARCH_CATCH_UP: usize = 10_000;
const MAX_SCAN_THREADS: usize = 8;
const MIN_ROWS_PER_THREAD: usize = 100_000;
const CANCEL_CHECK_INTERVAL: usize = 16_384;
/// Rows selected for the by-id fetch per result slot. The surplus covers rows
/// that the final disk-existence check rejects.
const CANDIDATES_PER_RESULT: usize = 4;
const MIN_CANDIDATES: usize = 64;
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

pub struct NameIndex {
    names: RwLock<Names>,
    loaded: AtomicBool,
    loader_running: AtomicBool,
    reload_requested: AtomicBool,
    /// A search found more new rows than it may append inline.
    behind: AtomicBool,
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
}

/// A query parsed once per search: the scorer inputs plus the admission rules
/// of the SQL candidate stages that this index replaces.
pub(crate) struct NameQuery<'a> {
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
}

impl Default for NameIndex {
    fn default() -> Self {
        Self {
            names: RwLock::new(Names::default()),
            loaded: AtomicBool::new(false),
            loader_running: AtomicBool::new(false),
            reload_requested: AtomicBool::new(false),
            behind: AtomicBool::new(false),
        }
    }
}

impl<'a> NameQuery<'a> {
    pub(crate) fn new(full: &'a str, tokens: &'a [&'a str]) -> Self {
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
                if let Err(error) = result {
                    eprintln!("[Prism Catalog] name index load failed: {error}");
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
        *self.names.write().unwrap_or_else(|e| e.into_inner()) = Names::default();
    }

    /// True when the background loader has work: the first load, a backlog
    /// that a search deferred, or enough deleted rows that a fresh load is
    /// cheaper than carrying them.
    pub fn needs_sync(&self) -> bool {
        !self.is_loaded()
            || self.behind.load(AtomicOrdering::Acquire)
            || self.reload_requested.load(AtomicOrdering::Acquire)
    }

    /// Synchronous full load (or catch-up), batch by batch.
    pub(crate) fn load(&self, db: &Database) -> Result<(), String> {
        if self.reload_requested.swap(false, AtomicOrdering::AcqRel) {
            self.reset();
        }
        while !self.catch_up(db, LOAD_BATCH)? {}
        self.behind.store(false, AtomicOrdering::Release);
        self.loaded.store(true, AtomicOrdering::Release);
        Ok(())
    }

    /// Appends up to `max_rows` rows above `max_id`. Returns true when the
    /// index has caught up with the catalog.
    fn catch_up(&self, db: &Database, max_rows: usize) -> Result<bool, String> {
        let after = self.names.read().unwrap_or_else(|e| e.into_inner()).max_id;
        let rows = db.file_names_after(after, max_rows)?;
        let caught_up = rows.len() < max_rows;
        if !rows.is_empty() {
            let mut names = self.names.write().unwrap_or_else(|e| e.into_inner());
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

    /// Fallback-catalog candidates for a query, best first. `None` means the
    /// index cannot answer now (not loaded, or a large backlog of new rows is
    /// pending) and the caller must use the SQL candidate path.
    pub(crate) fn candidates(
        &self,
        db: &Database,
        query: &NameQuery<'_>,
        limit: usize,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<Vec<CandidateEntry>>, String> {
        if !self.is_loaded() {
            return Ok(None);
        }
        if !self.catch_up(db, SEARCH_CATCH_UP)? {
            self.behind.store(true, AtomicOrdering::Release);
            return Ok(None);
        }
        let wanted = (limit * CANDIDATES_PER_RESULT).max(MIN_CANDIDATES);

        let mut found = Vec::with_capacity(wanted);
        for _ in 0..MAX_SELECTION_ROUNDS {
            let selected = {
                let names = self.names.read().unwrap_or_else(|e| e.into_inner());
                names
                    .top_hits(query, wanted, is_cancelled)
                    .into_iter()
                    .map(|hit| (hit.index, i64::from(names.ids[hit.index as usize])))
                    .collect::<Vec<_>>()
            };
            if is_cancelled() {
                return Ok(Some(Vec::new()));
            }
            let ids = selected.iter().map(|&(_, id)| id).collect::<Vec<_>>();
            let lookups = db.file_rows_by_id(&ids)?;
            let mut missing = Vec::new();
            for ((index, _), lookup) in selected.into_iter().zip(lookups) {
                match lookup {
                    RowLookup::Found(entry) => found.push(entry),
                    RowLookup::Missing => missing.push(index),
                    RowLookup::Excluded => {}
                }
            }
            if missing.is_empty() {
                break;
            }
            // Deleted rows held slots in this round; drop them and select
            // again so live rows further down are not cut off.
            self.mark_deleted(&missing);
            found.clear();
        }
        Ok(Some(found))
    }

    fn mark_deleted(&self, indexes: &[u32]) {
        let mut names = self.names.write().unwrap_or_else(|e| e.into_inner());
        for &index in indexes {
            let flags = &mut names.flags[index as usize];
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
            .candidates(db, &NameQuery::new(text, &tokens), limit, &|| false)
            .unwrap()
            .expect("index answers once loaded")
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
    fn large_backlog_defers_to_the_sql_path() {
        let (db, directory) = temp_db();
        let index = NameIndex::default();
        index.load(&db).unwrap();
        let rows = (0..SEARCH_CATCH_UP + 1)
            .map(|n| item(&format!(r"C:\bulk\file{n}.txt")))
            .collect::<Vec<_>>();
        db.insert_batch("vol", 1, &rows).unwrap();

        let tokens = ["file1"];
        let parsed = NameQuery::new("file1", &tokens);
        let first = index.candidates(&db, &parsed, 10, &|| false).unwrap();
        assert!(
            first.is_none(),
            "a backlog this large must not block a keystroke"
        );
        assert!(index.needs_sync());
        index.load(&db).unwrap();
        assert!(!index.needs_sync());
        let second = index.candidates(&db, &parsed, 10, &|| false).unwrap();
        assert!(second.is_some_and(|found| !found.is_empty()));
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
