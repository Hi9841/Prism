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
    fn scanner_includes_appdata_nodemodules_and_deep_trees() {
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

        // Create AppData and node_modules folders
        let appdata_folder = base.join("AppData").join("Local");
        std::fs::create_dir_all(&appdata_folder).unwrap();
        let appdata_file = appdata_folder.join("appdata_target.txt");
        std::fs::write(&appdata_file, "appdata content").unwrap();

        let node_modules_folder = base.join("node_modules").join("pkg");
        std::fs::create_dir_all(&node_modules_folder).unwrap();
        let nodemodules_file = node_modules_folder.join("nodemodules_target.txt");
        std::fs::write(&nodemodules_file, "nodemodules content").unwrap();

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
        assert_eq!(r3.items.len(), 1);

        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(db_dir);
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
