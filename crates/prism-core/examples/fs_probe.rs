//! Probe for the catalog v2 file-system sources (`catalog::walk`, `catalog::journal`).
//!
//! RUN THIS ONLY IN THE AGENT-DESKTOP SANDBOX. It reads the live journal and the
//! real folders of the machine it runs on.
//!
//! ```text
//! fs_probe walk <root> [threads]          walk <root>, print the stats and the first 20 nodes
//! fs_probe journal <X:\> <seconds>        read the journal from NextUsn for N seconds, print the changes
//! fs_probe verify <root> <seconds>        walk, apply journal changes for N seconds, walk again, diff
//! ```
//!
//! `verify`: after the first walk the caller makes file churn (create, rename a
//! folder, move to the recycle bin, delete a tree, a burst of files) below
//! `<root>`. The churn must stop before the N seconds end. The probe then reads
//! the rest of the journal, walks again, and prints the differences between
//! the model that the journal built and the second walk. Zero differences is
//! the pass. The model is a plain `HashMap`; it needs no store.
//!
//! Exit codes: 0 pass or done, 1 error, 2 verify found differences.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use prism_core::catalog::journal::JournalVolume;
use prism_core::catalog::store::{FsChange, FsNode};
use prism_core::catalog::walk::{
    root_record, walk_volume, walk_volume_threads, Exclusions, WalkStats,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("walk") if args.len() >= 2 => walk(&args[1], args.get(2)),
        Some("journal") if args.len() == 3 => journal(&args[1], &args[2]),
        Some("verify") if args.len() == 3 => verify(&args[1], &args[2]),
        _ => Err(
            "usage: fs_probe walk <root> [threads] | journal <X:\\> <seconds> | verify <root> <seconds>"
                .to_string(),
        ),
    };
    match result {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    }
}

/// The exclusions the app uses by default, with the app data folder of Prism.
fn exclusions() -> Exclusions {
    let data = std::env::var("APPDATA").unwrap_or_default();
    Exclusions::default_list(&Path::new(&data).join("app.prism.launcher"))
}

fn walk(root: &str, threads: Option<&String>) -> Result<i32, String> {
    let exclusions = exclusions();
    let cancel = AtomicBool::new(false);
    let mut first = Vec::new();
    let mut excluded = 0u64;
    let mut sink = |node: FsNode| {
        if node.excluded {
            excluded += 1;
        }
        if first.len() < 20 {
            first.push(node);
        }
    };
    let stats = match threads {
        Some(text) => {
            let count: usize = text.parse().map_err(|_| format!("bad thread count: {text}"))?;
            walk_volume_threads(Path::new(root), &exclusions, &cancel, count, false, &mut sink)?
        }
        None => walk_volume(Path::new(root), &exclusions, &cancel, &mut sink)?,
    };
    print_stats(&stats);
    println!("excluded folders: {excluded}");
    println!("first {} nodes:", first.len());
    for node in &first {
        print_node(node);
    }
    Ok(0)
}

fn print_stats(stats: &WalkStats) {
    let rate = (stats.entries * 1000).checked_div(stats.ms).unwrap_or(0);
    println!(
        "walk: entries={} folders={} denied={} ms={} ({} entries/s)",
        stats.entries, stats.folders, stats.denied, stats.ms, rate
    );
}

fn print_node(node: &FsNode) {
    println!(
        "  record={} seq={} parent={} {}{} {:?}",
        node.record,
        node.seq,
        node.parent_record,
        if node.is_dir { "dir" } else { "file" },
        if node.excluded { " EXCLUDED" } else { "" },
        node.name
    );
}

fn journal(volume_root: &str, seconds: &str) -> Result<i32, String> {
    let seconds: u64 = seconds.parse().map_err(|_| format!("bad seconds: {seconds}"))?;
    let exclusions = exclusions();
    let volume = JournalVolume::open(Path::new(volume_root))?;
    let metadata = volume.query()?;
    println!("journal: {metadata:?}");
    let mut next = metadata.next_usn;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let (mut reads, mut records, mut upserts, mut removes) = (0u32, 0u64, 0u64, 0u64);
    while Instant::now() < deadline {
        let call = Instant::now();
        let batch = { std::thread::sleep(Duration::from_secs(1)); volume.read(next, metadata.journal_id)? };
        reads += 1;
        if reads <= 5 {
            // A blocking read returns after about 1 s when the journal is quiet.
            println!("read #{reads}: {} records in {:?}", batch.records.len(), call.elapsed());
        }
        records += batch.records.len() as u64;
        if !batch.records.is_empty() {
            let folded = volume.changes(&batch, &|_| true, &exclusions);
            println!(
                "[{:.2}s] {} records, usn {next} -> {}, {} changes",
                started.elapsed().as_secs_f64(),
                batch.records.len(),
                batch.next_usn,
                folded.len()
            );
            for change in &folded {
                match change {
                    FsChange::Upsert(node) => {
                        upserts += 1;
                        print_node(node);
                    }
                    FsChange::Remove { record, seq } => {
                        removes += 1;
                        println!("  remove record={record} seq={seq}");
                    }
                }
            }
        }
        next = batch.next_usn;
    }
    println!("done: reads={reads} records={records} upserts={upserts} removes={removes} next_usn={next}");
    Ok(0)
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    seq: u16,
    parent: u64,
    name: String,
    is_dir: bool,
    excluded: bool,
}

type Model = HashMap<u64, Entry>;

fn walk_model(root: &Path, exclusions: &Exclusions) -> Result<(Model, WalkStats), String> {
    let cancel = AtomicBool::new(false);
    let mut model = Model::new();
    let stats = walk_volume(root, exclusions, &cancel, &mut |node| {
        model.insert(
            node.record,
            Entry {
                seq: node.seq,
                parent: node.parent_record,
                name: node.name,
                is_dir: node.is_dir,
                excluded: node.excluded,
            },
        );
    })?;
    Ok((model, stats))
}

fn apply(model: &mut Model, changes: Vec<FsChange>) {
    for change in changes {
        match change {
            FsChange::Upsert(node) => {
                model.insert(
                    node.record,
                    Entry {
                        seq: node.seq,
                        parent: node.parent_record,
                        name: node.name,
                        is_dir: node.is_dir,
                        excluded: node.excluded,
                    },
                );
            }
            FsChange::Remove { record, seq } => {
                if model.get(&record).is_some_and(|entry| entry.seq == seq) {
                    model.remove(&record);
                }
            }
        }
    }
}

/// The entries that a walk from `root_rec` would reach: an entry below a
/// removed or excluded folder is dropped (the ancestry rule of the store).
fn reachable(model: &Model, root_rec: u64) -> Model {
    let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
    for (record, entry) in model {
        children.entry(entry.parent).or_default().push(*record);
    }
    let mut result = Model::new();
    let mut stack = vec![root_rec];
    while let Some(parent) = stack.pop() {
        for record in children.get(&parent).into_iter().flatten() {
            let entry = &model[record];
            // The model of a walk has no cycle; the check guards a bad journal result.
            if result.insert(*record, entry.clone()).is_some() {
                continue;
            }
            if entry.is_dir && !entry.excluded {
                stack.push(*record);
            }
        }
    }
    result
}

fn path_of(model: &Model, root_rec: u64, root: &str, record: u64) -> String {
    let mut parts = Vec::new();
    let mut at = record;
    while at != root_rec {
        let Some(entry) = model.get(&at) else {
            parts.push(format!("<missing {at}>"));
            break;
        };
        parts.push(entry.name.clone());
        at = entry.parent;
        if parts.len() > 512 {
            break;
        }
    }
    parts.reverse();
    format!("{}\\{}", root.trim_end_matches('\\'), parts.join("\\"))
}

/// `X:\` of a path that starts with a drive letter.
fn volume_root_of(root: &str) -> Result<PathBuf, String> {
    let bytes = root.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Ok(PathBuf::from(format!("{}:\\", bytes[0] as char)))
    } else {
        Err(format!("verify needs a path with a drive letter: {root}"))
    }
}

fn verify(root: &str, seconds: &str) -> Result<i32, String> {
    let seconds: u64 = seconds.parse().map_err(|_| format!("bad seconds: {seconds}"))?;
    let exclusions = exclusions();
    let root_path = Path::new(root);
    let volume = JournalVolume::open(&volume_root_of(root)?)?;
    let metadata = volume.query()?;
    let root_rec = root_record(root_path)?;

    // The journal position is taken BEFORE the walk: changes during the walk are replayed.
    let mut next = metadata.next_usn;
    let (mut model, stats) = walk_model(root_path, &exclusions)?;
    print!("first ");
    print_stats(&stats);
    println!("model: {} entries; journal {metadata:?}", model.len());
    println!("make file churn below {root} now; reading the journal for {seconds} s ...");

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let (mut records, mut applied) = (0u64, 0u64);
    let mut read_batch = |model: &mut Model, wait: bool, next: &mut i64| -> Result<usize, String> {
        let batch = if wait {
            { std::thread::sleep(Duration::from_secs(1)); volume.read(*next, metadata.journal_id)? }
        } else {
            volume.read(*next, metadata.journal_id)?
        };
        records += batch.records.len() as u64;
        let changes = {
            let accepts = |record: u64| {
                record == root_rec
                    || model.get(&record).is_some_and(|entry| entry.is_dir && !entry.excluded)
            };
            volume.changes(&batch, &accepts, &exclusions)
        };
        applied += changes.len() as u64;
        apply(model, changes);
        *next = batch.next_usn;
        Ok(batch.records.len())
    };
    while Instant::now() < deadline {
        read_batch(&mut model, true, &mut next)?;
    }
    // Settle: read until the journal gives nothing more.
    for _ in 0..100 {
        if read_batch(&mut model, false, &mut next)? == 0 {
            break;
        }
    }
    println!("journal: {records} records read, {applied} changes applied");

    let (second, stats) = walk_model(root_path, &exclusions)?;
    print!("second ");
    print_stats(&stats);
    let model = reachable(&model, root_rec);
    println!("reachable model: {} entries; second walk: {} entries", model.len(), second.len());

    let mut differences = Vec::new();
    for (record, entry) in &model {
        match second.get(record) {
            None => differences.push((
                "STALE (model has it, walk does not)",
                path_of(&model, root_rec, root, *record),
                format!("{entry:?}"),
            )),
            Some(other) if other != entry => differences.push((
                "CHANGED",
                path_of(&second, root_rec, root, *record),
                format!("model {entry:?} | walk {other:?}"),
            )),
            Some(_) => {}
        }
    }
    for (record, entry) in &second {
        if !model.contains_key(record) {
            differences.push((
                "MISSING (walk has it, model does not)",
                path_of(&second, root_rec, root, *record),
                format!("{entry:?}"),
            ));
        }
    }
    differences.sort();
    println!("differences: {}", differences.len());
    for (kind, path, detail) in differences.iter().take(50) {
        println!("  {kind}: {path}  {detail}");
    }
    Ok(if differences.is_empty() { 0 } else { 2 })
}
