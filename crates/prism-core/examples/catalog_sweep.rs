//! Headless sweep benchmark: reconciles one directory tree into a catalog copy.
//!
//! `cargo run --release --example catalog_sweep -- <copy of catalog.db> <root, for example C:\Users\me\Desktop> [--verify]`
//!
//! The walk reads the real disk (directory enumeration only) and writes only
//! to the catalog copy. The first run applies what changed since the copy was
//! taken; the runs after it measure an idle sweep. The last line is a JSON
//! summary: wall time, kernel and user CPU time of the process, entries walked, and the
//! bytes the process read and wrote. With `--verify`, the catalog rows under
//! the root are then compared with a fresh walk of the disk (path, folder
//! flag, write time, size, and parent), and the differences are printed.
//! With `--rename <old> <new>`, it applies a folder rename that already
//! happened on disk as the watcher does (`Database::rename_file`), instead
//! of the sweep, and times it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use prism_core::catalog::db::Database;
use prism_core::catalog::scanner::reconcile_scope;
use prism_core::catalog::types::ScannedItem;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS,
};
use windows::Win32::Foundation::FILETIME;

/// Kernel and user CPU time of the process, in milliseconds.
fn cpu_ms() -> (f64, f64) {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the pseudo handle of the current process and valid out pointers.
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user);
    }
    let ticks = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime)) as f64;
    (ticks(kernel) / 10_000.0, ticks(user) / 10_000.0)
}

fn io() -> IO_COUNTERS {
    let mut counters = IO_COUNTERS::default();
    // SAFETY: the pseudo handle of the current process and a valid out pointer.
    unsafe {
        let _ = GetProcessIoCounters(GetCurrentProcess(), &mut counters);
    }
    counters
}

/// One entry as the catalog stores it: folder flag, write time, size, and
/// the lowercase parent folder. A folder's write time is 0: the sweep reads
/// it from the parent's directory index, which Windows updates late.
type Entry = (bool, i64, i64, String);

/// Every entry under `root`, keyed by its lowercase path. Reparse points are
/// leaves, as in the sweep.
fn walk(root: &Path, entries: &mut BTreeMap<String, Entry>) {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let Ok(children) = std::fs::read_dir(root) else {
        return;
    };
    let parent = root.to_string_lossy().to_lowercase();
    for child in children.flatten() {
        let Ok(metadata) = child.metadata() else {
            continue;
        };
        let path = child.path();
        let directory = metadata.is_dir();
        let size = if directory { 0 } else { metadata.file_size() as i64 };
        entries.insert(
            path.to_string_lossy().to_lowercase(),
            (
                directory,
                if directory { 0 } else { metadata.last_write_time() as i64 },
                size,
                parent.clone(),
            ),
        );
        if directory && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            walk(&path, entries);
        }
    }
}

/// Prints each difference between the catalog and the disk under `root`
/// and returns their count.
fn verify(db_path: &str, root: &Path) -> usize {
    let mut disk = BTreeMap::new();
    walk(root, &mut disk);
    let base = root.to_string_lossy().to_lowercase();
    let base = base.trim_end_matches('\\');
    let (low, high) = (format!("{base}\\"), format!("{base}]"));
    let conn = rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open catalog");
    let mut statement = conn
        .prepare(
            "SELECT normalized_path, is_directory, modified_at, size, parent FROM files
             WHERE normalized_path >= ?1 AND normalized_path < ?2;",
        )
        .expect("prepare");
    let catalog = statement
        .query_map([&low, &high], |row| {
            let directory = row.get::<_, i64>(1)? != 0;
            Ok((
                row.get::<_, String>(0)?,
                (
                    directory,
                    if directory { 0 } else { row.get::<_, i64>(2)? },
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?.to_lowercase(),
                ),
            ))
        })
        .expect("query")
        .collect::<Result<BTreeMap<String, Entry>, _>>()
        .expect("rows");
    let mut differences = 0;
    for (path, entry) in &disk {
        match catalog.get(path) {
            None => println!("MISSING {path}"),
            Some(stored) if stored != entry => {
                println!("STALE {path}: catalog {stored:?}, disk {entry:?}")
            }
            Some(_) => continue,
        }
        differences += 1;
    }
    for path in catalog.keys().filter(|path| !disk.contains_key(*path)) {
        println!("EXTRA {path}");
        differences += 1;
    }
    println!(
        "VERIFY {} entries on disk, {} rows, {differences} differences",
        disk.len(),
        catalog.len()
    );
    differences
}

/// The catalog row of `path` as the watcher builds it before
/// `Database::rename_file` (`watcher.rs` `inspect_single_path`).
fn watcher_item(path: &Path) -> ScannedItem {
    use std::time::UNIX_EPOCH;
    let metadata = std::fs::metadata(path).expect("renamed path");
    let display_path = path.to_string_lossy().into_owned();
    let name = path.file_name().expect("name").to_string_lossy().into_owned();
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| {
            duration.as_secs() * 10_000_000 + u64::from(duration.subsec_nanos()) / 100 + 116_444_736_000_000_000
        })
        .unwrap_or(0);
    ScannedItem {
        normalized_path: display_path.to_lowercase(),
        lower_name: name.to_lowercase(),
        parent: path.parent().map(|parent| parent.to_string_lossy().into_owned()).unwrap_or_default(),
        is_directory: metadata.is_dir(),
        extension: None,
        size: 0,
        display_path,
        name,
        modified_at,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: catalog_sweep <catalog.db> <root> [--verify] [--rename <old path> <new path>]";
    let path = args.next().expect(usage);
    let root = PathBuf::from(args.next().expect(usage));
    let (mut check, mut rename) = (false, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--verify" => check = true,
            "--rename" => rename = Some((args.next().expect(usage), PathBuf::from(args.next().expect(usage)))),
            _ => panic!("{usage}"),
        }
    }
    let db = Arc::new(Database::open(Path::new(&path)).expect("open catalog"));
    let root_text = root.to_string_lossy().to_lowercase();
    let volume_id = db
        .get_volume_ids()
        .expect("read volumes")
        .into_iter()
        .find(|(_, mount)| root_text.starts_with(&mount.to_lowercase()))
        .map(|(id, _)| id)
        .expect("no catalog volume holds the root");
    let app_data = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("app.prism.launcher");

    let (cpu_before, io_before) = (cpu_ms(), io());
    let started = Instant::now();
    let entries = match &rename {
        // A folder renamed on disk, applied as the watcher applies it.
        Some((old, new)) => {
            db.rename_file(&volume_id, &old.to_lowercase(), &watcher_item(new))
                .expect("rename");
            0
        }
        None => reconcile_scope(
            &root,
            &volume_id,
            1,
            Arc::clone(&db),
            &app_data,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("sweep"),
    };
    let sweep_ms = started.elapsed().as_secs_f64() * 1000.0;
    let (cpu_after, io_after) = (cpu_ms(), io());
    println!(
        "{{\"sweep_ms\":{sweep_ms:.0},\"kernel_ms\":{:.0},\"user_ms\":{:.0},\"entries\":{entries},\"read_mib\":{:.1},\"write_mib\":{:.1},\"rows_deleted\":{}}}",
        cpu_after.0 - cpu_before.0,
        cpu_after.1 - cpu_before.1,
        (io_after.ReadTransferCount - io_before.ReadTransferCount) as f64 / 1_048_576.0,
        (io_after.WriteTransferCount - io_before.WriteTransferCount) as f64 / 1_048_576.0,
        db.rows_deleted()
    );
    if check && verify(&path, &root) > 0 {
        std::process::exit(1);
    }
}
