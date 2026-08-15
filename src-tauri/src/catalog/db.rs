use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, params_from_iter, Connection, OpenFlags};

use super::types::{CandidateEntry, ScannedItem, VolumeCoverage, VolumeInfo, VolumeState};

#[allow(dead_code)]
const SCHEMA_VERSION: u32 = 4;

/// One-time purge patterns for rows created by the 0.9.0 catalog, which indexed
/// everything including high-noise directories. Kept in sync with the scanner's
/// exclusion list (see `scanner::is_excluded_dir`). Patterns are lowercase
/// because `normalized_path` is stored lowercase.
const EXCLUDED_PURGE_PATTERNS: &[&str] = &[
    "%\\node_modules",
    "%\\node_modules\\%",
    "%\\.git",
    "%\\.git\\%",
    "%\\.svn",
    "%\\.svn\\%",
    "%\\.hg",
    "%\\.hg\\%",
    "%\\$recycle.bin",
    "%\\$recycle.bin\\%",
    "%\\system volume information",
    "%\\system volume information\\%",
    "%\\windows.old",
    "%\\windows.old\\%",
    "%\\$windows.~bt",
    "%\\$windows.~bt\\%",
    "%\\$windows.~ws",
    "%\\$windows.~ws\\%",
    "%\\recovery",
    "%\\recovery\\%",
    "%\\perflogs",
    "%\\perflogs\\%",
    "%\\windowsapps",
    "%\\windowsapps\\%",
    "%\\postgres_data",
    "%\\postgres_data\\%",
    "%\\pgdata",
    "%\\pgdata\\%",
    "_:\\windows",
    "_:\\windows\\%",
    "%\\appdata\\local\\temp",
    "%\\appdata\\local\\temp\\%",
];

pub struct Database {
    #[allow(dead_code)]
    db_path: PathBuf,
    writer: Arc<Mutex<Connection>>,
    reader: Arc<Mutex<Connection>>,
    bulk_load_depth: Mutex<u32>,
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

        let db = Self {
            db_path: db_path.to_path_buf(),
            writer: Arc::new(Mutex::new(writer)),
            reader: Arc::new(Mutex::new(reader)),
            bulk_load_depth: Mutex::new(0),
        };
        db.migrate()?;
        Ok(db)
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
             CREATE INDEX IF NOT EXISTS idx_files_vol_parent ON files(volume_id, parent);

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

    /// Schema migrations. Older catalogs need the exclusion purge and one full
    /// sweep so the directory-row and offline-placeholder behavior is applied
    /// to existing indexes as well.
    fn migrate(&self) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        let version: i64 = conn
            .query_row("SELECT value FROM meta WHERE key = 'version';", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        if version >= SCHEMA_VERSION as i64 {
            return Ok(());
        }

        if version == 1 || version == 2 || version == 3 {
            eprintln!(
                "[Prism Catalog] Migrating catalog schema v{version} -> v{SCHEMA_VERSION} (purging excluded paths)"
            );
            conn.execute_batch(
                "DROP TRIGGER IF EXISTS files_ai;
                 DROP TRIGGER IF EXISTS files_ad;
                 DROP TRIGGER IF EXISTS files_au;",
            )
            .map_err(|e| e.to_string())?;
            drop(conn);

            let purged = self.purge_excluded_rows()?;
            eprintln!("[Prism Catalog] Purged {purged} excluded rows");

            let conn = self.writer.lock().map_err(|e| e.to_string())?;
            conn.execute_batch(
                "CREATE TRIGGER IF NOT EXISTS files_ai AFTER INSERT ON files BEGIN
                    INSERT INTO file_fts(rowid, name) VALUES (new.id, new.name);
                 END;
                 CREATE TRIGGER IF NOT EXISTS files_ad AFTER DELETE ON files BEGIN
                    INSERT INTO file_fts(file_fts, rowid, name) VALUES ('delete', old.id, old.name);
                 END;
                 CREATE TRIGGER IF NOT EXISTS files_au AFTER UPDATE OF name ON files BEGIN
                    INSERT INTO file_fts(file_fts, rowid, name) VALUES ('delete', old.id, old.name);
                    INSERT INTO file_fts(rowid, name) VALUES (new.id, new.name);
                 END;
                 INSERT INTO file_fts(file_fts) VALUES('rebuild');
                 UPDATE meta SET value = '4' WHERE key = 'version';
                 -- Force one sweep so directory rows and offline placeholders
                 -- are reconciled into the persisted catalog.
                 UPDATE volumes SET last_scanned_at = 0;",
            )
            .map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "UPDATE meta SET value = ?1 WHERE key = 'version';",
                params![SCHEMA_VERSION],
            )
            .map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    /// Deletes index rows that the scanner now excludes (see
    /// `scanner::is_excluded_dir`). Returns the number of rows purged. Callers
    /// that dropped the FTS triggers must follow with an FTS rebuild.
    pub fn purge_excluded_rows(&self) -> Result<u64, String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        let patterns: Vec<&str> = EXCLUDED_PURGE_PATTERNS.to_vec();
        let placeholders: Vec<String> = patterns
            .iter()
            .map(|_| "normalized_path LIKE ?".to_string())
            .collect();
        let sql = format!("DELETE FROM files WHERE {};", placeholders.join(" OR "));
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(patterns.len());
        for p in &patterns {
            params.push(p);
        }
        let purged = stmt
            .execute(params_from_iter(params.iter().copied()))
            .map_err(|e| e.to_string())?;
        Ok(purged as u64)
    }

    /// Bulk-load mode: the FTS triggers are dropped so mass inserts skip the
    /// per-row trigram indexing; `end_bulk_load` restores them and rebuilds the
    /// FTS index in one efficient pass. Reference-counted so parallel volume
    /// scans can nest safely - the rebuild happens only when the last scan
    /// finishes. Watcher events are buffered while scans run, so nothing writes
    /// through the FTS triggers in the meantime.
    pub fn begin_bulk_load(&self) -> Result<(), String> {
        let mut depth = self.bulk_load_depth.lock().map_err(|e| e.to_string())?;
        if *depth == 0 {
            let conn = self.writer.lock().map_err(|e| e.to_string())?;
            conn.execute_batch(
                "DROP TRIGGER IF EXISTS files_ai;
                 DROP TRIGGER IF EXISTS files_ad;
                 DROP TRIGGER IF EXISTS files_au;",
            )
            .map_err(|e| e.to_string())?;
        }
        *depth += 1;
        Ok(())
    }

    pub fn end_bulk_load(&self) -> Result<(), String> {
        let mut depth = self.bulk_load_depth.lock().map_err(|e| e.to_string())?;
        if *depth == 0 {
            return Ok(());
        }
        *depth -= 1;
        if *depth == 0 {
            let conn = self.writer.lock().map_err(|e| e.to_string())?;
            conn.execute_batch(
                "CREATE TRIGGER IF NOT EXISTS files_ai AFTER INSERT ON files BEGIN
                    INSERT INTO file_fts(rowid, name) VALUES (new.id, new.name);
                 END;
                 CREATE TRIGGER IF NOT EXISTS files_ad AFTER DELETE ON files BEGIN
                    INSERT INTO file_fts(file_fts, rowid, name) VALUES ('delete', old.id, old.name);
                 END;
                 CREATE TRIGGER IF NOT EXISTS files_au AFTER UPDATE OF name ON files BEGIN
                    INSERT INTO file_fts(file_fts, rowid, name) VALUES ('delete', old.id, old.name);
                    INSERT INTO file_fts(rowid, name) VALUES (new.id, new.name);
                 END;
                 INSERT INTO file_fts(file_fts) VALUES('rebuild');",
            )
            .map_err(|e| e.to_string())?;
        }
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

    /// Upserts a batch of scanned files. Returns the number of rows actually
    /// changed (inserts + modified updates), which callers use to keep
    /// in-memory counters accurate without ever scanning the table.
    pub fn insert_batch(
        &self,
        volume_id: &str,
        generation: u64,
        items: &[ScannedItem],
    ) -> Result<u64, String> {
        if items.is_empty() {
            return Ok(0);
        }
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut changed = 0u64;
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
                let rows = stmt
                    .execute(params![
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
                changed += rows as u64;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(changed)
    }

    /// Drops items whose (mtime, size) already match the index, so sweeps only
    /// write rows that actually changed. One indexed lookup per batch.
    pub fn filter_changed(
        &self,
        volume_id: &str,
        items: &[ScannedItem],
    ) -> Result<Vec<ScannedItem>, String> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let mut existing: HashSet<(String, i64, i64)> = HashSet::with_capacity(items.len());

        for chunk in items.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT normalized_path, modified_at, size FROM files
                 WHERE volume_id = ?1 AND normalized_path IN ({placeholders});"
            );
            let mut stmt = conn.prepare_cached(&sql).map_err(|e| e.to_string())?;
            let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
            params.push(&volume_id);
            for item in chunk {
                params.push(&item.normalized_path);
            }
            let rows = stmt
                .query_map(params_from_iter(params.iter().copied()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows.flatten() {
                existing.insert(row);
            }
        }

        Ok(items
            .iter()
            .filter(|item| {
                !existing.contains(&(
                    item.normalized_path.clone(),
                    item.modified_at as i64,
                    item.size as i64,
                ))
            })
            .cloned()
            .collect())
    }

    /// Removes index rows for children of `dir` that no longer exist on disk.
    /// Directories are deleted with their whole subtree. Runs for every walked
    /// directory, which is what keeps deletions out of the index.
    pub fn prune_removed_children(
        &self,
        volume_id: &str,
        dir_display: &str,
        seen: &HashSet<String>,
    ) -> Result<(), String> {
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let mut existing: Vec<(String, bool)> = Vec::new();
        {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT normalized_path, is_directory FROM files
                     WHERE volume_id = ?1 AND parent = ?2;",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![volume_id, dir_display], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)? != 0))
                })
                .map_err(|e| e.to_string())?;
            for row in rows.flatten() {
                existing.push(row);
            }
        }
        drop(conn);

        let missing: Vec<(String, bool)> = existing
            .into_iter()
            .filter(|(path, _)| !seen.contains(path))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }

        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (path, is_dir) in &missing {
            if *is_dir {
                let prefix = escape_like_pattern(&format!("{path}\\"));
                tx.execute(
                    "DELETE FROM files WHERE volume_id = ?1 AND (normalized_path = ?2 OR normalized_path LIKE ?3 || '%' ESCAPE '!');",
                    params![volume_id, path, prefix],
                )
                .map_err(|e| e.to_string())?;
            } else {
                tx.execute(
                    "DELETE FROM files WHERE volume_id = ?1 AND normalized_path = ?2;",
                    params![volume_id, path],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Marks a volume scan complete. Removed entries are handled incrementally
    /// by `prune_removed_children` during the walk, so there is no generation
    /// sweep here.
    pub fn finish_volume_scan(
        &self,
        volume_id: &str,
        generation: u64,
        total_scanned: u64,
    ) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        conn.execute(
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
        Ok(())
    }

    pub fn get_scanned_entries(&self, volume_id: &str) -> Result<u64, String> {
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let count: i64 = conn
            .query_row(
                "SELECT scanned_entries FROM volumes WHERE volume_id = ?1;",
                params![volume_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count.max(0) as u64)
    }

    /// True when the volume was fully scanned within `max_age_secs` - its
    /// persisted index is considered ready and startup serves it as-is.
    pub fn is_volume_fresh(&self, volume_id: &str, max_age_secs: u64) -> Result<bool, String> {
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let last_scanned: i64 = conn
            .query_row(
                "SELECT last_scanned_at FROM volumes WHERE volume_id = ?1;",
                params![volume_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if last_scanned <= 0 {
            return Ok(false);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Ok(now - last_scanned <= max_age_secs as i64)
    }

    /// All persisted (volume_id, mount_path) pairs.
    pub fn get_volume_ids(&self) -> Result<Vec<(String, String)>, String> {
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT volume_id, mount_path FROM volumes;")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows.flatten() {
            out.push(row);
        }
        Ok(out)
    }

    /// Removes a volume and all of its indexed rows (used to drop duplicate
    /// volume identities from earlier builds).
    pub fn remove_volume(&self, volume_id: &str) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM files WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM volumes WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Upserts a single file (watcher path). Returns the row delta: +1 when a
    /// row was inserted or changed, 0 when the stored values already match.
    pub fn add_or_update_file(&self, volume_id: &str, item: &ScannedItem) -> Result<i64, String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
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
        Ok(changed as i64)
    }

    /// Removes a file or a directory subtree. Returns the number of rows
    /// removed (negative delta for counters).
    pub fn remove_file(
        &self,
        volume_id: &str,
        normalized_path: &str,
        is_dir: bool,
    ) -> Result<i64, String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        let removed = if is_dir {
            let prefix = escape_like_pattern(&format!("{normalized_path}\\"));
            conn.execute(
                "DELETE FROM files WHERE volume_id = ?1 AND (normalized_path = ?2 OR normalized_path LIKE ?3 || '%' ESCAPE '!');",
                params![volume_id, normalized_path, prefix],
            )
            .map_err(|e| e.to_string())?
        } else {
            conn.execute(
                "DELETE FROM files WHERE volume_id = ?1 AND normalized_path = ?2;",
                params![volume_id, normalized_path],
            )
            .map_err(|e| e.to_string())?
        };
        Ok(removed as i64)
    }

    /// Renames a file or directory subtree (watcher path). Returns the net row
    /// delta for counters.
    pub fn rename_file(
        &self,
        volume_id: &str,
        old_normalized: &str,
        new_item: &ScannedItem,
    ) -> Result<i64, String> {
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        if new_item.is_directory {
            let old_prefix = format!("{old_normalized}\\");
            let old_prefix_like = escape_like_pattern(&old_prefix);
            let new_prefix = format!("{}\\", new_item.normalized_path);
            let old_display_prefix = format!("{}\\", old_normalized);
            let new_display_prefix = format!("{}\\", new_item.display_path);

            // Update children paths
            tx.execute(
                "UPDATE files SET
                    normalized_path = ?1 || SUBSTR(normalized_path, ?2),
                    display_path = ?3 || SUBSTR(display_path, ?4),
                    parent = ?3 || SUBSTR(parent, ?4)
                 WHERE volume_id = ?5 AND normalized_path LIKE ?6 || '%' ESCAPE '!';",
                params![
                    new_prefix,
                    (old_prefix.len() + 1) as i64,
                    new_display_prefix,
                    (old_display_prefix.len() + 1) as i64,
                    volume_id,
                    old_prefix_like,
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        // Delete old entry
        let removed = tx
            .execute(
                "DELETE FROM files WHERE volume_id = ?1 AND normalized_path = ?2;",
                params![volume_id, old_normalized],
            )
            .map_err(|e| e.to_string())?;

        // Insert new entry
        let inserted = tx
            .execute(
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
        Ok(inserted as i64 - removed as i64)
    }

    pub fn get_volume_coverages(&self) -> Result<Vec<VolumeCoverage>, String> {
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT mount_path, state, scanned_entries
                 FROM volumes;",
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

    /// Drops every indexed row and resets volumes so a rebuild starts clean.
    /// FTS is repopulated by `end_bulk_load`'s rebuild pass.
    pub fn clear_catalog(&self) -> Result<(), String> {
        self.begin_bulk_load()?;
        let result = (|| {
            let conn = self.writer.lock().map_err(|e| e.to_string())?;
            conn.execute_batch(
                "DELETE FROM files;
                 UPDATE volumes SET scanned_entries = 0, state = 'error', last_scanned_at = 0;",
            )
            .map_err(|e| e.to_string())
        })();
        self.end_bulk_load()?;
        result
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

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_")
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
