use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags};

use super::types::{CandidateEntry, ScannedItem, VolumeCoverage, VolumeInfo, VolumeState};

#[allow(dead_code)]
const SCHEMA_VERSION: u32 = 1;

pub struct Database {
    #[allow(dead_code)]
    db_path: PathBuf,
    writer: Arc<Mutex<Connection>>,
    reader: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(db_path: &Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        match Self::try_open(db_path) {
            Ok(db) => Ok(db),
            Err(err) => {
                eprintln!("[Prism Catalog] Failed to open database ({err}), recreating cleanly...");
                let _ = std::fs::remove_file(db_path);
                let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
                let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
                Self::try_open(db_path).map_err(|e| format!("database recreation failed: {e}"))
            }
        }
    }

    fn try_open(db_path: &Path) -> Result<Self, String> {
        let writer = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| e.to_string())?;

        Self::configure_pragmas(&writer)?;
        Self::init_schema(&writer)?;

        let reader = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| e.to_string())?;
        Self::configure_pragmas(&reader)?;

        Ok(Self {
            db_path: db_path.to_path_buf(),
            writer: Arc::new(Mutex::new(writer)),
            reader: Arc::new(Mutex::new(reader)),
        })
    }

    fn configure_pragmas(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size = -64000;
             PRAGMA mmap_size = 268435456;",
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn init_schema(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             INSERT OR IGNORE INTO meta(key, value) VALUES('version', '{SCHEMA_VERSION}');

             CREATE TABLE IF NOT EXISTS volumes (
                volume_id TEXT PRIMARY KEY,
                mount_path TEXT NOT NULL,
                drive_type INTEGER NOT NULL,
                label TEXT NOT NULL,
                file_system TEXT NOT NULL,
                state TEXT NOT NULL,
                scanned_entries INTEGER NOT NULL DEFAULT 0,
                last_scanned_at INTEGER NOT NULL DEFAULT 0,
                scan_generation INTEGER NOT NULL DEFAULT 0
             );

             CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                volume_id TEXT NOT NULL,
                normalized_path TEXT NOT NULL,
                display_path TEXT NOT NULL,
                name TEXT NOT NULL,
                lower_name TEXT NOT NULL,
                parent TEXT NOT NULL,
                is_directory INTEGER NOT NULL,
                extension TEXT,
                scan_generation INTEGER NOT NULL,
                modified_at INTEGER NOT NULL DEFAULT 0,
                size INTEGER NOT NULL DEFAULT 0,
                UNIQUE(volume_id, normalized_path)
             );

             CREATE INDEX IF NOT EXISTS idx_files_vol_gen ON files(volume_id, scan_generation);
             CREATE INDEX IF NOT EXISTS idx_files_lower_name ON files(lower_name, is_directory);
             CREATE INDEX IF NOT EXISTS idx_files_parent ON files(parent);

             CREATE VIRTUAL TABLE IF NOT EXISTS file_fts USING fts5(
                name,
                tokenize='trigram',
                content='files',
                content_rowid='id'
             );

             CREATE TRIGGER IF NOT EXISTS files_ai AFTER INSERT ON files BEGIN
                INSERT INTO file_fts(rowid, name) VALUES (new.id, new.name);
             END;

             CREATE TRIGGER IF NOT EXISTS files_ad AFTER DELETE ON files BEGIN
                INSERT INTO file_fts(file_fts, rowid, name) VALUES ('delete', old.id, old.name);
             END;

             CREATE TRIGGER IF NOT EXISTS files_au AFTER UPDATE OF name ON files BEGIN
                INSERT INTO file_fts(file_fts, rowid, name) VALUES ('delete', old.id, old.name);
                INSERT INTO file_fts(rowid, name) VALUES (new.id, new.name);
             END;"
        ))
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub fn upsert_volume(&self, vol: &VolumeInfo, state: VolumeState) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        let mount_str = vol
            .mount_paths
            .first()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| vol.drive_letter.clone().unwrap_or_default());
        let state_str = match state {
            VolumeState::Ready => "ready",
            VolumeState::Indexing => "indexing",
            VolumeState::Offline => "offline",
            VolumeState::Error => "error",
        };

        conn.execute(
            "INSERT INTO volumes(volume_id, mount_path, drive_type, label, file_system, state)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(volume_id) DO UPDATE SET
                mount_path = excluded.mount_path,
                drive_type = excluded.drive_type,
                label = excluded.label,
                file_system = excluded.file_system,
                state = excluded.state;",
            params![
                vol.volume_id,
                mount_str,
                vol.drive_type,
                vol.label,
                vol.fs_type,
                state_str
            ],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub fn set_volume_state(&self, volume_id: &str, state: VolumeState) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        let state_str = match state {
            VolumeState::Ready => "ready",
            VolumeState::Indexing => "indexing",
            VolumeState::Offline => "offline",
            VolumeState::Error => "error",
        };
        conn.execute(
            "UPDATE volumes SET state = ?1 WHERE volume_id = ?2;",
            params![state_str, volume_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn insert_batch(
        &self,
        volume_id: &str,
        generation: u64,
        items: &[ScannedItem],
    ) -> Result<(), String> {
        if items.is_empty() {
            return Ok(());
        }
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO files(
                        volume_id, normalized_path, display_path, name, lower_name, parent,
                        is_directory, extension, scan_generation, modified_at, size
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                    ON CONFLICT(volume_id, normalized_path) DO UPDATE SET
                        display_path = excluded.display_path,
                        name = excluded.name,
                        lower_name = excluded.lower_name,
                        parent = excluded.parent,
                        is_directory = excluded.is_directory,
                        extension = excluded.extension,
                        scan_generation = excluded.scan_generation,
                        modified_at = excluded.modified_at,
                        size = excluded.size;",
                )
                .map_err(|e| e.to_string())?;

            for item in items {
                stmt.execute(params![
                    volume_id,
                    item.normalized_path,
                    item.display_path,
                    item.name,
                    item.lower_name,
                    item.parent,
                    item.is_directory as i32,
                    item.extension,
                    generation as i64,
                    item.modified_at as i64,
                    item.size as i64,
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn finish_volume_scan(
        &self,
        volume_id: &str,
        generation: u64,
        total_scanned: u64,
    ) -> Result<(), String> {
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        // Delete records from previous generations for this volume that were not seen
        tx.execute(
            "DELETE FROM files WHERE volume_id = ?1 AND scan_generation != ?2;",
            params![volume_id, generation as i64],
        )
        .map_err(|e| e.to_string())?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        tx.execute(
            "UPDATE volumes SET
                state = 'ready',
                scanned_entries = ?1,
                last_scanned_at = ?2,
                scan_generation = ?3
             WHERE volume_id = ?4;",
            params![
                total_scanned as i64,
                now as i64,
                generation as i64,
                volume_id
            ],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn add_or_update_file(&self, volume_id: &str, item: &ScannedItem) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO files(
                volume_id, normalized_path, display_path, name, lower_name, parent,
                is_directory, extension, scan_generation, modified_at, size
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10)
            ON CONFLICT(volume_id, normalized_path) DO UPDATE SET
                display_path = excluded.display_path,
                name = excluded.name,
                lower_name = excluded.lower_name,
                parent = excluded.parent,
                is_directory = excluded.is_directory,
                extension = excluded.extension,
                modified_at = excluded.modified_at,
                size = excluded.size;",
            params![
                volume_id,
                item.normalized_path,
                item.display_path,
                item.name,
                item.lower_name,
                item.parent,
                item.is_directory as i32,
                item.extension,
                item.modified_at as i64,
                item.size as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn remove_file(
        &self,
        volume_id: &str,
        normalized_path: &str,
        is_dir: bool,
    ) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        if is_dir {
            let prefix = format!("{normalized_path}\\");
            conn.execute(
                "DELETE FROM files WHERE volume_id = ?1 AND (normalized_path = ?2 OR normalized_path LIKE ?3 || '%');",
                params![volume_id, normalized_path, prefix],
            )
            .map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "DELETE FROM files WHERE volume_id = ?1 AND normalized_path = ?2;",
                params![volume_id, normalized_path],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn rename_file(
        &self,
        volume_id: &str,
        old_normalized: &str,
        new_item: &ScannedItem,
    ) -> Result<(), String> {
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        if new_item.is_directory {
            let old_prefix = format!("{old_normalized}\\");
            let new_prefix = format!("{}\\", new_item.normalized_path);
            let old_display_prefix = format!("{}\\", old_normalized);
            let new_display_prefix = format!("{}\\", new_item.display_path);

            // Update children paths
            tx.execute(
                "UPDATE files SET
                    normalized_path = ?1 || SUBSTR(normalized_path, ?2),
                    display_path = ?3 || SUBSTR(display_path, ?4),
                    parent = ?3 || SUBSTR(parent, ?4)
                 WHERE volume_id = ?5 AND normalized_path LIKE ?6 || '%';",
                params![
                    new_prefix,
                    (old_prefix.len() + 1) as i64,
                    new_display_prefix,
                    (old_display_prefix.len() + 1) as i64,
                    volume_id,
                    old_prefix,
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        // Delete old entry
        tx.execute(
            "DELETE FROM files WHERE volume_id = ?1 AND normalized_path = ?2;",
            params![volume_id, old_normalized],
        )
        .map_err(|e| e.to_string())?;

        // Insert new entry
        tx.execute(
            "INSERT INTO files(
                volume_id, normalized_path, display_path, name, lower_name, parent,
                is_directory, extension, scan_generation, modified_at, size
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10)
            ON CONFLICT(volume_id, normalized_path) DO UPDATE SET
                display_path = excluded.display_path,
                name = excluded.name,
                lower_name = excluded.lower_name,
                parent = excluded.parent,
                is_directory = excluded.is_directory,
                extension = excluded.extension,
                modified_at = excluded.modified_at,
                size = excluded.size;",
            params![
                volume_id,
                new_item.normalized_path,
                new_item.display_path,
                new_item.name,
                new_item.lower_name,
                new_item.parent,
                new_item.is_directory as i32,
                new_item.extension,
                new_item.modified_at as i64,
                new_item.size as i64,
            ],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_volume_coverages(&self) -> Result<Vec<VolumeCoverage>, String> {
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT v.mount_path, v.state, COALESCE(COUNT(f.id), v.scanned_entries) as cnt
                 FROM volumes v
                 LEFT JOIN files f ON f.volume_id = v.volume_id
                 GROUP BY v.volume_id;",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                let mount_path: String = row.get(0)?;
                let state_str: String = row.get(1)?;
                let count: i64 = row.get(2)?;
                let state = match state_str.as_str() {
                    "ready" => VolumeState::Ready,
                    "indexing" => VolumeState::Indexing,
                    "offline" => VolumeState::Offline,
                    _ => VolumeState::Error,
                };
                Ok(VolumeCoverage {
                    drive: mount_path,
                    state,
                    indexed_count: count.max(0) as u64,
                    total_progress: None,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for cov in rows.flatten() {
            out.push(cov);
        }
        Ok(out)
    }

    pub fn get_total_indexed_count(&self) -> Result<u64, String> {
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files;", [], |row| row.get(0))
            .unwrap_or(0);
        Ok(count.max(0) as u64)
    }

    pub fn search_candidates(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<CandidateEntry>, String> {
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let lower = query.to_lowercase();
        let query_len = query.chars().count();
        let mut candidates = Vec::with_capacity(limit * 2);
        let mut seen_ids = std::collections::HashSet::new();

        // 1. Exact match query
        {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, display_path, lower_name, is_directory, extension
                     FROM files WHERE lower_name = ?1 LIMIT 20;",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![lower], |row| {
                    Ok(CandidateEntry {
                        id: row.get(0)?,
                        display_path: row.get(1)?,
                        lower_name: row.get(2)?,
                        is_directory: row.get::<_, i32>(3)? != 0,
                        extension: row.get(4)?,
                    })
                })
                .map_err(|e| e.to_string())?;

            for row in rows.flatten() {
                if seen_ids.insert(row.id) {
                    candidates.push(row);
                }
            }
        }

        // 2. Prefix match query
        {
            let prefix_end = format!("{lower}\u{FFFF}");
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, display_path, lower_name, is_directory, extension
                     FROM files WHERE lower_name >= ?1 AND lower_name <= ?2 LIMIT ?3;",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![lower, prefix_end, limit as i64], |row| {
                    Ok(CandidateEntry {
                        id: row.get(0)?,
                        display_path: row.get(1)?,
                        lower_name: row.get(2)?,
                        is_directory: row.get::<_, i32>(3)? != 0,
                        extension: row.get(4)?,
                    })
                })
                .map_err(|e| e.to_string())?;

            for row in rows.flatten() {
                if seen_ids.insert(row.id) {
                    candidates.push(row);
                }
            }
        }

        // 3. 3+ characters: FTS5 Trigram MATCH query
        if query_len >= 3 {
            let fts_query = sanitize_fts5_trigram_query(&lower);
            if !fts_query.is_empty() {
                let mut stmt = conn
                    .prepare_cached(
                        "SELECT id, display_path, lower_name, is_directory, extension
                         FROM files
                         WHERE id IN (
                            SELECT rowid FROM file_fts WHERE name MATCH ?1 LIMIT ?2
                         )
                         LIMIT ?2;",
                    )
                    .map_err(|e| e.to_string())?;

                let rows = stmt
                    .query_map(params![fts_query, (limit * 2) as i64], |row| {
                        Ok(CandidateEntry {
                            id: row.get(0)?,
                            display_path: row.get(1)?,
                            lower_name: row.get(2)?,
                            is_directory: row.get::<_, i32>(3)? != 0,
                            extension: row.get(4)?,
                        })
                    })
                    .map_err(|e| e.to_string())?;

                for row in rows.flatten() {
                    if seen_ids.insert(row.id) {
                        candidates.push(row);
                    }
                }
            }
        } else if query_len == 2 {
            // For 2 characters: Indexed prefix range query using B-Tree index
            let prefix_end = format!("{lower}\u{FFFF}");
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, display_path, lower_name, is_directory, extension
                     FROM files WHERE lower_name >= ?1 AND lower_name <= ?2 LIMIT ?3;",
                )
                .map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map(params![lower, prefix_end, limit as i64], |row| {
                    Ok(CandidateEntry {
                        id: row.get(0)?,
                        display_path: row.get(1)?,
                        lower_name: row.get(2)?,
                        is_directory: row.get::<_, i32>(3)? != 0,
                        extension: row.get(4)?,
                    })
                })
                .map_err(|e| e.to_string())?;

            for row in rows.flatten() {
                if seen_ids.insert(row.id) {
                    candidates.push(row);
                }
            }
        }

        Ok(candidates)
    }
}

/// Safely formats and escapes tokens for FTS5 trigram queries.
/// For trigram tokenizer, wrap tokens in double quotes `""token""`.
fn sanitize_fts5_trigram_query(query: &str) -> String {
    let tokens: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.chars().count() >= 3)
        .map(|token| {
            // Escape existing double quotes by doubling them
            let escaped = token.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect();

    tokens.join(" AND ")
}
