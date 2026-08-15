use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

#[allow(unused_imports)]
pub use crate::catalog::search::{browse_path, entry_score, target_score};
#[allow(unused_imports)]
pub use crate::catalog::types::{
    FileEntry, FileSearchResponse, QuickAccessEntry, VolumeCoverage, VolumeState,
};
pub use crate::catalog::{file_thumbnail, quick_access, warm, FileIndex};

pub(crate) fn replace_file(temp: &Path, destination: &Path) -> Result<(), String> {
    let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::db::Database;
    use crate::catalog::scanner::scan_volume;
    use crate::catalog::search::search;
    use crate::catalog::types::{CandidateEntry, ScannedItem};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_db() -> (Arc<Database>, PathBuf) {
        let temp_dir = std::env::temp_dir().join(format!(
            "prism-test-db-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("catalog.db");
        let db = Database::open(&db_path).expect("open test database");
        (Arc::new(db), temp_dir)
    }

    #[test]
    fn exact_and_prefix_names_rank_first() {
        let exact = CandidateEntry {
            id: 1,
            display_path: r"C:\Users\me\Documents\report.pdf".into(),
            lower_name: "report.pdf".into(),
            is_directory: false,
            extension: Some("pdf".into()),
        };
        let prefix = CandidateEntry {
            id: 2,
            display_path: r"C:\Users\me\Downloads\report-final.pdf".into(),
            lower_name: "report-final.pdf".into(),
            is_directory: false,
            extension: Some("pdf".into()),
        };
        let unrelated = CandidateEntry {
            id: 3,
            display_path: r"C:\Users\me\report\notes.txt".into(),
            lower_name: "notes.txt".into(),
            is_directory: false,
            extension: Some("txt".into()),
        };

        let tokens = ["report"];
        let exact_score = entry_score(&exact, &tokens, "report").unwrap();
        let prefix_score = entry_score(&prefix, &tokens, "report").unwrap();
        assert!(exact_score > prefix_score);
        assert!(entry_score(&unrelated, &tokens, "report").is_none());
    }

    #[test]
    fn exact_underscore_containing_filename_ranks_first() {
        let exact = CandidateEntry {
            id: 1,
            display_path: r"C:\Users\me\Documents\gemini_revisions_unverified.md".into(),
            lower_name: "gemini_revisions_unverified.md".into(),
            is_directory: false,
            extension: Some("md".into()),
        };
        let partial = CandidateEntry {
            id: 2,
            display_path: r"C:\Users\me\Documents\gemini_revisions.md".into(),
            lower_name: "gemini_revisions.md".into(),
            is_directory: false,
            extension: Some("md".into()),
        };
        let query = "gemini_revisions_unverified.md";
        let tokens = ["gemini_revisions_unverified.md"];
        let exact_score = entry_score(&exact, &tokens, query).unwrap();
        let partial_score = entry_score(&partial, &tokens, query);
        assert!(partial_score.is_none() || exact_score > partial_score.unwrap());
    }

    #[test]
    fn supports_fuzzy_and_multi_token_matches() {
        let file = CandidateEntry {
            id: 1,
            display_path: r"C:\Users\me\Documents\Project Roadmap 2026.pdf".into(),
            lower_name: "project roadmap 2026.pdf".into(),
            is_directory: false,
            extension: Some("pdf".into()),
        };
        assert!(entry_score(&file, &["prjct"], "prjct").is_some());
        assert!(entry_score(&file, &["road", "2026"], "road 2026").is_some());
        assert!(entry_score(&file, &["road", "missing"], "road missing").is_none());
        assert!(target_score("wez", "windows performance analyzer").is_none());
    }

    #[test]
    fn search_is_capped_and_requires_two_characters() {
        let (db, temp_dir) = test_db();
        let mut items = Vec::new();
        for i in 0..30 {
            let file_path = temp_dir.join(format!("report-{i}.txt"));
            std::fs::write(&file_path, "content").unwrap();
            items.push(ScannedItem {
                normalized_path: file_path.to_string_lossy().to_lowercase(),
                display_path: file_path.to_string_lossy().into_owned(),
                name: format!("report-{i}.txt"),
                lower_name: format!("report-{i}.txt"),
                parent: temp_dir.to_string_lossy().into_owned(),
                is_directory: false,
                extension: Some("txt".into()),
                modified_at: 0,
                size: 7,
            });
        }
        db.insert_batch("vol1", 1, &items).unwrap();
        let gen = AtomicU64::new(0);

        let res1 = search("r", Some(5), &db, &gen, &[], 30, false, true);
        assert!(res1.items.is_empty());

        let res5 = search("report", Some(5), &db, &gen, &[], 30, false, true);
        assert_eq!(res5.items.len(), 5);

        let res20 = search("report", Some(200), &db, &gen, &[], 30, false, true);
        assert_eq!(res20.items.len(), 20);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn deleted_index_entry_is_never_returned() {
        let (db, temp_dir) = test_db();
        let file_path = temp_dir.join("prism-vanishing-note.txt");
        std::fs::write(&file_path, "content").unwrap();

        let item = ScannedItem {
            normalized_path: file_path.to_string_lossy().to_lowercase(),
            display_path: file_path.to_string_lossy().into_owned(),
            name: "prism-vanishing-note.txt".into(),
            lower_name: "prism-vanishing-note.txt".into(),
            parent: temp_dir.to_string_lossy().into_owned(),
            is_directory: false,
            extension: Some("txt".into()),
            modified_at: 0,
            size: 7,
        };
        db.insert_batch("vol1", 1, &[item]).unwrap();
        let gen = AtomicU64::new(0);

        let res = search("vanishing", Some(5), &db, &gen, &[], 1, false, true);
        assert_eq!(res.items.len(), 1);

        std::fs::remove_file(&file_path).unwrap();
        let res_after = search("vanishing", Some(5), &db, &gen, &[], 1, false, true);
        assert!(res_after.items.is_empty());

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn absolute_directory_browse_does_not_depend_on_the_index() {
        let base = std::env::temp_dir().join(format!(
            "prism-file-browse-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let folder = base.join("Saved");
        std::fs::create_dir_all(&folder).expect("create test folder");
        std::fs::write(base.join("notes.txt"), "test").expect("create test file");

        let response = browse_path(&base.to_string_lossy(), 8).unwrap();
        assert_eq!(response.len(), 2);
        assert_eq!(response[0].name, "Saved");
        assert!(response[0].is_directory);

        let partial = base.join("sav");
        let response = browse_path(&partial.to_string_lossy(), 8).unwrap();
        assert_eq!(response.len(), 1);
        assert_eq!(response[0].name, "Saved");

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn scanner_skips_excluded_dirs_but_scans_deep_trees() {
        let base = std::env::temp_dir().join(format!(
            "prism-scan-deep-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        // Create deep folder > 16 levels
        let mut deep = base.clone();
        for i in 1..=18 {
            deep = deep.join(format!("level{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        let deep_file = deep.join("deep_target.txt");
        std::fs::write(&deep_file, "deep content").unwrap();

        // AppData is indexed (only Local\Temp is excluded)
        let appdata_folder = base.join("AppData").join("Local");
        std::fs::create_dir_all(&appdata_folder).unwrap();
        let appdata_file = appdata_folder.join("appdata_target.txt");
        std::fs::write(&appdata_file, "appdata content").unwrap();

        // node_modules and root-level Windows are excluded
        let node_modules_folder = base.join("node_modules").join("pkg");
        std::fs::create_dir_all(&node_modules_folder).unwrap();
        let nodemodules_file = node_modules_folder.join("nodemodules_target.txt");
        std::fs::write(&nodemodules_file, "nodemodules content").unwrap();

        let windows_folder = base.join("Windows").join("System32");
        std::fs::create_dir_all(&windows_folder).unwrap();
        let windows_file = windows_folder.join("windows_target.txt");
        std::fs::write(&windows_file, "windows content").unwrap();

        // Temp under AppData\Local is excluded
        let temp_folder = appdata_folder.join("Temp");
        std::fs::create_dir_all(&temp_folder).unwrap();
        let temp_file = temp_folder.join("temp_target.txt");
        std::fs::write(&temp_file, "temp content").unwrap();

        // A non-root "windows" folder is still indexed
        let nested_windows = base.join("project").join("windows");
        std::fs::create_dir_all(&nested_windows).unwrap();
        let nested_windows_file = nested_windows.join("nested_target.txt");
        std::fs::write(&nested_windows_file, "nested content").unwrap();

        let (db, db_dir) = test_db();
        let cancel = Arc::new(AtomicBool::new(false));
        let total = scan_volume(&base, "test_vol", 1, db.clone(), &db_dir, cancel, |_| {}).unwrap();
        assert!(total >= 20);

        let gen = AtomicU64::new(0);
        let r1 = search("deep_target", Some(5), &db, &gen, &[], total, false, true);
        assert_eq!(r1.items.len(), 1);
        assert_eq!(r1.items[0].name, "deep_target.txt");

        let r2 = search(
            "appdata_target",
            Some(5),
            &db,
            &gen,
            &[],
            total,
            false,
            true,
        );
        assert_eq!(r2.items.len(), 1);

        let r3 = search(
            "nodemodules_target",
            Some(5),
            &db,
            &gen,
            &[],
            total,
            false,
            true,
        );
        assert!(r3.items.is_empty(), "node_modules must not be indexed");

        let r4 = search(
            "windows_target",
            Some(5),
            &db,
            &gen,
            &[],
            total,
            false,
            true,
        );
        assert!(
            r4.items.is_empty(),
            "root-level Windows must not be indexed"
        );

        let r5 = search("temp_target", Some(5), &db, &gen, &[], total, false, true);
        assert!(
            r5.items.is_empty(),
            "AppData\\Local\\Temp must not be indexed"
        );

        let r6 = search("nested_target", Some(5), &db, &gen, &[], total, false, true);
        assert_eq!(r6.items.len(), 1, "non-root 'windows' folders stay indexed");

        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(db_dir);
    }

    #[test]
    fn sweep_rewrites_only_changed_files_and_prunes_deletions() {
        let base = std::env::temp_dir().join(format!(
            "prism-scan-incremental-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("docs")).unwrap();
        let first = base.join("docs").join("first.txt");
        std::fs::write(&first, "first").unwrap();

        let (db, db_dir) = test_db();
        let cancel = Arc::new(AtomicBool::new(false));

        // First scan indexes the whole tree (docs dir + first.txt; only files
        // become rows).
        let total = scan_volume(
            &base,
            "test_vol",
            1,
            db.clone(),
            &db_dir,
            cancel.clone(),
            |_| {},
        )
        .unwrap();
        assert_eq!(total, 2);
        assert_eq!(db.get_total_indexed_count().unwrap(), 1);

        std::thread::sleep(std::time::Duration::from_millis(50));

        // Nothing changed: the sweep walks the tree but must not grow the
        // index (unchanged rows are filtered before writing).
        let total = scan_volume(
            &base,
            "test_vol",
            2,
            db.clone(),
            &db_dir,
            cancel.clone(),
            |_| {},
        )
        .unwrap();
        assert_eq!(total, 2);
        assert_eq!(
            db.get_total_indexed_count().unwrap(),
            1,
            "no rows should be added"
        );

        std::thread::sleep(std::time::Duration::from_millis(50));

        // A file is added: the sweep picks it up.
        let second = base.join("docs").join("second.txt");
        std::fs::write(&second, "second").unwrap();
        let total = scan_volume(
            &base,
            "test_vol",
            3,
            db.clone(),
            &db_dir,
            cancel.clone(),
            |_| {},
        )
        .unwrap();
        assert_eq!(total, 3);
        assert_eq!(db.get_total_indexed_count().unwrap(), 2);

        let gen = AtomicU64::new(0);
        let r = search("second", Some(5), &db, &gen, &[], total, false, true);
        assert_eq!(r.items.len(), 1);

        std::thread::sleep(std::time::Duration::from_millis(50));

        // A file is deleted: the sweep prunes its row.
        std::fs::remove_file(&first).unwrap();
        let total = scan_volume(&base, "test_vol", 4, db.clone(), &db_dir, cancel, |_| {}).unwrap();
        assert_eq!(total, 2);
        assert_eq!(db.get_total_indexed_count().unwrap(), 1);

        let r = search("first", Some(5), &db, &gen, &[], total, false, true);
        assert!(
            r.items.is_empty(),
            "deleted file must be pruned from the index"
        );
        let r = search("second", Some(5), &db, &gen, &[], total, false, true);
        assert_eq!(r.items.len(), 1);

        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(db_dir);
    }

    #[test]
    fn filter_changed_skips_rows_with_matching_mtime_and_size() {
        let (db, temp_dir) = test_db();
        let file_path = temp_dir.join("stable.txt");
        std::fs::write(&file_path, "content").unwrap();
        let item = ScannedItem {
            normalized_path: file_path.to_string_lossy().to_lowercase(),
            display_path: file_path.to_string_lossy().into_owned(),
            name: "stable.txt".into(),
            lower_name: "stable.txt".into(),
            parent: temp_dir.to_string_lossy().into_owned(),
            is_directory: false,
            extension: Some("txt".into()),
            modified_at: 42,
            size: 7,
        };
        db.insert_batch("vol1", 1, std::slice::from_ref(&item)).unwrap();

        // Identical row: filtered out.
        let filtered = db.filter_changed("vol1", std::slice::from_ref(&item)).unwrap();
        assert_eq!(filtered.len(), 0);

        // Same path, new mtime/size: kept for re-insert.
        let mut changed = item.clone();
        changed.modified_at = 43;
        changed.size = 8;
        let filtered = db.filter_changed("vol1", &[changed]).unwrap();
        assert_eq!(filtered.len(), 1);

        // A brand-new path: kept.
        let mut fresh = item.clone();
        fresh.normalized_path = "c:\\fake\\new.txt".into();
        let filtered = db.filter_changed("vol1", &[fresh]).unwrap();
        assert_eq!(filtered.len(), 1);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn bulk_load_rebuilds_fts_when_the_last_scan_finishes() {
        let (db, temp_dir) = test_db();
        let mut items = Vec::new();
        for i in 0..10 {
            let file_path = temp_dir.join(format!("bulk-file-{i}.txt"));
            std::fs::write(&file_path, "data").unwrap();
            items.push(ScannedItem {
                normalized_path: file_path.to_string_lossy().to_lowercase(),
                display_path: file_path.to_string_lossy().into_owned(),
                name: format!("bulk-file-{i}.txt"),
                lower_name: format!("bulk-file-{i}.txt"),
                parent: temp_dir.to_string_lossy().into_owned(),
                is_directory: false,
                extension: Some("txt".into()),
                modified_at: 1700000000 + i as u64,
                size: 4,
            });
        }

        // Nested bulk loads: triggers stay dropped until the last end.
        db.begin_bulk_load().unwrap();
        db.begin_bulk_load().unwrap();
        db.insert_batch("vol1", 1, &items).unwrap();
        db.end_bulk_load().unwrap();
        db.end_bulk_load().unwrap();

        let gen = AtomicU64::new(0);
        // Trigram FTS needs 3+ characters; infix match proves the rebuild ran.
        let res = search("bulk-file-5", Some(5), &db, &gen, &[], 10, false, true);
        assert_eq!(res.items.len(), 1);
        assert_eq!(res.items[0].name, "bulk-file-5.txt");

        // Watcher-style single-file writes still update FTS after the restore.
        let extra_path = temp_dir.join("bulk-after.txt");
        std::fs::write(&extra_path, "data").unwrap();
        let extra = ScannedItem {
            normalized_path: extra_path.to_string_lossy().to_lowercase(),
            display_path: extra_path.to_string_lossy().into_owned(),
            name: "bulk-after.txt".into(),
            lower_name: "bulk-after.txt".into(),
            parent: temp_dir.to_string_lossy().into_owned(),
            is_directory: false,
            extension: Some("txt".into()),
            modified_at: 1700000000,
            size: 4,
        };
        db.add_or_update_file("vol1", &extra).unwrap();
        let res = search("bulk-after", Some(5), &db, &gen, &[], 11, false, true);
        assert_eq!(res.items.len(), 1);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn excluded_rows_are_purged_from_existing_catalogs() {
        let (db, temp_dir) = test_db();
        // The file tree lives outside the user temp dir: rows under
        // AppData\Local\Temp are themselves excluded by the purge patterns.
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "prism-purge-test-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
        let keep_path = base.join("keep.txt");
        let junk_path = base.join("node_modules").join("pkg").join("junk.txt");
        std::fs::create_dir_all(junk_path.parent().unwrap()).unwrap();
        std::fs::write(&keep_path, "keep").unwrap();
        std::fs::write(&junk_path, "junk").unwrap();

        let items = [
            ScannedItem {
                normalized_path: keep_path.to_string_lossy().to_lowercase(),
                display_path: keep_path.to_string_lossy().into_owned(),
                name: "keep.txt".into(),
                lower_name: "keep.txt".into(),
                parent: temp_dir.to_string_lossy().into_owned(),
                is_directory: false,
                extension: Some("txt".into()),
                modified_at: 1,
                size: 4,
            },
            ScannedItem {
                normalized_path: junk_path.to_string_lossy().to_lowercase(),
                display_path: junk_path.to_string_lossy().into_owned(),
                name: "junk.txt".into(),
                lower_name: "junk.txt".into(),
                parent: junk_path.parent().unwrap().to_string_lossy().into_owned(),
                is_directory: false,
                extension: Some("txt".into()),
                modified_at: 1,
                size: 4,
            },
        ];
        db.insert_batch("vol1", 1, &items).unwrap();
        assert_eq!(db.get_total_indexed_count().unwrap(), 2);

        // The migration path drops the triggers, purges, and rebuilds FTS.
        db.begin_bulk_load().unwrap();
        let purged = db.purge_excluded_rows().unwrap();
        db.end_bulk_load().unwrap();
        assert_eq!(purged, 1);
        assert_eq!(db.get_total_indexed_count().unwrap(), 1);

        let gen = AtomicU64::new(0);
        let r = search("junk", Some(5), &db, &gen, &[], 1, false, true);
        assert!(r.items.is_empty());
        let r = search("keep", Some(5), &db, &gen, &[], 1, false, true);
        assert_eq!(r.items.len(), 1);

        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn watcher_add_rename_delete_operations() {
        let (db, temp_dir) = test_db();
        let file_path = temp_dir.join("live_file.txt");
        std::fs::write(&file_path, "test").unwrap();

        let item = ScannedItem {
            normalized_path: file_path.to_string_lossy().to_lowercase(),
            display_path: file_path.to_string_lossy().into_owned(),
            name: "live_file.txt".into(),
            lower_name: "live_file.txt".into(),
            parent: temp_dir.to_string_lossy().into_owned(),
            is_directory: false,
            extension: Some("txt".into()),
            modified_at: 0,
            size: 4,
        };

        // Add
        db.add_or_update_file("vol1", &item).unwrap();
        let gen = AtomicU64::new(0);
        assert_eq!(
            search("live_file", Some(5), &db, &gen, &[], 1, false, true)
                .items
                .len(),
            1
        );

        // Rename
        let renamed_path = temp_dir.join("renamed_file.txt");
        std::fs::rename(&file_path, &renamed_path).unwrap();
        let renamed_item = ScannedItem {
            normalized_path: renamed_path.to_string_lossy().to_lowercase(),
            display_path: renamed_path.to_string_lossy().into_owned(),
            name: "renamed_file.txt".into(),
            lower_name: "renamed_file.txt".into(),
            parent: temp_dir.to_string_lossy().into_owned(),
            is_directory: false,
            extension: Some("txt".into()),
            modified_at: 0,
            size: 4,
        };
        db.rename_file("vol1", &item.normalized_path, &renamed_item)
            .unwrap();
        assert_eq!(
            search("live_file", Some(5), &db, &gen, &[], 1, false, true)
                .items
                .len(),
            0
        );
        assert_eq!(
            search("renamed_file", Some(5), &db, &gen, &[], 1, false, true)
                .items
                .len(),
            1
        );

        // Delete
        std::fs::remove_file(&renamed_path).unwrap();
        db.remove_file("vol1", &renamed_item.normalized_path, false)
            .unwrap();
        assert_eq!(
            search("renamed_file", Some(5), &db, &gen, &[], 0, false, true)
                .items
                .len(),
            0
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn corrupt_database_recreates_cleanly() {
        let temp_dir = std::env::temp_dir().join(format!(
            "prism-corrupt-db-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("catalog.db");
        std::fs::write(&db_path, "not a valid sqlite header").unwrap();

        // Database::open should detect corruption, remove it, and recreate cleanly
        let db = Database::open(&db_path).expect("open corrupt database should recover cleanly");
        assert_eq!(db.get_total_indexed_count().unwrap(), 0);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    /// Benchmark against 2 million synthetic entries to ensure < 50ms p95.
    #[test]
    #[ignore]
    fn benchmark_two_million_entries() {
        use std::time::Instant;

        let (db, temp_dir) = test_db();
        eprintln!("Inserting 2,000,000 synthetic entries into SQLite catalog...");
        let start_insert = Instant::now();

        let batch_size = 20_000;
        for batch_idx in 0..100 {
            let mut batch = Vec::with_capacity(batch_size);
            for i in 0..batch_size {
                let num = batch_idx * batch_size + i;
                batch.push(ScannedItem {
                    normalized_path: format!(
                        r"c:\users\bench\documents\project_{}\file_{}_report_{}.txt",
                        num % 500,
                        num,
                        num % 100
                    ),
                    display_path: format!(
                        r"C:\Users\bench\Documents\Project_{}\File_{}_Report_{}.txt",
                        num % 500,
                        num,
                        num % 100
                    ),
                    name: format!("File_{}_Report_{}.txt", num, num % 100),
                    lower_name: format!("file_{}_report_{}.txt", num, num % 100),
                    parent: format!(r"C:\Users\bench\Documents\Project_{}", num % 500),
                    is_directory: num % 20 == 0,
                    extension: Some("txt".into()),
                    modified_at: 1700000000 + (num as u64 % 10000),
                    size: (num as u64 * 37) % 1_000_000,
                });
            }
            db.insert_batch("C", 1, &batch).unwrap();
        }
        db.finish_volume_scan("C", 1, 2_000_000).unwrap();
        eprintln!(
            "Inserted 2M entries in {:.2}s. Total count: {}",
            start_insert.elapsed().as_secs_f64(),
            db.get_total_indexed_count().unwrap()
        );

        let _gen = AtomicU64::new(0);
        let queries = [
            "report",
            "file_12345",
            "project_499",
            "file_999999_report_99",
            "re",
            "zz",
            "nonexistent_file_name_query",
        ];

        for query in queries {
            let mut durations = Vec::with_capacity(50);
            let mut hit_count = 0;
            for _ in 0..50 {
                let t0 = Instant::now();
                let candidates = db.search_candidates(query, 200).unwrap();
                hit_count = candidates.len();
                durations.push(t0.elapsed());
            }
            durations.sort();
            let p50 = durations[durations.len() / 2].as_secs_f64() * 1000.0;
            let p95 = durations[(durations.len() as f64 * 0.95) as usize].as_secs_f64() * 1000.0;
            eprintln!(
                "Query '{query}': {hit_count} hits | p50: {p50:.2}ms, p95: {p95:.2}ms (target < 50ms)"
            );
            assert!(
                p95 < 50.0,
                "p95 search time was {p95:.2}ms, expected < 50ms"
            );
        }

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
