use super::*;
use std::path::PathBuf;

fn meta(volume_id: &str, mount: &str) -> VolumeMeta {
    VolumeMeta {
        volume_id: volume_id.into(),
        mount_path: mount.into(),
        root_record: 5,
        backend: VolumeBackend::Journal,
        journal_id: 7,
        next_usn: 100,
        exclusions: 0,
    }
}

fn node(record: u64, parent: u64, name: &str, is_dir: bool) -> FsNode {
    FsNode {
        record,
        seq: 1,
        parent_record: parent,
        name: name.into(),
        is_dir,
        excluded: false,
        link: false,
    }
}

fn dir(record: u64, parent: u64, name: &str) -> FsNode {
    node(record, parent, name, true)
}

fn file(record: u64, parent: u64, name: &str) -> FsNode {
    node(record, parent, name, false)
}

fn excluded(record: u64, parent: u64, name: &str) -> FsNode {
    FsNode { excluded: true, ..dir(record, parent, name) }
}

fn upsert(node: FsNode) -> FsChange {
    FsChange::Upsert(node)
}

/// `C:\Users\hi\Docs\report.txt`, `C:\Users\hi\Report-2.TXT`, and more.
fn sample() -> Vec<FsNode> {
    vec![
        dir(10, 5, "Users"),
        dir(11, 10, "hi"),
        dir(12, 11, "Docs"),
        file(20, 12, "report.txt"),
        file(21, 11, "Report-2.TXT"),
        file(22, 12, "budget.xlsx"),
    ]
}

fn store_with(nodes: Vec<FsNode>) -> (FileStore, VolumeSlot) {
    let store = FileStore::new();
    let slot = store.replace_volume(meta("vol-c", "C:\\"), nodes);
    (store, slot)
}

/// The display paths a query finds, in rank order.
fn find(store: &FileStore, text: &str) -> Vec<String> {
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    let query = NameQuery::new(text, &tokens);
    store
        .candidates(&query, 500, false, &|| false)
        .entries
        .into_iter()
        .map(|entry| entry.display_path)
        .collect()
}

/// The files whose name is `name` (a query also finds weaker matches).
fn exact(store: &FileStore, name: &str) -> Vec<String> {
    find(store, name)
        .into_iter()
        .filter(|path| path.to_lowercase().ends_with(&format!("\\{}", name.to_lowercase())))
        .collect()
}

fn sorted(mut paths: Vec<String>) -> Vec<String> {
    paths.sort();
    paths
}

fn entries(store: &FileStore, slot: VolumeSlot) -> usize {
    store.read().segment(slot).unwrap().entries()
}

/// Every entry's penalty and places equal what `path_penalty`, `path_depth` and
/// the length of its display path give. The places come from the parent chain.
fn assert_places_match_paths(store: &FileStore) {
    let inner = store.read();
    for segment in inner.segments() {
        for entry in 1..segment.entries() {
            if segment.flags[entry] & FLAG_DEAD != 0 || !segment.parent_alive(entry) {
                continue;
            }
            let path = segment.path_of(entry as u32);
            let lower = segment.names.name(segment.lower_of(entry) as usize);
            let penalty = i32::from(segment.flags[entry] & FLAG_PENALTY_MASK) * PENALTY_STEP;
            assert_eq!(penalty, path_penalty(&path, lower), "penalty of {path}");
            let places = (path_depth(&path).min(0xffff) as u32) << 16 | path.len().min(0xffff) as u32;
            assert_eq!(segment.places_of(entry as u32), places, "places of {path}");
        }
    }
}

/// A folder of its own under `target/tmp`.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("tmp")
            .join(format!(
                "store-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
            ));
        std::fs::create_dir_all(&dir).expect("create the test folder");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn builds_paths_from_the_parent_chain() {
    let (store, slot) = store_with(sample());
    assert_eq!(store.live_count(slot), 6);
    assert_eq!(
        sorted(find(&store, "report")),
        vec!["C:\\Users\\hi\\Docs\\report.txt", "C:\\Users\\hi\\Report-2.TXT"]
    );
    // A child of the volume root has the form `C:\name`.
    let (store, _) = store_with(vec![file(30, 5, "top.txt")]);
    assert_eq!(find(&store, "top"), vec!["C:\\top.txt"]);
    // A mount point below a folder.
    let store = FileStore::new();
    store.replace_volume(meta("vol-m", "D:\\Mount\\"), vec![file(30, 5, "a.txt")]);
    assert_eq!(find(&store, "a.txt"), vec!["D:\\Mount\\a.txt"]);
    assert_places_match_paths(&store);
}

#[test]
fn candidates_carry_name_and_kind() {
    let (store, _) = store_with(sample());
    let tokens = ["report"];
    let query = NameQuery::new("report", &tokens);
    let found = store.candidates(&query, 10, false, &|| false);
    let entry = found
        .entries
        .iter()
        .find(|entry| entry.lower_name == "report-2.txt")
        .unwrap();
    assert!(!entry.is_directory);
    assert_eq!(entry.display_path, "C:\\Users\\hi\\Report-2.TXT");
    let tokens = ["docs"];
    let query = NameQuery::new("docs", &tokens);
    let found = store.candidates(&query, 10, false, &|| false);
    assert_eq!(found.entries.len(), 1);
    assert!(found.entries[0].is_directory);
}

#[test]
fn insert_rename_move_and_remove() {
    let (store, slot) = store_with(sample());
    let before = entries(&store, slot);

    // A new file.
    store.apply(slot, &[upsert(file(23, 12, "new-file.md"))]);
    assert_eq!(find(&store, "new-file"), vec!["C:\\Users\\hi\\Docs\\new-file.md"]);
    assert_eq!(entries(&store, slot), before + 1);

    // A rename in place: the same entry, another name.
    store.apply(slot, &[upsert(file(23, 12, "Renamed.MD"))]);
    assert!(find(&store, "new-file").is_empty());
    assert_eq!(find(&store, "renamed"), vec!["C:\\Users\\hi\\Docs\\Renamed.MD"]);
    assert_eq!(entries(&store, slot), before + 1);

    // A move in place.
    store.apply(slot, &[upsert(file(23, 11, "Renamed.MD"))]);
    assert_eq!(find(&store, "renamed"), vec!["C:\\Users\\hi\\Renamed.MD"]);
    assert_eq!(entries(&store, slot), before + 1);

    // A case-only rename keeps the lowercase name.
    store.apply(slot, &[upsert(file(23, 11, "renamed.md"))]);
    assert_eq!(find(&store, "renamed"), vec!["C:\\Users\\hi\\renamed.md"]);

    // A folder rename changes one entry; the children follow.
    store.apply(slot, &[upsert(dir(12, 11, "Papers"))]);
    assert_eq!(
        sorted(find(&store, "report")),
        vec!["C:\\Users\\hi\\Papers\\report.txt", "C:\\Users\\hi\\Report-2.TXT"]
    );
    assert!(find(&store, "docs").is_empty());
    assert_eq!(find(&store, "papers"), vec!["C:\\Users\\hi\\Papers"]);
    assert_eq!(entries(&store, slot), before + 1);

    // A remove.
    store.apply(slot, &[FsChange::Remove { record: 23, seq: 1 }]);
    assert!(find(&store, "renamed").is_empty());
    assert_eq!(store.live_count(slot), 6);
    // A repeat of the same change does nothing more.
    store.apply(slot, &[FsChange::Remove { record: 23, seq: 1 }]);
    assert_eq!(store.live_count(slot), 6);
    // A stale remove (another sequence number) does nothing.
    store.apply(slot, &[FsChange::Remove { record: 20, seq: 9 }]);
    assert_eq!(exact(&store, "report.txt"), vec!["C:\\Users\\hi\\Papers\\report.txt"]);
    // The children of a renamed folder keep their old penalty until a
    // compaction computes it again; their places are always current.
    store.compact().expect("one tombstone");
    assert_places_match_paths(&store);
}

#[test]
fn a_rename_takes_an_entry_out_of_the_middle_of_a_name_chain() {
    // Three entries share a name; the middle one of the chain is renamed.
    let (store, slot) = store_with(vec![
        dir(10, 5, "a"),
        dir(11, 5, "b"),
        dir(12, 5, "c"),
        file(20, 10, "same.txt"),
        file(21, 11, "same.txt"),
        file(22, 12, "same.txt"),
    ]);
    store.apply(slot, &[upsert(file(21, 11, "other.txt"))]);
    assert_eq!(
        sorted(find(&store, "same.txt")),
        vec!["C:\\a\\same.txt", "C:\\c\\same.txt"]
    );
    store.apply(slot, &[upsert(file(22, 12, "other.txt"))]);
    store.apply(slot, &[upsert(file(20, 10, "other.txt"))]);
    assert!(find(&store, "same.txt").is_empty());
    assert_eq!(find(&store, "other.txt").len(), 3);
}

#[test]
fn descendants_of_a_removed_folder_are_dead_by_ancestry() {
    let (store, slot) = store_with(sample());
    store.apply(slot, &[FsChange::Remove { record: 12, seq: 1 }]);
    assert!(exact(&store, "report.txt").is_empty());
    assert!(find(&store, "budget").is_empty());
    assert!(find(&store, "docs").is_empty());
    // The other files stay.
    assert_eq!(find(&store, "report-2"), vec!["C:\\Users\\hi\\Report-2.TXT"]);
    assert!(!store.accepts_children(slot, 12));
    // A change below the dead folder is a remove.
    store.apply(slot, &[upsert(file(24, 12, "late.txt"))]);
    assert!(find(&store, "late").is_empty());
    // The folder comes back (a restore from the recycle bin): its children
    // come with it.
    store.apply(slot, &[upsert(dir(12, 11, "Docs"))]);
    assert_eq!(find(&store, "budget"), vec!["C:\\Users\\hi\\Docs\\budget.xlsx"]);
    assert!(store.accepts_children(slot, 12));
}

#[test]
fn a_move_under_an_excluded_or_unknown_parent_is_a_remove() {
    let (store, slot) = store_with({
        let mut nodes = sample();
        nodes.push(excluded(30, 11, "node_modules"));
        nodes.push(file(31, 30, "inside.js"));
        nodes
    });
    // The excluded folder is stored but never shown, and has no children.
    assert_eq!(store.live_count(slot), 7);
    assert!(find(&store, "node_modules").is_empty());
    assert!(find(&store, "inside").is_empty());
    assert!(!store.accepts_children(slot, 30));

    // A file that moves into the excluded folder is gone.
    store.apply(slot, &[upsert(file(21, 30, "Report-2.TXT"))]);
    assert!(find(&store, "report-2").is_empty());
    // A file that moves to an unknown folder is gone.
    store.apply(slot, &[upsert(file(22, 999, "budget.xlsx"))]);
    assert!(find(&store, "budget").is_empty());
    // A new file in an unknown folder is not stored.
    let count = entries(&store, slot);
    store.apply(slot, &[upsert(file(40, 999, "ghost.txt"))]);
    store.apply(slot, &[upsert(file(41, 30, "ghost2.txt"))]);
    assert_eq!(entries(&store, slot), count);
    assert!(find(&store, "ghost").is_empty());
    // A folder that moves into the excluded folder takes its files along.
    store.apply(slot, &[upsert(dir(12, 30, "Docs"))]);
    assert!(exact(&store, "report.txt").is_empty());
}

#[test]
fn a_folder_cannot_move_under_itself() {
    let (store, slot) = store_with(sample());
    // `Users` under its own descendant `Docs`: a stale or broken record.
    store.apply(slot, &[upsert(dir(10, 12, "Users"))]);
    assert!(find(&store, "report").is_empty());
    // The walk of the parent chains still ends.
    assert!(!store.accepts_children(slot, 10));
    assert!(store.accepts_children(slot, 5));
}

#[test]
fn a_reused_record_is_told_by_its_sequence_number() {
    let (store, slot) = store_with(vec![file(40, 5, "old.txt")]);
    let reused = FsNode { seq: 2, ..file(40, 5, "new.txt") };
    store.apply(slot, &[upsert(reused)]);
    assert!(find(&store, "old.txt").is_empty());
    assert_eq!(find(&store, "new.txt"), vec!["C:\\new.txt"]);
    // The remove of the first file arrives late.
    store.apply(slot, &[FsChange::Remove { record: 40, seq: 1 }]);
    assert_eq!(find(&store, "new.txt"), vec!["C:\\new.txt"]);
    store.apply(slot, &[FsChange::Remove { record: 40, seq: 2 }]);
    assert!(find(&store, "new.txt").is_empty());
    // The record number of the root and numbers past the limit are ignored.
    let count = entries(&store, slot);
    store.apply(
        slot,
        &[upsert(file(5, 5, "root.txt")), upsert(file(MAX_RECORD, 5, "huge.txt"))],
    );
    assert_eq!(entries(&store, slot), count);
}

#[test]
fn accepts_children_tells_live_folders() {
    let (store, slot) = store_with({
        let mut nodes = sample();
        nodes.push(excluded(30, 11, ".git"));
        nodes
    });
    assert!(store.accepts_children(slot, 5), "the root");
    assert!(store.accepts_children(slot, 12), "a folder");
    assert!(!store.accepts_children(slot, 20), "a file");
    assert!(!store.accepts_children(slot, 30), "an excluded folder");
    assert!(!store.accepts_children(slot, 999), "an unknown record");
    assert!(!store.accepts_children(slot + 1, 5), "an unknown volume");
    store.apply(slot, &[FsChange::Remove { record: 11, seq: 1 }]);
    assert!(!store.accepts_children(slot, 11), "a dead folder");
    assert!(!store.accepts_children(slot, 12), "below a dead folder");
}

#[test]
fn nodes_in_any_order_build_the_same_store() {
    let mut nodes = sample();
    let (ordered, _) = store_with(nodes.clone());
    nodes.reverse();
    let (reversed, slot) = store_with(nodes);
    for query in ["report", "budget", "docs", "hi", "txt"] {
        assert_eq!(sorted(find(&ordered, query)), sorted(find(&reversed, query)));
    }
    assert_eq!(reversed.live_count(slot), 6);
    assert_places_match_paths(&reversed);
    // Orphans and repeats are dropped.
    let (store, slot) = store_with(vec![
        file(20, 999, "orphan.txt"),
        file(21, 5, "a.txt"),
        file(21, 5, "again.txt"),
    ]);
    assert_eq!(store.live_count(slot), 1);
    assert_eq!(find(&store, "a.txt"), vec!["C:\\a.txt"]);
}

#[test]
fn location_penalties_come_from_the_chain() {
    let (store, slot) = store_with(vec![
        dir(10, 5, "Users"),
        dir(11, 10, "hi"),
        dir(12, 11, "app"),
        dir(13, 12, "node_modules"),
        dir(14, 13, "left-pad"),
        file(20, 14, "index.js"),
        file(21, 12, "index.js"),
        dir(15, 5, "Windows"),
        dir(16, 15, "System32"),
        file(22, 16, "index.js"),
        file(23, 11, "desktop.ini"),
    ]);
    assert_places_match_paths(&store);
    // The user's own file ranks first; Windows and node_modules follow.
    let found = find(&store, "index.js");
    assert_eq!(found[0], "C:\\Users\\hi\\app\\index.js");
    assert_eq!(found.len(), 3);
    // New entries get the same values from the chain.
    store.apply(slot, &[upsert(dir(17, 16, "drivers")), upsert(file(24, 17, "etc.txt"))]);
    assert_places_match_paths(&store);
    // A folder that moves gets its own values; the compaction that follows
    // computes its children's penalty again.
    store.apply(slot, &[upsert(dir(13, 11, "node_modules")), upsert(file(25, 11, "Thumbs.db"))]);
    store.apply(slot, &[FsChange::Remove { record: 25, seq: 1 }]);
    store.compact().expect("one tombstone");
    assert_places_match_paths(&store);
}

#[test]
fn compaction_drops_tombstones_and_keeps_the_results() {
    let (store, slot) = store_with({
        let mut nodes = vec![dir(10, 5, "a"), dir(11, 5, "b"), dir(12, 11, "deep")];
        for n in 0..200u64 {
            nodes.push(file(100 + n, 10, &format!("Report-{n:05}.txt")));
            nodes.push(file(400 + n, 11, &format!("Report-{n:05}.txt")));
            nodes.push(file(700 + n, 12, &format!("unique-{n}.md")));
        }
        nodes
    });
    // Single files, and a folder with its files (dead by ancestry).
    let mut changes = (0..30u64)
        .map(|n| FsChange::Remove { record: 100 + n * 3, seq: 1 })
        .collect::<Vec<_>>();
    changes.push(FsChange::Remove { record: 12, seq: 1 });
    store.apply(slot, &changes);

    let queries = ["report", "report-0001", "unique", "uniq 5", "txt", "rpt", ".md", "zzz", "deep"];
    let before = queries.map(|query| find(&store, query));
    let count = entries(&store, slot);
    assert!(find(&store, "unique").is_empty(), "the folder is dead");

    let compaction = store.compact().expect("compacted");
    assert_eq!(compaction.dropped, 30 + 1 + 200);
    assert_eq!(compaction.kept, count - 1 - compaction.dropped);
    assert!(compaction.locked < Duration::from_millis(500));
    assert_eq!(queries.map(|query| find(&store, query)), before);
    assert_eq!(entries(&store, slot), count - compaction.dropped);
    assert_places_match_paths(&store);
    // The record map follows the new numbers.
    store.apply(
        slot,
        &[upsert(file(101, 10, "Renamed.txt")), FsChange::Remove { record: 102, seq: 1 }],
    );
    assert_eq!(find(&store, "renamed"), vec!["C:\\a\\Renamed.txt"]);
    assert!(!store.accepts_children(slot, 12));
    // A file below the dropped folder is not stored.
    store.apply(slot, &[upsert(file(800, 12, "x.txt"))]);
    assert!(find(&store, "x.txt").is_empty());
    // The remove of record 102 left one tombstone; then nothing is left.
    assert!(store.compact().is_some());
    assert!(store.compact().is_none());
}

#[test]
fn changes_during_a_compaction_are_replayed() {
    let (store, slot) = store_with({
        let mut nodes = vec![dir(10, 5, "a"), dir(11, 5, "b")];
        for n in 0..50u64 {
            nodes.push(file(100 + n, 10, &format!("file-{n:03}.txt")));
        }
        nodes
    });
    store.apply(
        slot,
        &[FsChange::Remove { record: 100, seq: 1 }, FsChange::Remove { record: 101, seq: 1 }],
    );
    let begun = store.compact_begin(slot).expect("begun");
    let (fresh, dropped) = store.compact_build(slot, &begun).expect("built");
    assert_eq!(dropped, 2);
    // Changes that come before the swap: an insert, a remove of a copied
    // entry, a move, a new folder with a file, and a move below that folder.
    store.apply(
        slot,
        &[
            upsert(file(200, 10, "fresh.txt")),
            FsChange::Remove { record: 102, seq: 1 },
            upsert(file(103, 11, "moved.txt")),
            upsert(dir(12, 11, "newdir")),
            upsert(file(201, 12, "in-newdir.txt")),
            upsert(dir(11, 12, "b")),
        ],
    );
    // A folder cannot move below its own child: the last change is a remove.
    assert!(find(&store, "moved").is_empty());
    let (_, old) = store.compact_finish(slot, begun.id, fresh).expect("swapped");
    drop(old);
    assert_eq!(find(&store, "fresh"), vec!["C:\\a\\fresh.txt"]);
    assert!(find(&store, "file-002").is_empty());
    assert!(find(&store, "moved").is_empty());
    assert!(find(&store, "in-newdir").is_empty());
    assert_eq!(find(&store, "file-").len(), 46);
    assert_places_match_paths(&store);
    // The next compaction can start.
    store.apply(slot, &[FsChange::Remove { record: 104, seq: 1 }]);
    assert!(store.compact().is_some());
}

#[test]
fn a_move_during_a_compaction_lands_under_a_new_parent() {
    let (store, slot) = store_with({
        let mut nodes = vec![dir(10, 5, "a"), dir(11, 5, "b")];
        nodes.extend((0..20u64).map(|n| file(100 + n, 10, &format!("file-{n:03}.txt"))));
        nodes.extend((0..20u64).map(|n| file(300 + n, 11, &format!("other-{n:03}.txt"))));
        nodes
    });
    store.apply(slot, &[FsChange::Remove { record: 100, seq: 1 }]);
    let begun = store.compact_begin(slot).expect("begun");
    let (fresh, _) = store.compact_build(slot, &begun).expect("built");
    // A new folder, and the folder `b` (with its files) moves below it.
    store.apply(slot, &[upsert(dir(12, 10, "newdir")), upsert(dir(11, 12, "b"))]);
    let (_, old) = store.compact_finish(slot, begun.id, fresh).expect("swapped");
    drop(old);
    assert_eq!(find(&store, "other-005"), vec!["C:\\a\\newdir\\b\\other-005.txt"]);
    assert_eq!(find(&store, "other-").len(), 20);
    // A later compaction computes the penalty of the moved files again.
    store.apply(slot, &[FsChange::Remove { record: 101, seq: 1 }]);
    store.compact().expect("one tombstone");
    assert_places_match_paths(&store);
}

#[test]
fn a_replaced_volume_makes_a_running_compaction_obsolete() {
    let (store, slot) = store_with(sample());
    store.apply(slot, &[FsChange::Remove { record: 21, seq: 1 }]);
    let begun = store.compact_begin(slot).expect("begun");
    let (fresh, _) = store.compact_build(slot, &begun).expect("built");
    store.replace_volume(meta("vol-c", "C:\\"), vec![file(50, 5, "after.txt")]);
    assert!(store.compact_finish(slot, begun.id, fresh).is_none());
    assert_eq!(find(&store, "after"), vec!["C:\\after.txt"]);
    assert!(find(&store, "report").is_empty());
}

#[test]
fn many_tombstones_compact_inside_apply() {
    let (store, slot) = store_with({
        let mut nodes = vec![dir(10, 5, "build")];
        nodes.extend((0..12_000u64).map(|n| file(100 + n, 10, &format!("out-{n}.o"))));
        nodes.push(file(20_000, 5, "keep.txt"));
        nodes
    });
    let removals = (0..12_000u64)
        .map(|n| FsChange::Remove { record: 100 + n, seq: 1 })
        .collect::<Vec<_>>();
    store.apply(slot, &removals);
    // The compaction ran: the arrays hold the root, `build` and `keep.txt`.
    assert_eq!(entries(&store, slot), 3);
    assert_eq!(find(&store, "keep"), vec!["C:\\keep.txt"]);
}

#[test]
fn searches_that_meet_hidden_entries_ask_for_a_compaction() {
    let (store, slot) = store_with({
        let mut nodes = vec![dir(10, 5, "gone")];
        nodes.extend((0..12_000u64).map(|n| file(100 + n, 10, &format!("item-{n}.dat"))));
        nodes
    });
    // One tombstone: the remove alone does not compact.
    store.apply(slot, &[FsChange::Remove { record: 10, seq: 1 }]);
    assert_eq!(entries(&store, slot), 12_002);
    assert!(find(&store, "item").is_empty());
    // The search met 12,000 hidden entries: the next batch compacts.
    store.apply(slot, &[]);
    assert_eq!(entries(&store, slot), 1);
}

#[test]
fn ranking_prefers_exact_then_prefix_then_substring() {
    let (store, _) = store_with(vec![
        dir(10, 5, "docs"),
        file(20, 10, "my-readme-notes.txt"),
        file(21, 10, "readme.txt"),
        file(22, 10, "readme"),
        file(23, 10, "README.md"),
        file(24, 10, "unrelated.bin"),
    ]);
    assert_eq!(
        find(&store, "readme"),
        vec![
            "C:\\docs\\readme",
            "C:\\docs\\README.md",
            "C:\\docs\\readme.txt",
            "C:\\docs\\my-readme-notes.txt",
        ]
    );
    // A directory gets the folder bonus over a same-named file.
    let (store, _) = store_with(vec![dir(10, 5, "notes"), dir(11, 5, "x"), file(20, 11, "notes")]);
    assert_eq!(find(&store, "notes"), vec!["C:\\notes", "C:\\x\\notes"]);
}

#[test]
fn the_typo_pass_finds_a_misspelled_name_when_no_name_is_strong() {
    let (store, _) = store_with(vec![
        dir(10, 5, "Apps"),
        file(20, 10, "chrome.exe"),
        file(21, 10, "chip-remove.txt"),
        file(22, 10, "cargo.toml"),
    ]);
    let tokens = ["chrmoe"];
    let query = NameQuery::new("chrmoe", &tokens);
    let found = store.candidates(&query, 10, true, &|| false);
    assert_eq!(found.typos.len(), 1);
    assert_eq!(found.typos[0].0.display_path, "C:\\Apps\\chrome.exe");
    // "chip-remove" holds the letters in order: a scattered match, far
    // below the typo hit.
    assert_eq!(found.entries.len(), 1);
    assert_eq!(found.entries[0].display_path, "C:\\Apps\\chip-remove.txt");
    // Typos off: the scattered match only.
    let found = store.candidates(&query, 10, false, &|| false);
    assert!(found.typos.is_empty());
    assert_eq!(found.entries.len(), 1);
}

#[test]
fn two_volumes_search_together() {
    let store = FileStore::new();
    let c = store.replace_volume(meta("vol-c", "C:\\"), vec![file(10, 5, "same.txt")]);
    let d = store.replace_volume(meta("vol-d", "D:\\"), vec![dir(10, 5, "x"), file(11, 10, "same.txt")]);
    assert_ne!(c, d);
    assert_eq!(store.volume_slot("vol-d"), Some(d));
    assert_eq!(store.volume_slot("vol-z"), None);
    let tokens = ["same"];
    let query = NameQuery::new("same", &tokens);
    let found = store.candidates(&query, 10, false, &|| false);
    let ids = found
        .entries
        .iter()
        .map(|entry| entry.id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 2);
    assert_eq!(sorted(find(&store, "same")), vec!["C:\\same.txt", "D:\\x\\same.txt"]);
    // A change goes to its volume only.
    store.apply(d, &[FsChange::Remove { record: 11, seq: 1 }]);
    assert_eq!(find(&store, "same"), vec!["C:\\same.txt"]);
    // Removing and adding a volume.
    store.remove_volume("vol-c");
    assert!(find(&store, "same").is_empty());
    assert_eq!(store.volumes().len(), 1);
    let again = store.replace_volume(meta("vol-e", "E:\\"), vec![file(10, 5, "same.txt")]);
    assert_eq!(again, c, "the free slot is used");
}

#[test]
fn checkpoints_and_volume_metadata() {
    let (store, slot) = store_with(sample());
    assert_eq!(store.volumes(), vec![meta("vol-c", "C:\\")]);
    store.set_checkpoint(slot, 9, 12345);
    let volumes = store.volumes();
    assert_eq!((volumes[0].journal_id, volumes[0].next_usn), (9, 12345));
    store.apply_and_checkpoint(slot, &[upsert(file(60, 5, "z.txt"))], 9, 20000);
    assert_eq!(store.volumes()[0].next_usn, 20000);
    assert_eq!(find(&store, "z.txt").len(), 1);
    assert!(store.is_dirty());
}

#[test]
fn a_batch_with_no_effect_leaves_the_store_clean() {
    let temp = TempDir::new();
    let path = temp.0.join("file-index.bin");
    let (store, slot) = store_with(sample());
    store.save(&path).expect("save");
    // Removes of unknown ids (records of an excluded folder), a stale remove
    // (another sequence number), and an upsert that changes nothing.
    let changes = [
        FsChange::Remove { record: 900, seq: 1 },
        FsChange::Remove { record: 901, seq: 3 },
        FsChange::Remove { record: 20, seq: 9 },
        upsert(file(20, 12, "report.txt")),
        upsert(node(902, 999, "orphan.txt", false)),
    ];
    store.apply_and_checkpoint(slot, &changes, 7, 150);
    assert!(!store.is_dirty());
    // The checkpoint still moves, so the next save keeps it.
    assert_eq!(store.volumes()[0].next_usn, 150);
    store.advance_checkpoint(slot, 7, 160);
    assert!(!store.is_dirty());
    // A real change sets the flag.
    store.apply(slot, &[FsChange::Remove { record: 20, seq: 1 }]);
    assert!(store.is_dirty());
    // A remove of an entry that is already dead has no effect.
    store.save(&path).expect("save");
    store.apply(slot, &[FsChange::Remove { record: 20, seq: 1 }]);
    assert!(!store.is_dirty());
}

#[test]
fn a_cancelled_search_returns_nothing() {
    let (store, _) = store_with(sample());
    let tokens = ["report"];
    let query = NameQuery::new("report", &tokens);
    let found = store.candidates(&query, 10, true, &|| true);
    assert!(found.entries.is_empty() && found.typos.is_empty());
}

fn tmp_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

#[test]
fn the_snapshot_round_trips() {
    let temp = TempDir::new();
    let path = temp.0.join("file-index.bin");
    let store = FileStore::new();
    let c = store.replace_volume(meta("vol-c", "C:\\"), {
        let mut nodes = sample();
        nodes.push(excluded(30, 11, "node_modules"));
        nodes.push(file(31, 11, "Ünïcode-Näme.TXT"));
        nodes
    });
    store.replace_volume(
        VolumeMeta { backend: VolumeBackend::Walk, ..meta("vol-d", "D:\\") },
        vec![dir(10, 5, "x"), file(11, 10, "report.txt")],
    );
    store.apply(c, &[FsChange::Remove { record: 22, seq: 1 }]);
    store.set_checkpoint(c, 77, 4242);

    let size = store.save(&path).expect("save");
    assert_eq!(size, std::fs::metadata(&path).unwrap().len());
    assert!(!store.is_dirty());
    assert!(!tmp_of(&path).exists(), "the temporary file is renamed");

    let loaded = FileStore::load(&path).expect("load");
    assert_eq!(loaded.volumes(), store.volumes());
    assert_eq!(loaded.volume_slot("vol-d"), store.volume_slot("vol-d"));
    for query in ["report", "budget", "node_modules", "ünïcode", "x", "docs", "txt"] {
        assert_eq!(find(&loaded, query), find(&store, query), "query {query}");
    }
    assert_eq!(loaded.live_count(c), store.live_count(c));
    assert!(!loaded.accepts_children(c, 30));
    assert!(loaded.accepts_children(c, 12));
    // The loaded store takes changes and compacts.
    loaded.apply(
        c,
        &[upsert(file(70, 12, "after-load.txt")), FsChange::Remove { record: 20, seq: 1 }],
    );
    assert_eq!(find(&loaded, "after-load"), vec!["C:\\Users\\hi\\Docs\\after-load.txt"]);
    assert_eq!(exact(&loaded, "report.txt"), vec!["D:\\x\\report.txt"]);
    loaded.compact();
    assert_eq!(find(&loaded, "after-load").len(), 1);
    assert_places_match_paths(&loaded);

    // A second save over the first.
    loaded.save(&path).expect("save again");
    assert_eq!(find(&FileStore::load(&path).unwrap(), "after-load").len(), 1);
}

#[test]
fn an_empty_store_round_trips() {
    let temp = TempDir::new();
    let path = temp.0.join("empty.bin");
    FileStore::new().save(&path).expect("save");
    let loaded = FileStore::load(&path).expect("load");
    assert!(loaded.volumes().is_empty());
}

#[test]
fn a_changed_byte_or_a_short_file_does_not_load() {
    let temp = TempDir::new();
    let path = temp.0.join("file-index.bin");
    let (store, _) = store_with({
        let mut nodes = sample();
        nodes.push(excluded(30, 11, ".git"));
        nodes
    });
    store.save(&path).expect("save");
    let good = std::fs::read(&path).unwrap();
    FileStore::load(&path).expect("the good file loads");

    let bad = temp.0.join("bad.bin");
    // Every single byte, changed: a bad checksum, length or header.
    for position in 0..good.len() {
        let mut bytes = good.clone();
        bytes[position] ^= 0x5a;
        std::fs::write(&bad, &bytes).unwrap();
        assert!(FileStore::load(&bad).is_err(), "byte {position} changed");
    }
    // Every shorter file.
    for length in 0..good.len() {
        std::fs::write(&bad, &good[..length]).unwrap();
        assert!(FileStore::load(&bad).is_err(), "cut at {length}");
    }
    // Bytes after the end.
    let mut longer = good.clone();
    longer.push(0);
    std::fs::write(&bad, &longer).unwrap();
    assert!(FileStore::load(&bad).is_err());
    // A file that is not an index, and a missing file.
    std::fs::write(&bad, b"hello").unwrap();
    assert!(FileStore::load(&bad).is_err());
    assert!(FileStore::load(&temp.0.join("missing.bin")).is_err());
}

#[test]
fn a_bad_index_with_good_checksums_does_not_load() {
    // The checksums protect against damage; the load checks the indexes too,
    // so a writer bug cannot crash a search.
    let temp = TempDir::new();
    let path = temp.0.join("file-index.bin");
    let (store, _) = store_with(sample());
    store.save(&path).expect("save");
    let mut segments = super::super::snapshot::read(&path).expect("read");
    let mut segment = segments.pop().unwrap();
    segment.parent[3] = 9_999;
    super::super::snapshot::write(&path, &[&segment]).expect("write");
    assert!(FileStore::load(&path).is_err());
}

#[test]
fn places_stop_at_the_limit_and_follow_a_folder_rename() {
    let name = "x".repeat(250);
    let mut nodes = Vec::new();
    let mut parent = 5;
    for depth in 0..300u64 {
        nodes.push(dir(10 + depth, parent, &name));
        parent = 10 + depth;
    }
    nodes.push(dir(400, 5, "top"));
    nodes.push(file(401, 400, "f.txt"));
    // 300 folders of 251 bytes are more than 0xffff bytes.
    let (store, slot) = store_with(nodes);
    assert_places_match_paths(&store);
    // A folder rename changes the places of its children at once.
    store.apply(slot, &[upsert(dir(400, 5, "a-much-longer-name"))]);
    assert_places_match_paths(&store);
}

#[test]
fn a_version_2_snapshot_still_loads() {
    let temp = TempDir::new();
    let path = temp.0.join("file-index.bin");
    let (store, slot) = store_with({
        let mut nodes = sample();
        nodes.push(file(31, 11, "Ünïcode-Näme.TXT"));
        nodes
    });
    store.apply(slot, &[FsChange::Remove { record: 22, seq: 1 }]);
    let segments = store.read();
    let segments: Vec<&Segment> = segments.segments().collect();
    super::super::snapshot::write_version_2(&path, &segments).expect("write version 2");
    drop(segments);
    // The old layout has the places section, so it is longer.
    let current = temp.0.join("current.bin");
    store.save(&current).expect("save");
    assert!(std::fs::metadata(&path).unwrap().len() > std::fs::metadata(&current).unwrap().len());

    let loaded = FileStore::load(&path).expect("load version 2");
    for query in ["report", "budget", "ünïcode", "docs", "txt"] {
        assert_eq!(find(&loaded, query), find(&store, query), "query {query}");
    }
    assert_eq!(loaded.live_count(slot), store.live_count(slot));
    assert_places_match_paths(&loaded);
    // The next save writes the new version, and it loads again.
    let saved = temp.0.join("saved.bin");
    loaded.save(&saved).expect("save");
    assert_eq!(std::fs::read(&saved).unwrap(), std::fs::read(&current).unwrap());
    assert_eq!(find(&FileStore::load(&saved).unwrap(), "report"), find(&store, "report"));
}

/// The display path of every live entry, sorted.
fn all_paths(store: &FileStore) -> Vec<String> {
    let inner = store.read();
    let mut paths = Vec::new();
    for segment in inner.segments() {
        for entry in 1..segment.entries() {
            if segment.flags[entry] & FLAG_DEAD == 0 && segment.parent_alive(entry) {
                paths.push(segment.path_of(entry as u32));
            }
        }
    }
    paths.sort();
    paths
}

/// Names that differ only in case, in several folders, and some that do not.
fn case_nodes() -> Vec<FsNode> {
    vec![
        dir(10, 5, "Docs"),
        dir(11, 5, "docs"),
        dir(12, 5, "DOCS"),
        file(20, 10, "Readme.md"),
        file(21, 11, "readme.md"),
        file(22, 12, "README.MD"),
        file(23, 12, "README.MD"),
        file(24, 10, "Ünïcode-Näme.TXT"),
        file(25, 11, "plain.txt"),
    ]
}

#[test]
fn names_that_differ_only_in_case_stay_distinct() {
    let (store, _) = store_with(case_nodes());
    assert_eq!(
        sorted(find(&store, "readme")),
        vec!["C:\\DOCS\\README.MD", "C:\\DOCS\\README.MD", "C:\\Docs\\Readme.md", "C:\\docs\\readme.md"]
    );
    assert_eq!(
        all_paths(&store),
        vec![
            "C:\\DOCS",
            "C:\\DOCS\\README.MD",
            "C:\\DOCS\\README.MD",
            "C:\\Docs",
            "C:\\Docs\\Readme.md",
            "C:\\Docs\\Ünïcode-Näme.TXT",
            "C:\\docs",
            "C:\\docs\\plain.txt",
            "C:\\docs\\readme.md",
        ]
    );
    assert_places_match_paths(&store);
}

#[test]
fn a_rename_between_plain_and_variant_changes_only_the_exact_name() {
    let (store, slot) = store_with(case_nodes());
    store.apply(
        slot,
        &[
            upsert(file(20, 10, "readme.md")), // variant -> plain
            upsert(file(21, 11, "ReadMe.md")), // plain -> a new variant
            upsert(file(22, 12, "Readme.md")), // variant -> an old variant
        ],
    );
    assert_eq!(
        sorted(find(&store, "readme")),
        vec!["C:\\DOCS\\README.MD", "C:\\DOCS\\Readme.md", "C:\\Docs\\readme.md", "C:\\docs\\ReadMe.md"]
    );
    assert_places_match_paths(&store);
}

#[test]
fn compaction_keeps_the_exact_names_of_the_entries_it_keeps() {
    let (store, slot) = store_with(case_nodes());
    let before_paths = all_paths(&store);
    // Drop the only entries that use the variants "Docs", "Readme.md" and
    // "Ünïcode-Näme.TXT": the folder is dead, and so are its files.
    store.apply(slot, &[FsChange::Remove { record: 10, seq: 1 }]);
    let compaction = store.compact().expect("compacted");
    assert_eq!(compaction.dropped, 3);
    let after_paths = all_paths(&store);
    assert_eq!(
        after_paths,
        before_paths
            .iter()
            .filter(|path| !path.starts_with("C:\\Docs"))
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        sorted(find(&store, "readme")),
        vec!["C:\\DOCS\\README.MD", "C:\\DOCS\\README.MD", "C:\\docs\\readme.md"]
    );
    assert_places_match_paths(&store);
    // A new variant after the compaction is found and kept apart.
    store.apply(slot, &[upsert(file(30, 11, "ReadMe.MD"))]);
    assert!(find(&store, "readme").contains(&"C:\\docs\\ReadMe.MD".to_string()));
    assert_places_match_paths(&store);
}

#[test]
fn the_snapshot_keeps_case_variants() {
    let temp = TempDir::new();
    let path = temp.0.join("file-index.bin");
    let (store, slot) = store_with(case_nodes());
    store.save(&path).expect("save");
    let loaded = FileStore::load(&path).expect("load");
    assert_eq!(all_paths(&loaded), all_paths(&store));
    assert_eq!(find(&loaded, "readme"), find(&store, "readme"));
    // The loaded store takes an old variant again, and a new one.
    loaded.apply(slot, &[upsert(file(40, 11, "Readme.md")), upsert(file(41, 11, "ReadMe.md"))]);
    let found = find(&loaded, "readme");
    assert!(found.contains(&"C:\\docs\\Readme.md".to_string()), "{found:?}");
    assert!(found.contains(&"C:\\docs\\ReadMe.md".to_string()), "{found:?}");
    assert_places_match_paths(&loaded);
    // A variant number past the table does not load.
    let mut segments = super::super::snapshot::read(&path).expect("read");
    let mut segment = segments.pop().unwrap();
    segment.display[3] = VARIANT_TAG | 99;
    super::super::snapshot::write(&path, &[&segment]).expect("write");
    assert!(FileStore::load(&path).is_err());
}

#[test]
fn a_version_3_snapshot_loads_and_converts_its_display_table() {
    let temp = TempDir::new();
    let path = temp.0.join("file-index.bin");
    let (store, slot) = store_with(case_nodes());
    store.apply(slot, &[FsChange::Remove { record: 25, seq: 1 }]);
    let segments = store.read();
    let segments: Vec<&Segment> = segments.segments().collect();
    super::super::snapshot::write_version_3(&path, &segments).expect("write version 3");
    drop(segments);
    // The old layout has one display name for each exact-case name, so its
    // bytes differ from the current layout.
    let current = temp.0.join("current.bin");
    store.save(&current).expect("save");
    assert_ne!(std::fs::read(&path).unwrap(), std::fs::read(&current).unwrap());

    let loaded = FileStore::load(&path).expect("load version 3");
    assert_eq!(all_paths(&loaded), all_paths(&store));
    for query in ["readme", "docs", "ünïcode", "plain", "txt"] {
        assert_eq!(find(&loaded, query), find(&store, query), "query {query}");
    }
    assert_eq!(loaded.live_count(slot), store.live_count(slot));
    assert_places_match_paths(&loaded);
    // The next save writes version 4, equal to a save of the first store.
    let saved = temp.0.join("saved.bin");
    loaded.save(&saved).expect("save");
    assert_eq!(std::fs::read(&saved).unwrap(), std::fs::read(&current).unwrap());
}

#[test]
fn a_bad_display_table_in_an_old_snapshot_does_not_load() {
    let temp = TempDir::new();
    let path = temp.0.join("file-index.bin");
    let (store, _) = store_with(case_nodes());
    let segments = store.read();
    let segments: Vec<&Segment> = segments.segments().collect();
    super::super::snapshot::write_version_3(&path, &segments).expect("write version 3");
    drop(segments);
    FileStore::load(&path).expect("the good file loads");
    // An entry's display number past the old table. The sections get good
    // checksums when written again, so only the index test catches it.
    let mut segments = super::super::snapshot::read(&path).expect("read");
    let mut segment = segments.pop().unwrap();
    segment.display[2] = 9_999;
    super::super::snapshot::write_version_3(&path, &[&segment]).expect("write");
    assert!(FileStore::load(&path).is_err());
}
