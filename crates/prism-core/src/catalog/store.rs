//! In-memory file store: the truth of the file index while Prism runs.
//!
//! One `Segment` per volume. A segment holds the entries of one volume in
//! parallel arrays (parent, name, flags, sequence number, name chain)
//! and the names in two pools: each distinct lowercase name once (the text the
//! search scans), and each distinct exact-case name that differs from its
//! lowercase form once (a case variant). A name equal to its lowercase form
//! (most names) has no second copy: the entry points to the lowercase name.
//! No path text is stored: a result's path comes from the parent chain.
//!
//! The ranking code (`NameQuery`, `top_hits`, `compare`, the typo pass) is
//! the code of the old `NameIndex` (SQLite catalog, removed) and ranks the
//! same names in the same order.
//!
//! Rules:
//! - An entry is reachable only through live ancestors. A removed folder is
//!   marked dead; its descendants stay in the arrays but search and path
//!   building drop them (dead by ancestry). Compaction frees them.
//! - A folder rename or move changes one entry; its children keep their
//!   parent index. The children's penalty stays as it was until compaction
//!   recomputes it. Their places (depth and path length, a tie break) come
//!   from the parent chain when a search needs them, so they are always
//!   current.
//! - An excluded folder is stored (the journal reader needs to find it) but
//!   never shown and never gets children.
//! - A segment is replaced as a whole by `replace_volume`: the new arrays are
//!   built beside the live ones and swapped in. Changes that `apply` made to
//!   the old entries during the build are not carried over: the caller replays
//!   its journal from before the build.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use neo_frizbee::{Config, Matcher, Pattern, PatternConfig, SortStrategy, SIMD_CHUNK_BYTES};

use super::search::{is_strong_match, name_score, path_depth, path_penalty};
use super::types::CandidateEntry;

/// Entries (or names) that `compact` copies under one read lock. Between two
/// slices the lock is free, so a writer that waits never holds the searches
/// back for long.
const COMPACT_SLICE: usize = 1 << 18;
const MAX_SCAN_THREADS: usize = 8;
const MIN_ROWS_PER_THREAD: usize = 100_000;
const CANCEL_CHECK_INTERVAL: usize = 16_384;
/// Entries selected per result slot. The surplus covers entries that the
/// caller rejects after a disk check.
const CANDIDATES_PER_RESULT: usize = 15;
const MIN_CANDIDATES: usize = 150;
/// Tombstones that start a compaction: 1/`COMPACT_SHARE` of the entries, at
/// least `MIN_COMPACT_TOMBSTONES`. A dead entry costs memory and scan time, so
/// the store compacts when 1/8 of the entries are dead.
const COMPACT_SHARE: usize = 8;
const MIN_COMPACT_TOMBSTONES: usize = 10_000;
/// The SQL subsequence stage probed only the first four tokens.
const PROBE_TOKENS: usize = 4;

/// Every location penalty in `search::path_penalty` is a multiple of this, so
/// the penalty fits in the low byte of the entry flags.
const PENALTY_STEP: i32 = 50;
const FLAG_DIRECTORY: u16 = 1 << 15;
const FLAG_DEAD: u16 = 1 << 14;
const FLAG_EXCLUDED: u16 = 1 << 13;
/// The entry of a volume root: always entry 0 of its segment.
const FLAG_ROOT: u16 = 1 << 12;
const FLAG_PENALTY_MASK: u16 = 0xff;
/// A hit's places are not computed yet. No real value is this large: a chain
/// has at most `MAX_DEPTH` links.
const NO_PLACES: u32 = u32::MAX;
/// "No entry" and "no name".
const NONE: u32 = u32::MAX;
/// The high bit of `Segment::display`. Set: the low 31 bits number a case
/// variant. Clear: the value is the lowercase name number, and the exact name
/// equals the lowercase name. `NONE` (the root entry) has the bit set: test
/// for `NONE` first.
const VARIANT_TAG: u32 = 1 << 31;
const ROOT_ENTRY: u32 = 0;
/// Record numbers from this value on are not indexed: `by_record` holds one
/// slot per record number, so a stray huge number must not grow it.
const MAX_RECORD: u64 = 1 << 27;
/// Longest parent chain a walk follows. A longer chain is a cycle.
const MAX_DEPTH: usize = 4096;
/// The name tables grow when more than this share of their slots is in use.
const TABLE_LOAD_NUMERATOR: usize = 5;
const TABLE_LOAD_DENOMINATOR: usize = 8;
const MIN_TABLE_SLOTS: usize = 1 << 10;
/// Headroom of each array, as a share of its length.
const GROWTH_SHARE: usize = 32;
const MIN_GROWTH: usize = 1024;

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

pub type VolumeSlot = u16;

/// Low 48 bits of an FRN: the MFT record number.
pub const RECORD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Splits an FRN into (record number, sequence number).
pub fn split_frn(frn: u64) -> (u64, u16) {
    (frn & RECORD_MASK, (frn >> 48) as u16)
}

/// Joins a record number and a sequence number into an FRN.
pub fn make_frn(record: u64, seq: u16) -> u64 {
    (record & RECORD_MASK) | ((seq as u64) << 48)
}

/// One file or folder as a source reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsNode {
    /// MFT record number (FRN low 48 bits), or a number that a walk gave.
    pub record: u64,
    /// NTFS sequence number (FRN high 16 bits). It tells a reused record.
    pub seq: u16,
    pub parent_record: u64,
    /// Exact-case name.
    pub name: String,
    pub is_dir: bool,
    /// An excluded folder is stored but never shown and never descended.
    pub excluded: bool,
    /// A junction, symlink or mount point folder that is not a cloud
    /// placeholder. It is a leaf: a walk does not enter it. Not stored.
    pub link: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsChange {
    Upsert(FsNode),
    Remove { record: u64, seq: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeBackend {
    Journal,
    Walk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeMeta {
    pub volume_id: String,
    /// Mount path with a trailing backslash, for example `C:\`.
    pub mount_path: String,
    pub root_record: u64,
    pub backend: VolumeBackend,
    pub journal_id: u64,
    pub next_usn: i64,
    /// Fingerprint of the exclusion list that built the entries. A changed
    /// list needs a new walk.
    pub exclusions: u64,
}

/// What `FileStore::compact` did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compaction {
    /// Entries the new arrays do not hold (tombstones and entries below a
    /// dead or excluded folder).
    pub dropped: usize,
    /// Entries of the new arrays.
    pub kept: usize,
    /// Time to build the new arrays (searches run meanwhile).
    pub build: Duration,
    /// Time the swap held the write lock.
    pub locked: Duration,
}

/// What the store found for a query.
#[derive(Default)]
pub struct IndexCandidates {
    /// Names the admission rules accept, for the caller's scorer.
    pub entries: Vec<CandidateEntry>,
    /// Typo-tolerant fuzzy matches the rules reject, with their final rank
    /// (below every strong match).
    pub typos: Vec<(CandidateEntry, i32)>,
}

/// A query parsed once per search: the scorer inputs plus the admission rules
/// of the candidate stages.
pub struct NameQuery<'a> {
    full: &'a str,
    tokens: &'a [&'a str],
    mask: u32,
    /// The substring and subsequence stages ran only for 3+ characters.
    long: bool,
    /// Substring stage: every 3+ character token is a substring.
    substring_tokens: Vec<&'a str>,
    /// Subsequence stage: the name starts with a token's first character and
    /// holds the rest of that token in order (`LIKE 'r%p%t%'`).
    probe_tokens: &'a [&'a str],
}

#[derive(Clone, Copy)]
struct Hit {
    score: i32,
    /// Entry number in its segment.
    index: u32,
    /// Lowercase name number in its segment.
    name: u32,
    /// Slot of the segment.
    seg: VolumeSlot,
    /// Found by the typo pass: `score` is its final rank.
    typo: bool,
    /// The entry's places once a tie needed them, else `NO_PLACES`.
    places: u32,
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

    /// True when one of the stages (exact, prefix, substring, or subsequence
    /// probe) accepts this name.
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

/// Each distinct lowercase name once, back to back, with the head of its
/// entry chain.
#[derive(Default)]
pub(super) struct Names {
    pub(super) text: String,
    /// End offset of each distinct name in `text`.
    pub(super) ends: Vec<u32>,
    /// Characters present in each distinct name (see `char_mask`).
    pub(super) masks: Vec<u32>,
    /// Newest entry of each distinct name. The older entries with that name
    /// follow `Segment::next`.
    pub(super) heads: Vec<u32>,
    /// Open-addressing hash table over the distinct names: name number + 1,
    /// or 0 for a free slot. Its length is a power of two.
    table: Vec<u32>,
}

impl Names {
    pub(super) fn len(&self) -> usize {
        self.ends.len()
    }

    /// The text of name number `name`.
    fn len_of(&self, name: usize) -> usize {
        let start = if name == 0 { 0 } else { self.ends[name - 1] as usize };
        self.ends[name] as usize - start
    }

    pub(super) fn name(&self, name: usize) -> &str {
        let start = if name == 0 {
            0
        } else {
            self.ends[name - 1] as usize
        };
        &self.text[start..self.ends[name] as usize]
    }

    /// The name number of `name` (whose `name_hash` is `hash`), added when it
    /// is new. `None` when the text would pass 4 GiB.
    fn intern(&mut self, name: &str, hash: u64) -> Option<u32> {
        if (self.ends.len() + 1) * TABLE_LOAD_DENOMINATOR > self.table.len() * TABLE_LOAD_NUMERATOR
        {
            self.place_names((self.table.len() * 2).max(MIN_TABLE_SLOTS));
        }
        let mask = self.table.len() - 1;
        let mut slot = slot_of(hash, mask);
        loop {
            match self.table[slot] {
                0 => break,
                stored if self.name(stored as usize - 1) == name => return Some(stored - 1),
                _ => slot = (slot + 1) & mask,
            }
        }
        let end = u32::try_from(self.text.len() + name.len()).ok()?;
        let number = u32::try_from(self.ends.len()).ok().filter(|&n| n < VARIANT_TAG)?;
        self.text.push_str(name);
        self.ends.push(end);
        self.masks.push(char_mask(name));
        self.heads.push(NONE);
        self.table[slot] = number + 1;
        Some(number)
    }

    /// Fills a new hash table of `slots` slots (a power of two). The text
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

    /// Builds the hash table again (after a load).
    pub(super) fn rebuild_table(&mut self) {
        let mut slots = MIN_TABLE_SLOTS;
        while (self.ends.len() + 1) * TABLE_LOAD_DENOMINATOR > slots * TABLE_LOAD_NUMERATOR {
            slots *= 2;
        }
        self.place_names(slots);
    }

    fn shrink(&mut self) {
        self.text.shrink_to(self.text.len() + self.text.len() / GROWTH_SHARE);
        fit(&mut self.ends);
        fit(&mut self.masks);
        fit(&mut self.heads);
    }
}

/// The case variants: each distinct exact-case name that differs from its
/// lowercase form, once. A name equal to its lowercase form is not here: its
/// entry points to the lowercase name (see `VARIANT_TAG`).
#[derive(Default)]
pub(super) struct Variants {
    /// The variant texts, back to back.
    pub(super) text: String,
    /// End offset of each variant in `text`.
    pub(super) ends: Vec<u32>,
    /// The lowercase name number of each variant.
    pub(super) lower: Vec<u32>,
    /// Hash table over the exact text: variant number + 1, or 0 for a free
    /// slot. Its length is a power of two.
    table: Vec<u32>,
}

impl Variants {
    /// The text of variant `variant`.
    fn text_of(&self, variant: usize) -> &str {
        let start = if variant == 0 {
            0
        } else {
            self.ends[variant - 1] as usize
        };
        &self.text[start..self.ends[variant] as usize]
    }

    /// The variant number of `name` (whose `name_hash` is `hash`; its
    /// lowercase form is name number `lower`), added when it is new. `None`
    /// when the text would pass 4 GiB.
    fn intern(&mut self, name: &str, lower: u32, hash: u64) -> Option<u32> {
        if (self.ends.len() + 1) * TABLE_LOAD_DENOMINATOR > self.table.len() * TABLE_LOAD_NUMERATOR
        {
            self.place_variants((self.table.len() * 2).max(MIN_TABLE_SLOTS));
        }
        let mask = self.table.len() - 1;
        let mut slot = slot_of(hash, mask);
        loop {
            match self.table[slot] {
                0 => break,
                stored if self.text_of(stored as usize - 1) == name => return Some(stored - 1),
                _ => slot = (slot + 1) & mask,
            }
        }
        let number = u32::try_from(self.ends.len()).ok().filter(|&n| n < VARIANT_TAG)?;
        let end = u32::try_from(self.text.len() + name.len()).ok()?;
        self.text.push_str(name);
        self.ends.push(end);
        self.lower.push(lower);
        self.table[slot] = number + 1;
        Some(number)
    }

    /// Fills a new hash table of `slots` slots (a power of two). The text
    /// holds the names, so the hashes are computed again instead of stored.
    fn place_variants(&mut self, slots: usize) {
        let mut table = vec![0u32; slots];
        let mask = slots - 1;
        for number in 0..self.ends.len() {
            let mut slot = slot_of(name_hash(self.text_of(number)), mask);
            while table[slot] != 0 {
                slot = (slot + 1) & mask;
            }
            table[slot] = number as u32 + 1;
        }
        self.table = table;
    }

    /// Builds the hash table again (after a load).
    pub(super) fn rebuild_table(&mut self) {
        let mut slots = MIN_TABLE_SLOTS;
        while (self.ends.len() + 1) * TABLE_LOAD_DENOMINATOR > slots * TABLE_LOAD_NUMERATOR {
            slots *= 2;
        }
        self.place_variants(slots);
    }

    /// Converts the display names of snapshot versions 2 and 3 (one per
    /// exact-case name, an empty range for a name equal to its lowercase form)
    /// to case variants. Returns the variants and, for each old display
    /// number, the `Segment::display` value that replaces it. `name_count` is
    /// the number of lowercase names.
    pub(super) fn from_legacy(
        text: &str,
        ends: &[u32],
        lower: &[u32],
        name_count: usize,
    ) -> Result<(Self, Vec<u32>), String> {
        if lower.len() != ends.len() {
            return Err("the display names differ in length".into());
        }
        if ends.last().map_or(0, |&end| end as usize) != text.len() {
            return Err("the display text has a tail".into());
        }
        // The old text holds only the non-empty names: the arrays get their
        // exact size.
        let starts = std::iter::once(&0).chain(ends);
        let count = ends.iter().zip(starts).filter(|(end, start)| end != start).count();
        let mut variants = Self {
            text: String::with_capacity(text.len()),
            ends: Vec::with_capacity(count),
            lower: Vec::with_capacity(count),
            table: Vec::new(),
        };
        let mut map = Vec::with_capacity(ends.len());
        let mut start = 0usize;
        for (&end, &lower) in ends.iter().zip(lower) {
            let end = end as usize;
            if end < start || end > text.len() || !text.is_char_boundary(end) {
                return Err("a name range is wrong".into());
            }
            if lower as usize >= name_count || lower >= VARIANT_TAG {
                return Err("a display name points outside its array".into());
            }
            if start == end {
                map.push(lower);
            } else {
                map.push(VARIANT_TAG | variants.ends.len() as u32);
                variants.text.push_str(&text[start..end]);
                variants.ends.push(variants.text.len() as u32);
                variants.lower.push(lower);
            }
            start = end;
        }
        Ok((variants, map))
    }

    fn shrink(&mut self) {
        self.text.shrink_to(self.text.len() + self.text.len() / GROWTH_SHARE);
        fit(&mut self.ends);
        fit(&mut self.lower);
    }
}

fn fit<T>(buffer: &mut Vec<T>) {
    buffer.shrink_to(buffer.len() + buffer.len() / GROWTH_SHARE);
}

/// Makes room for one more element with a small headroom instead of doubling.
fn room<T>(buffer: &mut Vec<T>) {
    if buffer.len() == buffer.capacity() {
        buffer.reserve_exact((buffer.len() / GROWTH_SHARE).max(MIN_GROWTH));
    }
}

/// The entries of one volume.
pub(super) struct Segment {
    /// Unique among the segments of a store (a rebuilt segment gets a new
    /// one): a compaction swaps only the segment it started from.
    pub(super) id: u64,
    pub(super) slot: VolumeSlot,
    pub(super) meta: VolumeMeta,
    /// The mount path without its trailing backslash: the path of the root
    /// entry. A child's path is this, a backslash, and its name.
    root_path: String,
    /// The places of the root entry (see `places_of`).
    root_places: u32,
    pub(super) names: Names,
    pub(super) variants: Variants,
    /// Parent entry. `NONE` only for the root entry, and for an entry that
    /// waits for its parent after a compaction (it is hidden meanwhile).
    pub(super) parent: Vec<u32>,
    /// The exact-case name: the lowercase name number when the name equals
    /// its lowercase form, else `VARIANT_TAG` | the case variant number.
    /// `NONE` for the root entry.
    pub(super) display: Vec<u32>,
    /// Directory, dead, excluded and root flags; penalty / `PENALTY_STEP` in
    /// the low byte.
    pub(super) flags: Vec<u16>,
    /// NTFS sequence number of the record.
    pub(super) seq: Vec<u16>,
    /// Next older entry with the same lowercase name, or `NONE`.
    pub(super) next: Vec<u32>,
    /// Entry of each record number, or `NONE`.
    pub(super) by_record: Vec<u32>,
    /// Entries with the dead flag.
    pub(super) deleted: usize,
    /// Entries that searches found below a dead or excluded folder. They
    /// count toward the tombstones.
    hidden_seen: AtomicUsize,
    /// `Some` while a compaction builds new arrays: the changes applied
    /// meanwhile, for the swap to replay.
    log: Option<Vec<FsChange>>,
}

/// The places of a volume's root entry: the depth and the length of its path.
fn root_places_of(root_path: &str) -> u32 {
    let depth = path_depth(root_path).min(0xffff) as u32;
    let length = root_path.len().min(0xffff) as u32;
    depth << 16 | length
}

impl Segment {
    /// An empty segment with its root entry.
    fn new(id: u64, meta: VolumeMeta) -> Self {
        let root_path = meta.mount_path.trim_end_matches('\\').to_string();
        let root_places = root_places_of(&root_path);
        let mut segment = Self {
            id,
            slot: 0,
            root_path,
            root_places,
            names: Names::default(),
            variants: Variants::default(),
            parent: vec![NONE],
            display: vec![NONE],
            flags: vec![FLAG_ROOT | FLAG_DIRECTORY],
            seq: vec![0],
            next: vec![NONE],
            by_record: Vec::new(),
            deleted: 0,
            hidden_seen: AtomicUsize::new(0),
            log: None,
            meta,
        };
        segment.set_record(segment.meta.root_record, ROOT_ENTRY);
        segment
    }

    /// A segment from loaded arrays. Checks every index, so a search never
    /// reads outside an array.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_parts(
        id: u64,
        meta: VolumeMeta,
        mut names: Names,
        mut variants: Variants,
        parent: Vec<u32>,
        display: Vec<u32>,
        flags: Vec<u16>,
        seq: Vec<u16>,
        next: Vec<u32>,
        by_record: Vec<u32>,
    ) -> Result<Self, String> {
        let entries = parent.len();
        if entries == 0
            || [display.len(), flags.len(), seq.len(), next.len()]
                .iter()
                .any(|&length| length != entries)
        {
            return Err("the entry arrays differ in length".into());
        }
        if entries >= NONE as usize {
            return Err("too many entries".into());
        }
        // Names: ranges that end in order, on character boundaries.
        let check_text = |text: &str, ends: &[u32]| -> Result<(), String> {
            let mut previous = 0u32;
            for &end in ends {
                if end < previous || end as usize > text.len() || !text.is_char_boundary(end as usize)
                {
                    return Err("a name range is wrong".into());
                }
                previous = end;
            }
            Ok(())
        };
        check_text(&names.text, &names.ends)?;
        if names.ends.last().map_or(0, |&end| end as usize) != names.text.len() {
            return Err("the name text has a tail".into());
        }
        if names.heads.len() != names.ends.len() {
            return Err("the name heads differ in length".into());
        }
        check_text(&variants.text, &variants.ends)?;
        if variants.ends.last().map_or(0, |&end| end as usize) != variants.text.len() {
            return Err("the variant text has a tail".into());
        }
        if variants.lower.len() != variants.ends.len() {
            return Err("the variants differ in length".into());
        }
        let name_count = names.ends.len();
        let variant_count = variants.ends.len();
        let wrong = |what: &str| Err(format!("{what} points outside its array"));
        if names.heads.iter().any(|&head| head != NONE && head as usize >= entries) {
            return wrong("a name head");
        }
        if variants.lower.iter().any(|&lower| lower as usize >= name_count) {
            return wrong("a case variant");
        }
        if parent.iter().any(|&p| p != NONE && p as usize >= entries) {
            return wrong("a parent");
        }
        // The root entry has no name. Any other entry names a lowercase name
        // or a case variant (`NONE` has the tag and no variant is that high).
        let named = |d: u32| {
            if d & VARIANT_TAG == 0 {
                (d as usize) < name_count
            } else {
                ((d & !VARIANT_TAG) as usize) < variant_count
            }
        };
        if display[0] != NONE || !display.iter().skip(1).all(|&d| named(d)) {
            return wrong("an entry's name");
        }
        if next.iter().any(|&n| n != NONE && n as usize >= entries) {
            return wrong("a chain link");
        }
        if by_record.iter().any(|&e| e != NONE && e as usize >= entries) {
            return wrong("a record");
        }
        if flags[0] & FLAG_ROOT == 0 || flags[1..].iter().any(|&f| f & FLAG_ROOT != 0) {
            return Err("the root entry is wrong".into());
        }
        if meta.root_record < MAX_RECORD
            && by_record.get(meta.root_record as usize) != Some(&ROOT_ENTRY)
        {
            return Err("the root record does not point to the root entry".into());
        }
        names.masks = (0..name_count).map(|n| char_mask(names.name(n))).collect();
        names.rebuild_table();
        variants.rebuild_table();
        let deleted = flags.iter().filter(|&&f| f & FLAG_DEAD != 0).count();
        let root_path = meta.mount_path.trim_end_matches('\\').to_string();
        let root_places = root_places_of(&root_path);
        Ok(Self {
            id,
            slot: 0,
            meta,
            root_path,
            root_places,
            names,
            variants,
            parent,
            display,
            flags,
            seq,
            next,
            by_record,
            deleted,
            hidden_seen: AtomicUsize::new(0),
            log: None,
        })
    }

    pub(super) fn entries(&self) -> usize {
        self.parent.len()
    }

    fn memory_bytes(&self) -> usize {
        fn of<T>(buffer: &Vec<T>) -> usize {
            buffer.capacity() * std::mem::size_of::<T>()
        }
        self.names.text.capacity()
            + of(&self.names.ends)
            + of(&self.names.masks)
            + of(&self.names.heads)
            + of(&self.names.table)
            + self.variants.text.capacity()
            + of(&self.variants.ends)
            + of(&self.variants.lower)
            + of(&self.variants.table)
            + of(&self.parent)
            + of(&self.display)
            + of(&self.flags)
            + of(&self.seq)
            + of(&self.next)
            + of(&self.by_record)
    }

    /// Entries that a search can return (the root and the dead excluded).
    pub(super) fn live(&self) -> u64 {
        (self.parent.len() - 1).saturating_sub(self.deleted) as u64
    }

    /// True when the tombstones are many enough to compact.
    fn has_many_tombstones(&self) -> bool {
        self.deleted + self.hidden_seen.load(AtomicOrdering::Relaxed)
            > (self.parent.len() / COMPACT_SHARE).max(MIN_COMPACT_TOMBSTONES)
    }

    fn shrink(&mut self) {
        self.names.shrink();
        self.variants.shrink();
        fit(&mut self.parent);
        fit(&mut self.display);
        fit(&mut self.flags);
        fit(&mut self.seq);
        fit(&mut self.next);
        fit(&mut self.by_record);
    }

    fn entry_of(&self, record: u64) -> Option<u32> {
        let entry = *self.by_record.get(usize::try_from(record).ok()?)?;
        (entry != NONE).then_some(entry)
    }

    fn set_record(&mut self, record: u64, entry: u32) {
        if record >= MAX_RECORD {
            return;
        }
        let record = record as usize;
        if record >= self.by_record.len() {
            let wanted = record + 1;
            if wanted > self.by_record.capacity() {
                let extra = (wanted - self.by_record.len()).max(self.by_record.len() / GROWTH_SHARE);
                self.by_record.reserve_exact(extra);
            }
            self.by_record.resize(wanted, NONE);
        }
        self.by_record[record] = entry;
    }

    /// The exact-case name of an entry. Not for the root entry.
    fn display_name(&self, entry: usize) -> &str {
        let display = self.display[entry];
        debug_assert!(display != NONE, "the root entry has no name");
        if display & VARIANT_TAG == 0 {
            self.names.name(display as usize)
        } else {
            self.variants.text_of((display & !VARIANT_TAG) as usize)
        }
    }

    /// The byte length of the exact-case name of an entry. Not for the root
    /// entry.
    fn display_len(&self, entry: usize) -> usize {
        let display = self.display[entry];
        debug_assert!(display != NONE, "the root entry has no name");
        if display & VARIANT_TAG == 0 {
            self.names.len_of(display as usize)
        } else {
            self.variants.text_of((display & !VARIANT_TAG) as usize).len()
        }
    }

    /// The lowercase name number of an entry. Not for the root entry.
    fn lower_of(&self, entry: usize) -> u32 {
        self.lower_of_display(self.display[entry])
    }

    /// The lowercase name number that a `display` value stands for.
    fn lower_of_display(&self, display: u32) -> u32 {
        debug_assert!(display != NONE, "the root entry has no name");
        if display & VARIANT_TAG == 0 {
            display
        } else {
            self.variants.lower[(display & !VARIANT_TAG) as usize]
        }
    }

    /// True when the entry and every ancestor up to the root is live and not
    /// excluded.
    fn chain_alive(&self, mut entry: u32) -> bool {
        for _ in 0..MAX_DEPTH {
            let flags = self.flags[entry as usize];
            if flags & (FLAG_DEAD | FLAG_EXCLUDED) != 0 {
                return false;
            }
            if flags & FLAG_ROOT != 0 {
                return true;
            }
            entry = self.parent[entry as usize];
            if entry == NONE {
                return false;
            }
        }
        false
    }

    /// True when `entry` is a live, shown entry: its ancestors are all live
    /// and not excluded.
    fn parent_alive(&self, entry: usize) -> bool {
        let parent = self.parent[entry];
        parent != NONE && self.chain_alive(parent)
    }

    /// True when the entry can have children.
    fn accepts(&self, entry: u32) -> bool {
        self.flags[entry as usize] & FLAG_DIRECTORY != 0 && self.chain_alive(entry)
    }

    /// True when `ancestor` is `from` or one of its ancestors.
    fn descends_from(&self, mut from: u32, ancestor: u32) -> bool {
        for _ in 0..MAX_DEPTH {
            if from == ancestor {
                return true;
            }
            from = self.parent[from as usize];
            if from == NONE {
                return false;
            }
        }
        true
    }

    /// The display path of an entry, built from the parent chain.
    fn path_of(&self, entry: u32) -> String {
        let mut chain: Vec<u32> = Vec::with_capacity(16);
        let mut current = entry;
        while current != NONE && chain.len() < MAX_DEPTH {
            if self.flags[current as usize] & FLAG_ROOT != 0 {
                break;
            }
            chain.push(current);
            current = self.parent[current as usize];
        }
        let length = self.root_path.len()
            + chain
                .iter()
                .map(|&e| 1 + self.display_name(e as usize).len())
                .sum::<usize>();
        let mut path = String::with_capacity(length);
        path.push_str(&self.root_path);
        for &e in chain.iter().rev() {
            path.push('\\');
            path.push_str(self.display_name(e as usize));
        }
        path
    }

    /// The location penalty / `PENALTY_STEP` of everything below the root.
    fn root_below(&self) -> u8 {
        let mut path = self.root_path.clone();
        path.push('\\');
        (path_penalty(&path, "") / PENALTY_STEP) as u8
    }

    /// The location penalty / `PENALTY_STEP` of everything below folder
    /// `dir`: `path_penalty` sees a needle only when a separator closes it.
    fn below_of(&self, dir: u32) -> u8 {
        let mut path = self.path_of(dir);
        path.push('\\');
        (path_penalty(&path, "") / PENALTY_STEP) as u8
    }

    /// The flags (penalty, kind) of an entry below a folder whose
    /// below-penalty is `parent_below`. The penalty equals `path_penalty`.
    fn entry_flags(parent_below: u8, node: &FsNode, lower: &str) -> u16 {
        let penalty = (i32::from(parent_below) * PENALTY_STEP).max(path_penalty("", lower));
        let mut flags = (penalty / PENALTY_STEP) as u16 & FLAG_PENALTY_MASK;
        if node.is_dir {
            flags |= FLAG_DIRECTORY;
            if node.excluded {
                flags |= FLAG_EXCLUDED;
            }
        }
        flags
    }

    /// The places of an entry: path depth (high 16 bits) and path byte length
    /// (low 16 bits), each at most 0xffff. They are the final sort's tie
    /// breaks after the name, and equal `path_depth` and the length of the
    /// display path. Computed from the parent chain, so they are always
    /// current. An entry whose chain does not reach the root (a parent still
    /// missing, or a cycle) counts from zero.
    pub(super) fn places_of(&self, entry: u32) -> u32 {
        let mut depth = 0u32;
        let mut length = 0u32;
        let mut current = entry;
        let mut base = 0;
        for _ in 0..MAX_DEPTH {
            if current == NONE {
                break;
            }
            if self.flags[current as usize] & FLAG_ROOT != 0 {
                base = self.root_places;
                break;
            }
            depth += 1;
            length += 1 + self.display_len(current as usize) as u32;
            current = self.parent[current as usize];
        }
        let depth = ((base >> 16) + depth).min(0xffff);
        let length = ((base & 0xffff) + length).min(0xffff);
        depth << 16 | length
    }

    /// The `display` value and the lowercase name number of `name`. A name
    /// equal to its lowercase form gets the lowercase number as its display
    /// value; any other name gets a case variant.
    fn intern_name(&mut self, name: &str, lower: &str) -> Option<(u32, u32)> {
        let lower_number = self.names.intern(lower, name_hash(lower))?;
        if lower == name {
            return Some((lower_number, lower_number));
        }
        let variant = self.variants.intern(name, lower_number, name_hash(name))?;
        Some((VARIANT_TAG | variant, lower_number))
    }

    /// Adds a new entry below `parent`. Returns its number.
    fn push_entry(&mut self, node: &FsNode, parent: u32, parent_below: u8) -> Option<u32> {
        let index = u32::try_from(self.parent.len()).ok().filter(|&i| i != NONE)?;
        let lower = node.name.to_lowercase();
        let (display, lower_number) = self.intern_name(&node.name, &lower)?;
        let flags = Self::entry_flags(parent_below, node, &lower);
        room(&mut self.parent);
        room(&mut self.display);
        room(&mut self.flags);
        room(&mut self.seq);
        room(&mut self.next);
        self.parent.push(parent);
        self.display.push(display);
        self.flags.push(flags);
        self.seq.push(node.seq);
        self.next.push(NONE);
        self.link(lower_number, index);
        self.set_record(node.record, index);
        Some(index)
    }

    /// Puts `entry` at the head of the chain of name `name`.
    fn link(&mut self, name: u32, entry: u32) {
        let head = self.names.heads[name as usize];
        self.next[entry as usize] = head;
        self.names.heads[name as usize] = entry;
    }

    /// Takes `entry` out of the chain of name `name`.
    fn unlink(&mut self, name: u32, entry: u32) {
        let head = self.names.heads[name as usize];
        if head == entry {
            self.names.heads[name as usize] = self.next[entry as usize];
            return;
        }
        let mut current = head;
        while current != NONE {
            let following = self.next[current as usize];
            if following == entry {
                self.next[current as usize] = self.next[entry as usize];
                return;
            }
            current = following;
        }
    }

    /// Marks an entry dead. The root never dies. Returns true when the entry
    /// was alive.
    fn kill(&mut self, entry: u32) -> bool {
        let flags = &mut self.flags[entry as usize];
        if *flags & (FLAG_DEAD | FLAG_ROOT) == 0 {
            *flags |= FLAG_DEAD;
            self.deleted += 1;
            true
        } else {
            false
        }
    }

    /// Applies one change. Returns true when it changed an entry.
    fn apply_one(&mut self, change: &FsChange) -> bool {
        match change {
            FsChange::Upsert(node) => self.upsert(node),
            FsChange::Remove { record, seq } => match self.entry_of(*record) {
                Some(entry) if self.seq[entry as usize] == *seq => self.kill(entry),
                _ => false,
            },
        }
    }

    fn upsert(&mut self, node: &FsNode) -> bool {
        if node.record >= MAX_RECORD || node.record == self.meta.root_record {
            return false;
        }
        let existing = self.entry_of(node.record);
        let parent = self
            .entry_of(node.parent_record)
            .filter(|&parent| node.parent_record != node.record && self.accepts(parent));
        // A folder that is moved to an unknown, dead or excluded place (the
        // recycle bin, an excluded folder, another volume) is gone.
        let Some(parent) = parent else {
            return existing.is_some_and(|entry| self.kill(entry));
        };
        match existing {
            Some(entry)
                if self.seq[entry as usize] == node.seq
                    && (self.flags[entry as usize] & FLAG_DIRECTORY != 0) == node.is_dir =>
            {
                self.update_in_place(entry, node, parent)
            }
            // The record belongs to another file now.
            Some(entry) => {
                self.kill(entry);
                self.insert(node, parent);
                true
            }
            None => {
                self.insert(node, parent);
                true
            }
        }
    }

    fn insert(&mut self, node: &FsNode, parent: u32) {
        let below = self.below_of(parent);
        self.push_entry(node, parent, below);
    }

    /// A rename or a move of an existing entry (or its return after a remove).
    /// Its children keep their parent number. Returns true when the entry
    /// changed.
    fn update_in_place(&mut self, entry: u32, node: &FsNode, parent: u32) -> bool {
        let index = entry as usize;
        let was_dead = self.flags[index] & FLAG_DEAD != 0;
        let lower = node.name.to_lowercase();
        let Some((display, lower_number)) = self.intern_name(&node.name, &lower) else {
            self.kill(entry);
            return true;
        };
        let excluded = node.is_dir && node.excluded;
        if !was_dead
            && self.display[index] == display
            && self.parent[index] == parent
            && (self.flags[index] & FLAG_EXCLUDED != 0) == excluded
        {
            return false;
        }
        // A folder cannot move under itself.
        if node.is_dir && self.parent[index] != parent && self.descends_from(parent, entry) {
            return self.kill(entry);
        }
        let old_lower = self.lower_of(index);
        if old_lower != lower_number {
            self.unlink(old_lower, entry);
            self.link(lower_number, entry);
        }
        let below = self.below_of(parent);
        let flags = Self::entry_flags(below, node, &lower);
        self.display[index] = display;
        self.parent[index] = parent;
        self.flags[index] = flags;
        if was_dead {
            self.deleted -= 1;
        }
        true
    }

    /// A segment of `nodes`. They may come in any order.
    fn build(id: u64, meta: VolumeMeta, nodes: impl IntoIterator<Item = FsNode>) -> Self {
        let mut segment = Self::new(id, meta);
        // The below-penalty of each entry: the files of a folder take it from
        // the folder, so it is computed once per folder.
        let mut below = vec![segment.root_below()];
        // Nodes whose parent has not come yet, by the record they wait for.
        let mut waiting: HashMap<u64, Vec<FsNode>> = HashMap::new();
        let mut work = Vec::new();
        for node in nodes {
            work.push(node);
            while let Some(node) = work.pop() {
                if node.record >= MAX_RECORD
                    || node.record == segment.meta.root_record
                    || segment.entry_of(node.record).is_some()
                {
                    continue;
                }
                let Some(parent) = segment.entry_of(node.parent_record) else {
                    waiting.entry(node.parent_record).or_default().push(node);
                    continue;
                };
                if segment.flags[parent as usize] & (FLAG_DIRECTORY | FLAG_EXCLUDED)
                    != FLAG_DIRECTORY
                {
                    continue;
                }
                let Some(index) = segment.push_entry(&node, parent, below[parent as usize]) else {
                    continue;
                };
                below.push(if node.is_dir && !node.excluded {
                    segment.below_of(index)
                } else {
                    0
                });
                if let Some(children) = waiting.remove(&node.record) {
                    work.extend(children);
                }
            }
        }
        segment.shrink();
        segment
    }

    /// Computes the penalty of every entry again, from the chains. Parents
    /// may come after their children in the arrays.
    fn recompute_penalties(&mut self) {
        let count = self.parent.len();
        let mut done = vec![false; count];
        let mut below = vec![0u8; count];
        done[0] = true;
        below[0] = self.root_below();
        let mut stack: Vec<usize> = Vec::new();
        for start in 1..count {
            if done[start] {
                continue;
            }
            stack.clear();
            let mut current = start;
            // Climb to a finished entry (or to a missing parent, or a cycle).
            let mut base_missing = false;
            loop {
                stack.push(current);
                let parent = self.parent[current];
                if parent == NONE || stack.len() > MAX_DEPTH {
                    base_missing = true;
                    break;
                }
                if done[parent as usize] {
                    break;
                }
                current = parent as usize;
            }
            for (position, &entry) in stack.iter().enumerate().rev() {
                let top = position + 1 == stack.len();
                let parent = self.parent[entry];
                let parent_below = if top && base_missing { 0 } else { below[parent as usize] };
                let lower = self.names.name(self.lower_of(entry) as usize);
                let penalty = (i32::from(parent_below) * PENALTY_STEP).max(path_penalty("", lower));
                let flags = self.flags[entry];
                self.flags[entry] = (flags & !FLAG_PENALTY_MASK)
                    | ((penalty / PENALTY_STEP) as u16 & FLAG_PENALTY_MASK);
                if flags & FLAG_DIRECTORY != 0 && flags & FLAG_EXCLUDED == 0 {
                    below[entry] = self.below_of(entry as u32);
                }
                done[entry] = true;
            }
        }
    }
}

/// The segments of a store, by slot.
#[derive(Default)]
struct Inner {
    segs: Vec<Option<Segment>>,
}

impl Inner {
    fn segment(&self, slot: VolumeSlot) -> Option<&Segment> {
        self.segs.get(usize::from(slot))?.as_ref()
    }

    fn segment_mut(&mut self, slot: VolumeSlot) -> Option<&mut Segment> {
        self.segs.get_mut(usize::from(slot))?.as_mut()
    }

    fn segments(&self) -> impl Iterator<Item = &Segment> {
        self.segs.iter().flatten()
    }

    /// The segment of a hit: the hit came from it under the same lock.
    fn seg(&self, slot: VolumeSlot) -> &Segment {
        self.segment(slot).expect("a hit's segment is in the store")
    }
}

/// The in-memory file index (see the module comment). Share it as
/// `Arc<FileStore>`.
pub struct FileStore {
    inner: RwLock<Inner>,
    next_id: AtomicU64,
    /// The store changed since the last `save`.
    dirty: AtomicBool,
}

impl Default for FileStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FileStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
            next_id: AtomicU64::new(1),
            dirty: AtomicBool::new(false),
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    fn new_id(&self) -> u64 {
        self.next_id.fetch_add(1, AtomicOrdering::Relaxed)
    }

    pub fn volume_slot(&self, volume_id: &str) -> Option<VolumeSlot> {
        let inner = self.read();
        let slot = inner
            .segments()
            .find(|segment| segment.meta.volume_id == volume_id)
            .map(|segment| segment.slot);
        slot
    }

    /// Builds a volume's entries in new arrays beside the live ones (searches
    /// keep running) and swaps them in. Replaces any old entries of that
    /// volume. `nodes` may come in any order: a node whose parent has not
    /// come yet waits for it, and a node whose parent never comes is dropped.
    /// A second node with the same record is dropped.
    pub fn replace_volume(
        &self,
        meta: VolumeMeta,
        nodes: impl IntoIterator<Item = FsNode>,
    ) -> VolumeSlot {
        self.replace_volume_checked(meta, nodes, || true)
            .expect("a replace with no check always commits")
    }

    /// `replace_volume`, with a check after the build: the new arrays are
    /// swapped in only when `commit` returns true (for a walk that failed
    /// while `nodes` was still being read, it returns false). Returns `None`
    /// when the old entries stay.
    pub fn replace_volume_checked(
        &self,
        meta: VolumeMeta,
        nodes: impl IntoIterator<Item = FsNode>,
        commit: impl FnOnce() -> bool,
    ) -> Option<VolumeSlot> {
        let mut segment = Segment::build(self.new_id(), meta, nodes);
        if !commit() {
            return None;
        }
        let (slot, old) = {
            let mut inner = self.write();
            let slot = inner
                .segs
                .iter()
                .position(|held| {
                    held.as_ref()
                        .is_some_and(|held| held.meta.volume_id == segment.meta.volume_id)
                })
                .or_else(|| inner.segs.iter().position(Option::is_none))
                .unwrap_or_else(|| {
                    inner.segs.push(None);
                    inner.segs.len() - 1
                });
            let slot = VolumeSlot::try_from(slot).expect("fewer than 65536 volumes");
            segment.slot = slot;
            let old = inner.segs[usize::from(slot)].replace(segment);
            (slot, old)
        };
        // The old arrays are large: free them outside the lock.
        drop(old);
        self.dirty.store(true, AtomicOrdering::Release);
        Some(slot)
    }

    pub fn remove_volume(&self, volume_id: &str) {
        let old = {
            let mut inner = self.write();
            let slot = inner.segs.iter().position(|held| {
                held.as_ref().is_some_and(|held| held.meta.volume_id == volume_id)
            });
            slot.and_then(|slot| inner.segs[slot].take())
        };
        if old.is_some() {
            self.dirty.store(true, AtomicOrdering::Release);
        }
        drop(old);
    }

    /// Applies journal or walk changes in order. An upsert inserts, or moves
    /// and renames in place (children keep their parent). An upsert whose
    /// parent record is unknown, dead or excluded is a remove. A remove of a
    /// folder marks it dead; its descendants are dead by ancestry. A remove
    /// whose `seq` is not the stored one is stale and does nothing; an upsert
    /// with another `seq` replaces the entry (the record was reused).
    /// Returns true when an entry changed. A compaction that the changes
    /// start does not count: it changes no entry that a search can return.
    pub fn apply(&self, slot: VolumeSlot, changes: &[FsChange]) -> bool {
        self.apply_inner(slot, changes, None)
    }

    /// `apply`, and the journal checkpoint that the changes reach, in one
    /// step: a `save` never sees one without the other.
    pub fn apply_and_checkpoint(
        &self,
        slot: VolumeSlot,
        changes: &[FsChange],
        journal_id: u64,
        next_usn: i64,
    ) -> bool {
        self.apply_inner(slot, changes, Some((journal_id, next_usn)))
    }

    fn apply_inner(&self, slot: VolumeSlot, changes: &[FsChange], checkpoint: Option<(u64, i64)>) -> bool {
        let (due, changed) = {
            let mut inner = self.write();
            let Some(segment) = inner.segment_mut(slot) else {
                return false;
            };
            let mut changed = false;
            for change in changes {
                changed |= segment.apply_one(change);
            }
            if let Some(log) = segment.log.as_mut() {
                log.extend_from_slice(changes);
            }
            if let Some((journal_id, next_usn)) = checkpoint {
                segment.meta.journal_id = journal_id;
                segment.meta.next_usn = next_usn;
            }
            (segment.has_many_tombstones(), changed)
        };
        // A batch with no effect (records of excluded folders) leaves the
        // store clean: only the checkpoint moved.
        if changed {
            self.dirty.store(true, AtomicOrdering::Release);
        }
        if due {
            self.compact_slot(slot);
        }
        changed
    }

    /// True when `record` is a live, non-excluded folder, or the volume root.
    /// The journal reader uses it to skip the records in excluded or unknown
    /// folders before it opens any file.
    pub fn accepts_children(&self, slot: VolumeSlot, record: u64) -> bool {
        let inner = self.read();
        let Some(segment) = inner.segment(slot) else {
            return false;
        };
        segment.entry_of(record).is_some_and(|entry| segment.accepts(entry))
    }

    /// Every entry that a search can return whose display path starts with
    /// `prefix` (compare in lowercase), with its folder flag. A full scan:
    /// for checks and probes, never for a search.
    pub fn paths_below(&self, prefix: &str) -> Vec<(String, bool)> {
        let prefix = prefix.to_lowercase();
        let inner = self.read();
        let mut paths = Vec::new();
        for segment in inner.segments() {
            for entry in 1..segment.parent.len() {
                let flags = segment.flags[entry];
                if flags & (FLAG_DEAD | FLAG_ROOT | FLAG_EXCLUDED) != 0 || !segment.parent_alive(entry) {
                    continue;
                }
                let path = segment.path_of(entry as u32);
                if path.to_lowercase().starts_with(&prefix) {
                    paths.push((path, flags & FLAG_DIRECTORY != 0));
                }
            }
        }
        paths
    }

    /// The display path of a live, non-excluded folder, or `None`.
    pub fn folder_path(&self, slot: VolumeSlot, record: u64) -> Option<String> {
        let inner = self.read();
        let segment = inner.segment(slot)?;
        let entry = segment.entry_of(record).filter(|&entry| segment.accepts(entry))?;
        Some(segment.path_of(entry))
    }

    /// Moves the journal checkpoint forward over records that changed
    /// nothing (the read skips them). It does not make the store dirty: a
    /// snapshot of unchanged entries is not worth a write, but the next
    /// `save` keeps the newer checkpoint.
    pub fn advance_checkpoint(&self, slot: VolumeSlot, journal_id: u64, next_usn: i64) {
        if let Some(segment) = self.write().segment_mut(slot) {
            segment.meta.journal_id = journal_id;
            segment.meta.next_usn = next_usn;
        }
    }

    pub fn set_checkpoint(&self, slot: VolumeSlot, journal_id: u64, next_usn: i64) {
        if let Some(segment) = self.write().segment_mut(slot) {
            segment.meta.journal_id = journal_id;
            segment.meta.next_usn = next_usn;
            self.dirty.store(true, AtomicOrdering::Release);
        }
    }

    pub fn volumes(&self) -> Vec<VolumeMeta> {
        self.read().segments().map(|segment| segment.meta.clone()).collect()
    }

    /// Entries of the volume that a search can return, without the entries
    /// below a removed folder that no search or compaction has found yet.
    pub fn live_count(&self, slot: VolumeSlot) -> u64 {
        self.read().segment(slot).map_or(0, Segment::live)
    }

    /// Bytes the arrays, names and tables of every volume hold (their
    /// capacity, not their length).
    pub fn memory_bytes(&self) -> usize {
        self.read().segments().map(Segment::memory_bytes).sum()
    }

    /// True when the store changed since the last `save`.
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(AtomicOrdering::Acquire)
    }

    /// Writes the snapshot file (see `snapshot.rs`). Returns its size. The
    /// read lock is held only while the bytes are made in memory: the disk
    /// write runs with no lock, so a waiting writer does not block searches.
    pub fn save(&self, path: &Path) -> Result<u64, String> {
        // A change during the write sets the flag again.
        self.dirty.store(false, AtomicOrdering::Release);
        let bytes = {
            let inner = self.read();
            let segments = inner.segments().collect::<Vec<_>>();
            super::snapshot::serialize(&segments)
        };
        let result = bytes.and_then(|bytes| super::snapshot::write_file(path, &bytes));
        if result.is_err() {
            self.dirty.store(true, AtomicOrdering::Release);
        }
        result
    }

    /// Reads a snapshot file. Any problem (a bad checksum, a truncated file,
    /// an unknown version, an index out of range) is an error: the caller
    /// builds the store again.
    pub fn load(path: &Path) -> Result<FileStore, String> {
        let segments = super::snapshot::read(path)?;
        let store = FileStore::new();
        {
            let mut inner = store.write();
            for mut segment in segments {
                segment.id = store.new_id();
                segment.slot = VolumeSlot::try_from(inner.segs.len())
                    .map_err(|_| "too many volumes".to_string())?;
                inner.segs.push(Some(segment));
            }
        }
        Ok(store)
    }

    /// Fallback candidates for a query, best first. `typos` is false when
    /// another source has a strong match.
    pub fn candidates(
        &self,
        query: &NameQuery<'_>,
        limit: usize,
        typos: bool,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> IndexCandidates {
        let wanted = (limit * CANDIDATES_PER_RESULT).max(MIN_CANDIDATES);
        let inner = self.read();
        let mut hits = inner.top_hits(query, wanted, is_cancelled);
        // No strong match: add typo matches, and rank every close name (typo
        // or scattered subsequence) above the rest.
        if typos && query.wants_typos() && !inner.has_strong_hit(&hits, query) && !is_cancelled() {
            let mut closeness = Closeness::new(query);
            for hit in &mut hits {
                if let Some(rank) = inner.close_name_rank(inner.seg(hit.seg), hit.name as usize, &mut closeness)
                {
                    *hit = Hit {
                        score: inner.close_entry_rank(rank, inner.seg(hit.seg), hit.index as usize),
                        typo: true,
                        ..*hit
                    };
                }
            }
            let typo_hits = inner.typo_hits(query, MAX_TYPO_HITS, &hits, &mut closeness);
            hits.extend(typo_hits);
            inner.rank(&mut hits, wanted);
        }
        if is_cancelled() {
            return IndexCandidates::default();
        }
        let mut found = IndexCandidates::default();
        for hit in &hits {
            let entry = inner.seg(hit.seg).candidate(hit);
            if hit.typo {
                found.typos.push((entry, hit.score));
            } else {
                found.entries.push(entry);
            }
        }
        found
    }

    /// Builds new arrays without the dead entries and without the entries
    /// below a dead or excluded folder, recomputes their penalty
    /// from the chains, and swaps them in. Searches run during the build (it
    /// holds the read lock in slices). Compacts every volume that has
    /// tombstones. Returns `None` when there was nothing to do.
    ///
    /// `apply` calls this itself when the tombstones pass `COMPACT_SHARE`.
    pub fn compact(&self) -> Option<Compaction> {
        let slots = self
            .read()
            .segments()
            .map(|segment| segment.slot)
            .collect::<Vec<_>>();
        slots
            .into_iter()
            .filter_map(|slot| self.compact_slot(slot))
            .reduce(|a, b| Compaction {
                dropped: a.dropped + b.dropped,
                kept: a.kept + b.kept,
                build: a.build + b.build,
                locked: a.locked + b.locked,
            })
    }

    fn compact_slot(&self, slot: VolumeSlot) -> Option<Compaction> {
        let started = Instant::now();
        let begun = self.compact_begin(slot)?;
        let Some((fresh, dropped)) = self.compact_build(slot, &begun) else {
            self.compact_abort(slot, begun.id);
            return None;
        };
        let build = started.elapsed();
        let kept = fresh.entries() - 1;
        let (locked, old) = self.compact_finish(slot, begun.id, fresh)?;
        // The old arrays are large: free them outside the lock.
        drop(old);
        self.dirty.store(true, AtomicOrdering::Release);
        Some(Compaction { dropped, kept, build, locked })
    }

    /// Starts the log of the changes that come during the build.
    fn compact_begin(&self, slot: VolumeSlot) -> Option<CompactionStart> {
        let mut inner = self.write();
        let segment = inner.segment_mut(slot)?;
        if segment.log.is_some()
            || (segment.deleted == 0 && segment.hidden_seen.load(AtomicOrdering::Relaxed) == 0)
        {
            return None;
        }
        segment.log = Some(Vec::new());
        Some(CompactionStart {
            id: segment.id,
            entries: segment.entries(),
            records: segment.by_record.len(),
        })
    }

    fn compact_abort(&self, slot: VolumeSlot, id: u64) {
        if let Some(segment) = self.write().segment_mut(slot).filter(|s| s.id == id) {
            segment.log = None;
        }
    }

    /// Runs `read` on the segment under a read lock, or returns `None` when a
    /// `replace_volume` or `remove_volume` replaced it.
    fn with_segment<R>(&self, slot: VolumeSlot, id: u64, read: impl FnOnce(&Segment) -> R) -> Option<R> {
        let inner = self.read();
        let segment = inner.segment(slot).filter(|s| s.id == id)?;
        Some(read(segment))
    }

    /// The new arrays, and the number of entries they leave out. The build
    /// copies slices of entries under one read lock each. What changes in
    /// between is safe: the swap replays the logged changes on the new arrays
    /// (each change is idempotent), and the entries that a change appended
    /// lie above `start.entries`.
    fn compact_build(&self, slot: VolumeSlot, start: &CompactionStart) -> Option<(Segment, usize)> {
        let (id, entries, records) = (start.id, start.entries, start.records);
        let slices = |count: usize| {
            (0..count)
                .step_by(COMPACT_SLICE)
                .map(move |from| from..(from + COMPACT_SLICE).min(count))
        };

        // The entries that stay: live, below live folders that are not
        // excluded. A parent that came after the start is unknown: the
        // entry stays and the log gives it its parent.
        let mut state = vec![0u8; entries];
        state[0] = 1;
        let mut stack = Vec::new();
        for range in slices(entries) {
            self.with_segment(slot, id, |segment| {
                for entry in range {
                    if state[entry] == 0 {
                        decide_kept(segment, entry, entries, &mut state, &mut stack);
                    }
                }
            })?;
        }
        let mut new_index = vec![NONE; entries];
        let mut count = 0u32;
        for (entry, &kept) in state.iter().enumerate() {
            if kept == 1 {
                new_index[entry] = count;
                count += 1;
            }
        }
        let kept = count as usize;

        let meta = self.with_segment(slot, id, |segment| segment.meta.clone())?;
        let mut fresh = Segment::new(self.new_id(), meta);
        fresh.parent.reserve_exact(kept);
        fresh.display.reserve_exact(kept);
        fresh.flags.reserve_exact(kept);
        fresh.seq.reserve_exact(kept);
        fresh.next.reserve_exact(kept);
        // Names and case variants are copied once each, by text, into the new
        // pools: only the ones that a kept entry uses. The maps go from an old
        // lowercase name number and an old variant number to the new
        // `display` value.
        let mut plain_map: Vec<u32> = Vec::new();
        let mut variant_map: Vec<u32> = Vec::new();
        for range in slices(entries) {
            self.with_segment(slot, id, |segment| {
                for entry in range {
                    if entry == 0 || state[entry] != 1 {
                        continue;
                    }
                    let old_display = segment.display[entry];
                    let (map, number) = if old_display & VARIANT_TAG == 0 {
                        (&mut plain_map, old_display as usize)
                    } else {
                        (&mut variant_map, (old_display & !VARIANT_TAG) as usize)
                    };
                    if number >= map.len() {
                        map.resize(number + 1, NONE);
                    }
                    if map[number] == NONE {
                        let exact = segment.display_name(entry);
                        let lower = segment.names.name(segment.lower_of(entry) as usize);
                        map[number] = fresh.intern_name(exact, lower)?.0;
                    }
                    let display = map[number];
                    let parent = segment.parent[entry];
                    let parent = if parent == NONE || parent as usize >= entries {
                        NONE
                    } else {
                        new_index[parent as usize]
                    };
                    let flags = segment.flags[entry];
                    let index = fresh.parent.len() as u32;
                    fresh.parent.push(parent);
                    fresh.display.push(display);
                    fresh.flags.push(flags);
                    fresh.seq.push(segment.seq[entry]);
                    fresh.next.push(NONE);
                    if flags & FLAG_DEAD != 0 {
                        // Died after the decision: the log kills it too.
                        fresh.deleted += 1;
                    }
                    fresh.link(fresh.lower_of_display(display), index);
                }
                Some(())
            })??;
        }
        fresh.by_record = vec![NONE; records];
        for range in slices(records) {
            self.with_segment(slot, id, |segment| {
                for record in range {
                    let old = segment.by_record.get(record).copied().unwrap_or(NONE);
                    if old != NONE && (old as usize) < entries {
                        fresh.by_record[record] = new_index[old as usize];
                    }
                }
            })?;
        }
        fresh.recompute_penalties();
        fresh.shrink();
        Some((fresh, entries - kept))
    }

    /// Swaps the new arrays in. The changes applied since the build began
    /// are replayed on them. Returns how long the swap held the write lock,
    /// and the old segment.
    fn compact_finish(
        &self,
        slot: VolumeSlot,
        id: u64,
        mut fresh: Segment,
    ) -> Option<(Duration, Segment)> {
        let locked_at = Instant::now();
        let mut inner = self.write();
        let live = inner.segment_mut(slot).filter(|s| s.id == id)?;
        let log = live.log.take().unwrap_or_default();
        // The checkpoint of the live segment is the newest.
        fresh.meta = live.meta.clone();
        fresh.slot = slot;
        for change in &log {
            fresh.apply_one(change);
        }
        let old = std::mem::replace(live, fresh);
        Some((locked_at.elapsed(), old))
    }
}

/// What a compaction needs to know about the live arrays when it starts.
struct CompactionStart {
    id: u64,
    entries: usize,
    records: usize,
}

/// Sets `state` (1 stays, 2 goes) of `start` and of the ancestors that it
/// had to look at. An entry stays when it is live and its parent stays and is
/// not excluded.
fn decide_kept(
    segment: &Segment,
    start: usize,
    entries: usize,
    state: &mut [u8],
    stack: &mut Vec<usize>,
) {
    stack.clear();
    let mut current = start;
    let verdict = loop {
        match state[current] {
            0 => {}
            known => break known,
        }
        if segment.flags[current] & FLAG_DEAD != 0 {
            state[current] = 2;
            break 2;
        }
        stack.push(current);
        let parent = segment.parent[current];
        if parent == NONE || stack.len() > MAX_DEPTH {
            break 2;
        }
        // A parent that came after the start: the log tells where it is.
        if parent as usize >= entries {
            break 1;
        }
        if segment.flags[parent as usize] & FLAG_EXCLUDED != 0 {
            break 2;
        }
        current = parent as usize;
    };
    for &entry in stack.iter() {
        state[entry] = verdict;
    }
}

impl Segment {
    /// The result row of a hit.
    fn candidate(&self, hit: &Hit) -> CandidateEntry {
        let entry = hit.index as usize;
        let is_directory = self.flags[entry] & FLAG_DIRECTORY != 0;
        CandidateEntry {
            // The slot in the high half keeps the numbers of two volumes apart.
            id: i64::from(self.slot) << 32 | i64::from(hit.index),
            display_path: self.path_of(hit.index),
            lower_name: self.names.name(hit.name as usize).to_string(),
            is_directory,
        }
    }
}

impl Inner {
    /// The final sort in `search::search_with_generation` (score, name
    /// length, name, path depth, path length), with the entry number as the
    /// last tie break so the selection is deterministic.
    fn compare(&self, left: &Hit, right: &Hit) -> Ordering {
        self.compare_names(left, right)
            .then_with(|| self.places(left).cmp(&self.places(right)))
            .then_with(|| (left.seg, left.index).cmp(&(right.seg, right.index)))
    }

    /// `compare` without the places and the entry number: the order of the
    /// score and the name.
    fn compare_names(&self, left: &Hit, right: &Hit) -> Ordering {
        right.score.cmp(&left.score).then_with(|| {
            // One number per distinct name in a segment: equal numbers,
            // equal names.
            if left.seg == right.seg && left.name == right.name {
                return Ordering::Equal;
            }
            let left_name = self.seg(left.seg).names.name(left.name as usize);
            let right_name = self.seg(right.seg).names.name(right.name as usize);
            left_name
                .len()
                .cmp(&right_name.len())
                .then_with(|| left_name.cmp(right_name))
        })
    }

    /// The places of a hit: the kept value, or a walk of the parent chain.
    fn places(&self, hit: &Hit) -> u32 {
        if hit.places == NO_PLACES {
            self.seg(hit.seg).places_of(hit.index)
        } else {
            hit.places
        }
    }

    /// Keeps the value of the places in the hit, so a hit that ties many
    /// times walks its parent chain once.
    fn fill_places(&self, hit: &mut Hit) {
        if hit.places == NO_PLACES {
            hit.places = self.seg(hit.seg).places_of(hit.index);
        }
    }

    /// Keeps the `wanted` best hits (not sorted). Only the hits that tie with
    /// the last kept hit on score and name need their places.
    fn keep_best(&self, hits: &mut Vec<Hit>, wanted: usize) {
        if hits.len() <= wanted {
            return;
        }
        hits.select_nth_unstable_by(wanted - 1, |a, b| self.compare_names(a, b));
        let edge = hits[wanted - 1];
        let mut tied = 0;
        for hit in hits.iter_mut() {
            if self.compare_names(hit, &edge) == Ordering::Equal {
                self.fill_places(hit);
                tied += 1;
            }
        }
        if tied > 1 {
            hits.select_nth_unstable_by(wanted - 1, |a, b| self.compare(a, b));
        }
        hits.truncate(wanted);
    }

    /// Keeps the `wanted` best hits and sorts them. The places are computed
    /// only for hits that tie with a neighbour on score and name.
    fn rank(&self, hits: &mut Vec<Hit>, wanted: usize) {
        self.keep_best(hits, wanted);
        hits.sort_unstable_by(|a, b| self.compare_names(a, b));
        for index in 1..hits.len() {
            if self.compare_names(&hits[index - 1], &hits[index]) == Ordering::Equal {
                self.fill_places(&mut hits[index - 1]);
                self.fill_places(&mut hits[index]);
            }
        }
        hits.sort_unstable_by(|a, b| self.compare(a, b));
    }

    fn top_hits(
        &self,
        query: &NameQuery<'_>,
        wanted: usize,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Vec<Hit> {
        let mut hits = Vec::new();
        for segment in self.segments() {
            let total = segment.names.len();
            let threads = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1)
                .min(MAX_SCAN_THREADS)
                .min(total.div_ceil(MIN_ROWS_PER_THREAD))
                .max(1);
            let chunk = total.div_ceil(threads).max(1);
            let mut found = if threads == 1 {
                self.scan(segment, 0..total, query, wanted, is_cancelled)
            } else {
                std::thread::scope(|scope| {
                    let workers = (0..threads)
                        .map(|thread| {
                            let range = thread * chunk..((thread + 1) * chunk).min(total);
                            scope.spawn(move || self.scan(segment, range, query, wanted, is_cancelled))
                        })
                        .collect::<Vec<_>>();
                    workers
                        .into_iter()
                        .flat_map(|worker| worker.join().unwrap_or_default())
                        .collect::<Vec<_>>()
                })
            };
            self.keep_best(&mut found, wanted);
            hits.extend(found);
        }
        self.rank(&mut hits, wanted);
        hits
    }

    /// Scans the names in `range` (name numbers) and expands each admitted
    /// name to its live, shown entries.
    fn scan(
        &self,
        segment: &Segment,
        range: std::ops::Range<usize>,
        query: &NameQuery<'_>,
        wanted: usize,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Vec<Hit> {
        let mut hits = Vec::new();
        // The score of the worst hit that `keep_best` kept. `wanted` hits
        // beat it, so an entry with a lower score cannot reach the top.
        let mut floor = i32::MIN;
        // Entries below a dead or excluded folder that this scan met.
        let mut hidden = 0usize;
        for name in range {
            if name % CANCEL_CHECK_INTERVAL == 0 && is_cancelled() {
                return Vec::new();
            }
            // Every admission rule and scorer tier needs every query
            // character in the name, so the mask test is a necessary
            // condition that rejects most names in one instruction.
            if segment.names.masks[name] & query.mask != query.mask {
                continue;
            }
            let text = segment.names.name(name);
            if !query.admits(text) {
                continue;
            }
            // `name_score` per kind (file, directory), computed when the
            // first live entry of that kind comes up.
            let mut scores: [Option<Option<i32>>; 2] = [None, None];
            let mut entry = segment.names.heads[name];
            while entry != NONE {
                let current = entry as usize;
                entry = segment.next[current];
                let flags = segment.flags[current];
                if flags & (FLAG_DEAD | FLAG_EXCLUDED) != 0 {
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
                if !segment.parent_alive(current) {
                    hidden += 1;
                    continue;
                }
                hits.push(Hit {
                    score,
                    index: current as u32,
                    name: name as u32,
                    seg: segment.slot,
                    typo: false,
                    places: NO_PLACES,
                });
                if hits.len() >= wanted * 8 {
                    self.keep_best(&mut hits, wanted);
                    floor = hits.iter().map(|hit| hit.score).min().unwrap_or(i32::MIN);
                }
            }
        }
        if hidden > 0 {
            segment.hidden_seen.fetch_add(hidden, AtomicOrdering::Relaxed);
        }
        hits
    }

    /// True when an admitted hit is a strong match (see
    /// `search::is_strong_match`).
    fn has_strong_hit(&self, hits: &[Hit], query: &NameQuery<'_>) -> bool {
        hits.iter().any(|hit| {
            let segment = self.seg(hit.seg);
            let directory = segment.flags[hit.index as usize] & FLAG_DIRECTORY != 0;
            is_strong_match(
                segment.names.name(hit.name as usize),
                directory,
                query.tokens,
                query.full,
            )
        })
    }

    /// The rank of a close name (see `CLOSE_TYPO_SCORE`) before the entry's
    /// folder bonus and location penalty (see `close_entry_rank`), or `None`
    /// when the name is not close.
    fn close_name_rank(&self, segment: &Segment, name: usize, closeness: &mut Closeness) -> Option<i32> {
        let text = segment.names.name(name);
        let distance = closeness.distance(text)?;
        Some(
            closeness.base + (TYPO_FAR - distance) as i32 * CLOSE_EDIT_STEP
                - text.len().min(80) as i32,
        )
    }

    /// `close_name_rank` plus the entry's folder bonus, minus its location
    /// penalty.
    fn close_entry_rank(&self, name_rank: i32, segment: &Segment, entry: usize) -> i32 {
        let flags = segment.flags[entry];
        let directory_bonus = if flags & FLAG_DIRECTORY != 0 { 20 } else { 0 };
        let penalty = i32::from(flags & FLAG_PENALTY_MASK) * PENALTY_STEP;
        name_rank + directory_bonus - penalty
    }

    /// Close typo-tolerant matches (FFF's frizbee matcher, SIMD Smith-Waterman)
    /// over every distinct name, expanded to every live, shown entry except
    /// the `admitted` hits. Best first; each hit's score is its final rank
    /// (see `close_name_rank`).
    fn typo_hits(
        &self,
        query: &NameQuery<'_>,
        wanted: usize,
        admitted: &[Hit],
        closeness: &mut Closeness,
    ) -> Vec<Hit> {
        let mut hits = Vec::new();
        for segment in self.segments() {
            hits.extend(self.typo_hits_in(segment, query, wanted, admitted, closeness));
        }
        self.rank(&mut hits, wanted);
        hits
    }

    fn typo_hits_in(
        &self,
        segment: &Segment,
        query: &NameQuery<'_>,
        wanted: usize,
        admitted: &[Hit],
        closeness: &mut Closeness,
    ) -> Vec<Hit> {
        const CHUNK_SPAN: usize = TYPO_MAX_CHUNKS * SIMD_CHUNK_BYTES;
        let names = &segment.names;
        let text = names.text.as_bytes();
        // The matcher reads every name in whole 16-byte chunks, so the last
        // chunk runs up to 15 bytes past the name. Names in the last
        // `CHUNK_SPAN` bytes read from a zero-padded copy, never past `text`.
        let tail_start = text.len().saturating_sub(CHUNK_SPAN);
        let mut tail = text[tail_start..].to_vec();
        tail.resize(tail.len() + SIMD_CHUNK_BYTES, 0);
        // Each character class of the query that a name lacks costs at least
        // one typo: a mask test rejects most names before frizbee reads them.
        // A name whose entries are all dead can match; it expands to no hit.
        let budget = query.typo_budget();
        let resolve = |name: u32, chunks: &mut [*const u8; TYPO_MAX_CHUNKS]| {
            let name = name as usize;
            if (query.mask & !names.masks[name]).count_ones() > budget {
                return None;
            }
            let start = if name == 0 {
                0
            } else {
                names.ends[name - 1] as usize
            };
            let length = names.ends[name] as usize - start;
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
        let started = Instant::now();
        let matches = query
            .typo_matcher()
            .match_range_parallel_resolved(names.len(), &resolve, threads);
        let matched = started.elapsed();
        let match_count = matches.len();

        // Frizbee also accepts scattered matches ("chrmoe" in "chip-remove",
        // "crome" in "cargo.toml"). A misspelled name is the name's start
        // with a letter wrong: only those close names are kept.
        let mut admitted = admitted
            .iter()
            .filter(|hit| hit.seg == segment.slot)
            .map(|hit| hit.index)
            .collect::<Vec<_>>();
        admitted.sort_unstable();
        let mut hits = Vec::new();
        for found in matches {
            let name = found.index as usize;
            let Some(name_rank) = self.close_name_rank(segment, name, closeness) else {
                continue;
            };
            let mut entry = names.heads[name];
            while entry != NONE {
                let current = entry as usize;
                entry = segment.next[current];
                if segment.flags[current] & (FLAG_DEAD | FLAG_EXCLUDED) != 0
                    || admitted.binary_search(&(current as u32)).is_ok()
                    || !segment.parent_alive(current)
                {
                    continue;
                }
                hits.push(Hit {
                    score: self.close_entry_rank(name_rank, segment, current),
                    index: current as u32,
                    name: found.index,
                    seg: segment.slot,
                    typo: true,
                    places: NO_PLACES,
                });
                if hits.len() >= wanted * 8 {
                    self.keep_best(&mut hits, wanted);
                }
            }
        }
        self.rank(&mut hits, wanted);
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

/// A multiplicative hash over eight bytes at a time, for the name tables.
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
mod tests;
