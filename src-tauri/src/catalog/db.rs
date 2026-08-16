use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, params_from_iter, Connection, OpenFlags};

use super::ntfs::{resolve_path, PathNode, PathResolution};
use super::types::{
    CandidateEntry, JournalCheckpoint, NtfsChange, NtfsNode, ScannedItem, VolumeCoverage,
    VolumeInfo, VolumeState,
};

#[allow(dead_code)]
const SCHEMA_VERSION: u32 = 6;

/// One-time purge patterns for rows created by the 0.9.0 catalog, which indexed
/// everything including high-noise directories. Historical artifact of the
/// v1-v3 migrations only: the current policy is full coverage (system and
/// dependency directories are indexed and down-ranked at query time - see
/// `search::path_penalty`), so nothing purges these rows anymore. Patterns are
/// lowercase because `normalized_path` is stored lowercase.
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
    /// Set by any write that lands while the FTS triggers are dropped. The
    /// bulk-load teardown only rebuilds the FTS index when this is set, so a
    /// sweep that changed nothing (the common overflow-reconcile case) does
    /// not re-tokenize millions of names for nothing.
    bulk_load_dirty: AtomicBool,
}

impl Database {
    pub fn open(db_path: &Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let database = match Self::try_open(db_path) {
            Ok(db) => Ok(db),
            Err(err) => {
                let recovery_path = db_path.with_extension(format!(
                    "recovery-{}.db",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ));
                let preserved = std::fs::rename(db_path, &recovery_path).is_ok();
                if preserved {
                    let recovery_wal = recovery_path.with_extension("db-wal");
                    let _ = std::fs::rename(db_path.with_extension("db-wal"), &recovery_wal);
                    let recovery_shm = recovery_path.with_extension("db-shm");
                    let _ = std::fs::rename(db_path.with_extension("db-shm"), &recovery_shm);
                }
                eprintln!(
                    "[Prism Catalog] Failed to open database ({err}), recreating cleanly; preserved_old_catalog={preserved} path={}",
                    recovery_path.display()
                );
                let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
                let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
                Self::try_open(db_path).map_err(|e| format!("database recreation failed: {e}"))
            }
        }?;
        // A preserved recovery copy is a full catalog snapshot (gigabytes on
        // long-lived installs). Once the replacement opens cleanly, copies
        // older than a week are dead weight; drop them opportunistically.
        if let Some(parent) = db_path.parent() {
            cleanup_stale_recovery_copies(parent);
        }
        Ok(database)
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
            bulk_load_dirty: AtomicBool::new(false),
        };
        db.migrate()?;
        Ok(db)
    }

    fn configure_pragmas(conn: &Connection) -> Result<(), String> {
        // The reader and writer connections each cap at 32 MiB of page cache;
        // hot lookups (B-tree roots, FTS postings) fit well below that, while
        // mmap keeps larger scans off the process heap.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size = -32000;
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
                scan_generation INTEGER NOT NULL DEFAULT 0,
                backend TEXT NOT NULL DEFAULT 'fallback',
                journal_id TEXT,
                next_usn INTEGER,
                index_generation INTEGER NOT NULL DEFAULT 0,
                index_status TEXT NOT NULL DEFAULT 'needs_rebuild'
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
             END;

             CREATE TABLE IF NOT EXISTS ntfs_nodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                volume_id TEXT NOT NULL,
                frn BLOB NOT NULL CHECK(length(frn) = 8),
                parent_frn BLOB NOT NULL CHECK(length(parent_frn) = 8),
                name TEXT NOT NULL,
                lower_name TEXT NOT NULL,
                extension TEXT,
                is_directory INTEGER NOT NULL,
                attributes INTEGER NOT NULL,
                modified_at INTEGER NOT NULL DEFAULT 0,
                size INTEGER NOT NULL DEFAULT 0,
                generation INTEGER NOT NULL,
                UNIQUE(volume_id, frn)
             );
             CREATE INDEX IF NOT EXISTS idx_ntfs_nodes_name ON ntfs_nodes(lower_name, is_directory);
             CREATE INDEX IF NOT EXISTS idx_ntfs_nodes_parent ON ntfs_nodes(volume_id, parent_frn);

             CREATE TABLE IF NOT EXISTS ntfs_staging (
                volume_id TEXT NOT NULL,
                generation INTEGER NOT NULL,
                frn BLOB NOT NULL CHECK(length(frn) = 8),
                parent_frn BLOB NOT NULL CHECK(length(parent_frn) = 8),
                name TEXT NOT NULL,
                lower_name TEXT NOT NULL,
                extension TEXT,
                is_directory INTEGER NOT NULL,
                attributes INTEGER NOT NULL,
                modified_at INTEGER NOT NULL DEFAULT 0,
                size INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(volume_id, generation, frn)
             ) WITHOUT ROWID;

             CREATE VIRTUAL TABLE IF NOT EXISTS ntfs_fts USING fts5(
                name,
                tokenize='trigram',
                content='ntfs_nodes',
                content_rowid='id'
             );
             CREATE TRIGGER IF NOT EXISTS ntfs_nodes_ai AFTER INSERT ON ntfs_nodes BEGIN
                INSERT INTO ntfs_fts(rowid, name) VALUES (new.id, new.name);
             END;
             CREATE TRIGGER IF NOT EXISTS ntfs_nodes_ad AFTER DELETE ON ntfs_nodes BEGIN
                INSERT INTO ntfs_fts(ntfs_fts, rowid, name) VALUES ('delete', old.id, old.name);
             END;
             CREATE TRIGGER IF NOT EXISTS ntfs_nodes_au AFTER UPDATE OF name ON ntfs_nodes BEGIN
                INSERT INTO ntfs_fts(ntfs_fts, rowid, name) VALUES ('delete', old.id, old.name);
                INSERT INTO ntfs_fts(rowid, name) VALUES (new.id, new.name);
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
        // The version row is stored as TEXT; read it as a string and parse so
        // a type mismatch can never masquerade as version 0 (which made every
        // open replay the v5 ALTER TABLE, fail on the duplicate column, and
        // drop into the recovery path that discards the whole catalog).
        let mut version: i64 = conn
            .query_row("SELECT value FROM meta WHERE key = 'version';", [], |row| {
                row.get::<_, String>(0)
            })
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0);

        if version >= SCHEMA_VERSION as i64 {
            return Ok(());
        }

        if version == 1 || version == 2 || version == 3 {
            eprintln!(
                "[Prism Catalog] Migrating catalog schema v{version} -> v4 (purging excluded paths)"
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
            version = 4;
        } else {
            drop(conn);
        }

        if version < 5 {
            let conn = self.writer.lock().map_err(|e| e.to_string())?;
            eprintln!(
                "[Prism Catalog] Migrating catalog schema v{version} -> v5 (NTFS FRN catalog)"
            );
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE volumes ADD COLUMN backend TEXT NOT NULL DEFAULT 'fallback';
                 ALTER TABLE volumes ADD COLUMN journal_id TEXT;
                 ALTER TABLE volumes ADD COLUMN next_usn INTEGER;
                 ALTER TABLE volumes ADD COLUMN index_generation INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE volumes ADD COLUMN index_status TEXT NOT NULL DEFAULT 'needs_rebuild';
                 UPDATE volumes SET backend = 'fallback', index_status = 'needs_rebuild';
                 UPDATE meta SET value = '5' WHERE key = 'version';
                 COMMIT;",
            )
            .map_err(|error| {
                let _ = conn.execute_batch("ROLLBACK;");
                error.to_string()
            })?;
        }

        if version < 6 {
            let conn = self.writer.lock().map_err(|e| e.to_string())?;
            eprintln!(
                "[Prism Catalog] Migrating catalog schema v{version} -> v6 (full-coverage resweep)"
            );
            // The scanner is full-coverage now: directories it used to skip
            // (C:\Windows, node_modules, .git, temp, ...) must enter the
            // catalog. NTFS generations already contain them - the MFT ingest
            // was never exclusion-filtered - so only fallback volumes need
            // their scan clock reset to trigger one resweep.
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 UPDATE volumes SET last_scanned_at = 0 WHERE backend = 'fallback';
                 UPDATE meta SET value = '6' WHERE key = 'version';
                 COMMIT;",
            )
            .map_err(|error| {
                let _ = conn.execute_batch("ROLLBACK;");
                error.to_string()
            })?;
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
            self.bulk_load_dirty.store(false, Ordering::Relaxed);
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
            let rebuild_needed = self.bulk_load_dirty.swap(false, Ordering::Relaxed);
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
                 END;",
            )
            .map_err(|e| e.to_string())?;
            if rebuild_needed {
                conn.execute_batch("INSERT INTO file_fts(file_fts) VALUES('rebuild');")
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Marks the FTS index as out of date: the row was written while the
    /// maintenance triggers were dropped, so the bulk-load teardown must
    /// rebuild it. Harmless outside bulk loads (nothing reads the flag then).
    fn note_bulk_write(&self) {
        self.bulk_load_dirty.store(true, Ordering::Relaxed);
    }

    /// Truncates the write-ahead log. A long sweep grows the WAL by hundreds
    /// of megabytes, and every process start pays recovery for whatever was
    /// left un-checkpointed - checkpointing after big writes keeps launches
    /// cheap and the app data directory small.
    pub fn checkpoint(&self) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())
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

    pub fn get_ntfs_checkpoint(
        &self,
        volume_id: &str,
    ) -> Result<Option<JournalCheckpoint>, String> {
        let conn = self.reader.lock().map_err(|e| e.to_string())?;
        let state = conn.query_row(
            "SELECT journal_id, next_usn FROM volumes
             WHERE volume_id = ?1 AND backend = 'ntfs' AND index_status = 'ready';",
            params![volume_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            },
        );
        match state {
            Ok((Some(journal_id), Some(next_usn))) => {
                Ok(journal_id
                    .parse::<u64>()
                    .ok()
                    .map(|journal_id| JournalCheckpoint {
                        journal_id,
                        next_usn,
                    }))
            }
            Ok(_) | Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn begin_ntfs_rebuild(&self, volume_id: &str) -> Result<u64, String> {
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let generation: i64 = tx
            .query_row(
                "SELECT index_generation + 1 FROM volumes WHERE volume_id = ?1;",
                params![volume_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM ntfs_staging WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE volumes SET index_status = 'rebuilding', state = 'indexing'
             WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(generation.max(1) as u64)
    }

    pub fn insert_ntfs_staging(
        &self,
        volume_id: &str,
        generation: u64,
        nodes: &[NtfsNode],
    ) -> Result<u64, String> {
        if nodes.is_empty() {
            return Ok(0);
        }
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut inserted = 0u64;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO ntfs_staging(
                        volume_id, generation, frn, parent_frn, name, lower_name,
                        extension, is_directory, attributes, modified_at, size
                     ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(volume_id, generation, frn) DO UPDATE SET
                        parent_frn = excluded.parent_frn,
                        name = excluded.name,
                        lower_name = excluded.lower_name,
                        extension = excluded.extension,
                        is_directory = excluded.is_directory,
                        attributes = excluded.attributes,
                        modified_at = excluded.modified_at,
                        size = excluded.size;",
                )
                .map_err(|e| e.to_string())?;
            for node in nodes {
                inserted += stmt
                    .execute(params![
                        volume_id,
                        generation as i64,
                        frn_blob(node.frn),
                        frn_blob(node.parent_frn),
                        node.name,
                        node.lower_name,
                        node.extension,
                        node.is_directory as i32,
                        node.attributes as i64,
                        node.modified_at,
                        node.size as i64,
                    ])
                    .map_err(|e| e.to_string())? as u64;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(inserted)
    }

    pub fn abort_ntfs_rebuild(&self, volume_id: &str, generation: u64) -> Result<(), String> {
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM ntfs_staging WHERE volume_id = ?1 AND generation = ?2;",
            params![volume_id, generation as i64],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE volumes SET index_status = 'needs_rebuild', state = 'error'
             WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn finish_ntfs_rebuild(
        &self,
        volume_id: &str,
        generation: u64,
        checkpoint: JournalCheckpoint,
        total: u64,
    ) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        let now = unix_timestamp();
        let journal_id = checkpoint.journal_id.to_string();
        conn.execute_batch("BEGIN IMMEDIATE;")
            .map_err(|e| e.to_string())?;
        let result = (|| {
            conn.execute_batch(
                "DROP TRIGGER IF EXISTS ntfs_nodes_ai;
                 DROP TRIGGER IF EXISTS ntfs_nodes_ad;
                 DROP TRIGGER IF EXISTS ntfs_nodes_au;",
            )?;
            conn.execute(
                "DELETE FROM ntfs_nodes WHERE volume_id = ?1;",
                params![volume_id],
            )?;
            conn.execute(
                "INSERT INTO ntfs_nodes(
                    volume_id, frn, parent_frn, name, lower_name, extension,
                    is_directory, attributes, modified_at, size, generation
                 )
                 SELECT volume_id, frn, parent_frn, name, lower_name, extension,
                        is_directory, attributes, modified_at, size, generation
                 FROM ntfs_staging WHERE volume_id = ?1 AND generation = ?2;",
                params![volume_id, generation as i64],
            )?;
            conn.execute(
                "DELETE FROM files WHERE volume_id = ?1;",
                params![volume_id],
            )?;
            conn.execute(
                "DELETE FROM ntfs_staging WHERE volume_id = ?1;",
                params![volume_id],
            )?;
            conn.execute_batch(
                "INSERT INTO ntfs_fts(ntfs_fts) VALUES('rebuild');
                 CREATE TRIGGER ntfs_nodes_ai AFTER INSERT ON ntfs_nodes BEGIN
                    INSERT INTO ntfs_fts(rowid, name) VALUES (new.id, new.name);
                 END;
                 CREATE TRIGGER ntfs_nodes_ad AFTER DELETE ON ntfs_nodes BEGIN
                    INSERT INTO ntfs_fts(ntfs_fts, rowid, name) VALUES ('delete', old.id, old.name);
                 END;
                 CREATE TRIGGER ntfs_nodes_au AFTER UPDATE OF name ON ntfs_nodes BEGIN
                    INSERT INTO ntfs_fts(ntfs_fts, rowid, name) VALUES ('delete', old.id, old.name);
                    INSERT INTO ntfs_fts(rowid, name) VALUES (new.id, new.name);
                 END;",
            )?;
            conn.execute(
                "UPDATE volumes SET
                    backend = 'ntfs', journal_id = ?1, next_usn = ?2,
                    index_generation = ?3, index_status = 'ready', state = 'ready',
                    scanned_entries = ?4, last_scanned_at = ?5
                 WHERE volume_id = ?6;",
                params![
                    journal_id,
                    checkpoint.next_usn,
                    generation as i64,
                    total as i64,
                    now,
                    volume_id
                ],
            )?;
            Ok::<(), rusqlite::Error>(())
        })();
        match result {
            Ok(()) => conn.execute_batch("COMMIT;").map_err(|e| e.to_string()),
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(error.to_string())
            }
        }
    }

    pub fn apply_ntfs_changes(
        &self,
        volume_id: &str,
        journal_id: u64,
        next_usn: i64,
        changes: &[NtfsChange],
    ) -> Result<(), String> {
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let saved: Option<(Option<String>, Option<i64>)> = tx
            .query_row(
                "SELECT journal_id, next_usn FROM volumes
                 WHERE volume_id = ?1 AND backend = 'ntfs';",
                params![volume_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        let Some((Some(saved_id), Some(saved_usn))) = saved else {
            return Err(
                "cannot apply journal changes without an active NTFS generation".to_string(),
            );
        };
        if saved_id != journal_id.to_string() {
            return Err("journal ID changed while applying a batch".to_string());
        }
        if next_usn <= saved_usn {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(());
        }

        let generation: i64 = tx
            .query_row(
                "SELECT index_generation FROM volumes WHERE volume_id = ?1;",
                params![volume_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let mut delta = 0i64;
        for change in changes {
            match change {
                NtfsChange::Upsert(node) => {
                    let exists: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM ntfs_nodes WHERE volume_id = ?1 AND frn = ?2);",
                            params![volume_id, frn_blob(node.frn)],
                            |row| row.get(0),
                        )
                        .map_err(|e| e.to_string())?;
                    tx.execute(
                        "INSERT INTO ntfs_nodes(
                            volume_id, frn, parent_frn, name, lower_name, extension,
                            is_directory, attributes, modified_at, size, generation
                         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                         ON CONFLICT(volume_id, frn) DO UPDATE SET
                            parent_frn = excluded.parent_frn,
                            name = excluded.name,
                            lower_name = excluded.lower_name,
                            extension = excluded.extension,
                            is_directory = excluded.is_directory,
                            attributes = excluded.attributes,
                            modified_at = excluded.modified_at,
                            size = excluded.size;",
                        params![
                            volume_id,
                            frn_blob(node.frn),
                            frn_blob(node.parent_frn),
                            node.name,
                            node.lower_name,
                            node.extension,
                            node.is_directory as i32,
                            node.attributes as i64,
                            node.modified_at,
                            node.size as i64,
                            generation,
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                    if !exists {
                        delta += 1;
                    }
                }
                NtfsChange::Delete { frn } => {
                    // A directory delete can be represented by one journal
                    // record while descendants are already inaccessible. Walk
                    // only the persisted parent graph, bounded by a set, so
                    // orphaned/cyclic metadata cannot recurse forever.
                    let mut doomed = vec![*frn];
                    let mut seen = HashSet::new();
                    let mut cursor = 0usize;
                    {
                        // One prepared statement for the whole descent, not
                        // one per visited node.
                        let mut children = tx
                            .prepare_cached(
                                "SELECT frn FROM ntfs_nodes
                                 WHERE volume_id = ?1 AND parent_frn = ?2;",
                            )
                            .map_err(|e| e.to_string())?;
                        while cursor < doomed.len() {
                            let current = doomed[cursor];
                            cursor += 1;
                            if !seen.insert(current) {
                                continue;
                            }
                            let rows = children
                                .query_map(params![volume_id, frn_blob(current)], |row| {
                                    let blob: Vec<u8> = row.get(0)?;
                                    Ok(blob_frn(&blob))
                                })
                                .map_err(|e| e.to_string())?;
                            for child in rows.flatten().flatten() {
                                doomed.push(child);
                            }
                        }
                    }
                    for doomed_frn in doomed {
                        delta -= tx
                            .execute(
                                "DELETE FROM ntfs_nodes WHERE volume_id = ?1 AND frn = ?2;",
                                params![volume_id, frn_blob(doomed_frn)],
                            )
                            .map_err(|e| e.to_string())? as i64;
                    }
                }
            }
        }
        tx.execute(
            "UPDATE volumes SET next_usn = ?1,
                scanned_entries = MAX(0, scanned_entries + ?2),
                index_status = 'ready', state = 'ready'
             WHERE volume_id = ?3 AND journal_id = ?4;",
            params![next_usn, delta, volume_id, journal_id.to_string()],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn mark_ntfs_ready(&self, volume_id: &str) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        // The where clause keeps the steady-state poll (journal already
        // current) a read-only no-op instead of a write per volume per tick.
        conn.execute(
            "UPDATE volumes SET state = 'ready', index_status = 'ready'
             WHERE volume_id = ?1 AND backend = 'ntfs'
               AND (state <> 'ready' OR index_status <> 'ready');",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn mark_directory_backend(&self, volume_id: &str) -> Result<(), String> {
        let conn = self.writer.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE volumes SET backend = 'fallback', journal_id = NULL,
                next_usn = NULL, index_status = 'needs_rebuild'
             WHERE volume_id = ?1;",
            params![volume_id],
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
        if changed > 0 {
            self.note_bulk_write();
        }
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
        self.note_bulk_write();
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
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let now = unix_timestamp();

        // Keep an old NTFS generation searchable until the fallback walk has
        // completed. The handover happens here, never at scan start.
        tx.execute(
            "DELETE FROM ntfs_nodes WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE volumes SET
                state = 'ready',
                scanned_entries = ?1,
                last_scanned_at = ?2,
                scan_generation = ?3,
                backend = 'fallback',
                journal_id = NULL,
                next_usn = NULL,
                index_status = 'ready'
             WHERE volume_id = ?4;",
            params![total_scanned as i64, now, generation as i64, volume_id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
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
        let mut conn = self.writer.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM files WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM ntfs_nodes WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM ntfs_staging WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM volumes WHERE volume_id = ?1;",
            params![volume_id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
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
        if removed != 0 {
            self.note_bulk_write();
        }
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
        self.note_bulk_write();
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
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM files f
                     WHERE NOT EXISTS(
                        SELECT 1 FROM volumes v
                        WHERE v.volume_id = f.volume_id AND v.backend = 'ntfs'
                     ))
                  + (SELECT COUNT(*) FROM ntfs_nodes n
                     JOIN volumes v ON v.volume_id = n.volume_id
                     WHERE v.backend = 'ntfs');",
                [],
                |row| row.get(0),
            )
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
                 DELETE FROM ntfs_nodes;
                 DELETE FROM ntfs_staging;
                 INSERT INTO ntfs_fts(ntfs_fts) VALUES('rebuild');
                 UPDATE volumes SET scanned_entries = 0, state = 'error', last_scanned_at = 0,
                    backend = 'fallback', journal_id = NULL, next_usn = NULL,
                    index_status = 'needs_rebuild';",
            )
            .map_err(|e| e.to_string())?;
            self.note_bulk_write();
            Ok(())
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

        // Full coverage means common names (readme.md, config.json) have
        // thousands of hits across system and dependency directories. The
        // location-based ranking happens after fetch, so the candidate pools
        // must be wide enough that a user's own file is among them before
        // penalties can order it to the top.
        let exact_limit = 40i64;
        let prefix_limit = (limit.max(10) * 3) as i64;
        let fts_limit = (limit.max(10) * 3) as i64;

        // 1. Exact match query
        {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, display_path, lower_name, is_directory, extension
                     FROM files f WHERE lower_name = ?1
                       AND NOT EXISTS(SELECT 1 FROM volumes v
                                      WHERE v.volume_id = f.volume_id AND v.backend = 'ntfs')
                     LIMIT ?2;",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![lower, exact_limit], |row| {
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
                     FROM files f WHERE lower_name >= ?1 AND lower_name <= ?2
                       AND NOT EXISTS(SELECT 1 FROM volumes v
                                      WHERE v.volume_id = f.volume_id AND v.backend = 'ntfs')
                     LIMIT ?3;",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![lower, prefix_end, prefix_limit], |row| {
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
                         FROM files f
                         WHERE id IN (
                            SELECT rowid FROM file_fts WHERE name MATCH ?1 LIMIT ?2
                         )
                         AND NOT EXISTS(SELECT 1 FROM volumes v
                                        WHERE v.volume_id = f.volume_id AND v.backend = 'ntfs')
                         LIMIT ?2;",
                    )
                    .map_err(|e| e.to_string())?;

                let rows = stmt
                    .query_map(params![fts_query, fts_limit], |row| {
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
                     FROM files f WHERE lower_name >= ?1 AND lower_name <= ?2
                       AND NOT EXISTS(SELECT 1 FROM volumes v
                                      WHERE v.volume_id = f.volume_id AND v.backend = 'ntfs')
                     LIMIT ?3;",
                )
                .map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map(params![lower, prefix_end, prefix_limit], |row| {
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

        append_ntfs_candidates(&conn, &lower, query_len, limit, &mut candidates)?;

        Ok(candidates)
    }
}

#[derive(Clone)]
struct NtfsCandidateRow {
    id: i64,
    volume_id: String,
    frn: u64,
    lower_name: String,
    is_directory: bool,
    extension: Option<String>,
    mount_path: String,
}

fn append_ntfs_candidates(
    conn: &Connection,
    lower: &str,
    query_len: usize,
    limit: usize,
    candidates: &mut Vec<CandidateEntry>,
) -> Result<(), String> {
    let mut rows = Vec::with_capacity(limit * 2);
    let mut seen = HashSet::new();

    let mut collect = |sql: &str, values: &[&dyn rusqlite::ToSql]| -> Result<(), String> {
        // Cached: this runs on every keystroke-driven search.
        let mut stmt = conn.prepare_cached(sql).map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(params_from_iter(values.iter().copied()), |row| {
                let blob: Vec<u8> = row.get(2)?;
                let frn = blob_frn(&blob).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Blob,
                        "invalid FRN blob".into(),
                    )
                })?;
                Ok(NtfsCandidateRow {
                    id: row.get(0)?,
                    volume_id: row.get(1)?,
                    frn,
                    lower_name: row.get(3)?,
                    is_directory: row.get::<_, i32>(4)? != 0,
                    extension: row.get(5)?,
                    mount_path: row.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?;
        for row in mapped.flatten() {
            if seen.insert(row.id) {
                rows.push(row);
            }
        }
        Ok(())
    };

    // Keep the NTFS candidate pools aligned with the fallback-table queries
    // above: ranking happens after fetch, so both need the wider limits.
    let exact_limit = 40i64;
    collect(
        "SELECT n.id, n.volume_id, n.frn, n.lower_name, n.is_directory, n.extension, v.mount_path
         FROM ntfs_nodes n JOIN volumes v ON v.volume_id = n.volume_id
         WHERE v.backend = 'ntfs' AND n.lower_name = ?1 LIMIT ?2;",
        &[&lower, &exact_limit],
    )?;
    let prefix_end = format!("{lower}\u{FFFF}");
    let sql_limit = (limit.max(10) * 3) as i64;
    collect(
        "SELECT n.id, n.volume_id, n.frn, n.lower_name, n.is_directory, n.extension, v.mount_path
         FROM ntfs_nodes n JOIN volumes v ON v.volume_id = n.volume_id
         WHERE v.backend = 'ntfs' AND n.lower_name >= ?1 AND n.lower_name <= ?2 LIMIT ?3;",
        &[&lower, &prefix_end, &sql_limit],
    )?;
    if query_len >= 3 {
        let fts_query = sanitize_fts5_trigram_query(lower);
        if !fts_query.is_empty() {
            let fts_limit = (limit.max(10) * 3) as i64;
            collect(
                "SELECT n.id, n.volume_id, n.frn, n.lower_name, n.is_directory, n.extension, v.mount_path
                 FROM ntfs_nodes n JOIN volumes v ON v.volume_id = n.volume_id
                 WHERE v.backend = 'ntfs' AND n.id IN (
                    SELECT rowid FROM ntfs_fts WHERE name MATCH ?1 LIMIT ?2
                 ) LIMIT ?2;",
                &[&fts_query, &fts_limit],
            )?;
        }
    }

    let mut node_cache: std::collections::HashMap<(String, u64), PathNode> =
        std::collections::HashMap::new();
    let mut path_cache: std::collections::HashMap<(String, u64), Option<String>> =
        std::collections::HashMap::new();
    let mut lookup = conn
        .prepare_cached(
            "SELECT frn, parent_frn, name FROM ntfs_nodes
             WHERE volume_id = ?1 AND frn = ?2;",
        )
        .map_err(|e| e.to_string())?;

    for row in rows {
        let cache_key = (row.volume_id.clone(), row.frn);
        let display_path = if let Some(cached) = path_cache.get(&cache_key) {
            cached.clone()
        } else {
            let volume_id = row.volume_id.clone();
            let resolution = resolve_path(row.frn, &row.mount_path, |frn| {
                let key = (volume_id.clone(), frn);
                if let Some(node) = node_cache.get(&key) {
                    return Some(node.clone());
                }
                let loaded = lookup
                    .query_row(params![volume_id, frn_blob(frn)], |record| {
                        let frn_blob: Vec<u8> = record.get(0)?;
                        let parent_blob: Vec<u8> = record.get(1)?;
                        let Some(frn) = blob_frn(&frn_blob) else {
                            return Err(rusqlite::Error::InvalidQuery);
                        };
                        let Some(parent_frn) = blob_frn(&parent_blob) else {
                            return Err(rusqlite::Error::InvalidQuery);
                        };
                        Ok(PathNode {
                            frn,
                            parent_frn,
                            name: record.get(2)?,
                        })
                    })
                    .ok()?;
                node_cache.insert(key, loaded.clone());
                Some(loaded)
            });
            let path = match resolution {
                PathResolution::Resolved(path) => Some(path.to_string_lossy().into_owned()),
                PathResolution::Orphaned { .. }
                | PathResolution::Cycle { .. }
                | PathResolution::DepthExceeded => None,
            };
            path_cache.insert(cache_key, path.clone());
            path
        };
        if let Some(display_path) = display_path {
            candidates.push(CandidateEntry {
                id: row.id,
                display_path,
                lower_name: row.lower_name,
                is_directory: row.is_directory,
                extension: row.extension,
            });
        }
    }
    Ok(())
}

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_")
}

/// Removes `catalog.recovery-*.db{,-wal,-shm}` copies older than a week from
/// the app data directory. Only files matching that exact prefix are touched.
fn cleanup_stale_recovery_copies(parent: &Path) {
    const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_recovery = name.starts_with("catalog.recovery-")
            && (name.ends_with(".db") || name.ends_with(".db-wal") || name.ends_with(".db-shm"));
        if !is_recovery {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if SystemTime::now()
            .duration_since(modified)
            .is_ok_and(|age| age > MAX_AGE)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn frn_blob(frn: u64) -> [u8; 8] {
    frn.to_le_bytes()
}

fn blob_frn(blob: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(blob.try_into().ok()?))
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
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

#[cfg(test)]
mod ntfs_tests {
    use super::*;

    fn temp_db_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("prism-{name}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("catalog.db")
    }

    fn volume() -> VolumeInfo {
        VolumeInfo {
            volume_id: "stable-volume".into(),
            drive_letter: Some("C:".into()),
            mount_paths: vec![PathBuf::from("C:\\")],
            drive_type: windows::Win32::System::WindowsProgramming::DRIVE_FIXED,
            label: String::new(),
            fs_type: "NTFS".into(),
        }
    }

    fn node(frn: u64, parent_frn: u64, name: &str, is_directory: bool) -> NtfsNode {
        NtfsNode {
            frn,
            parent_frn,
            name: name.into(),
            lower_name: name.to_lowercase(),
            extension: (!is_directory)
                .then(|| {
                    Path::new(name)
                        .extension()
                        .map(|value| value.to_string_lossy().to_lowercase())
                })
                .flatten(),
            is_directory,
            attributes: if is_directory {
                windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY.0
            } else {
                0
            },
            modified_at: 0,
            size: 0,
        }
    }

    #[test]
    fn ntfs_generation_cursor_and_replayed_events_are_consistent() {
        let path = temp_db_path("ntfs-state");
        let db = Database::open(&path).unwrap();
        db.upsert_volume(&volume(), VolumeState::Indexing).unwrap();
        let generation = db.begin_ntfs_rebuild("stable-volume").unwrap();
        let initial = [
            node(5, 5, ".", true),
            node(10, 5, "Projects", true),
            node(11, 10, "Main.RS", false),
            node(12, 11, "child.txt", false),
        ];
        assert_eq!(
            db.insert_ntfs_staging("stable-volume", generation, &initial)
                .unwrap(),
            4
        );
        db.finish_ntfs_rebuild(
            "stable-volume",
            generation,
            JournalCheckpoint {
                journal_id: u64::MAX,
                next_usn: 100,
            },
            4,
        )
        .unwrap();
        assert_eq!(
            db.get_ntfs_checkpoint("stable-volume").unwrap(),
            Some(JournalCheckpoint {
                journal_id: u64::MAX,
                next_usn: 100
            })
        );
        let candidates = db.search_candidates("main", 20).unwrap();
        assert!(candidates
            .iter()
            .any(|candidate| candidate.display_path == "C:\\Projects\\Main.RS"));

        // The same FRN value is valid on another volume; volume_id is part of
        // the NTFS identity and cannot be collapsed globally.
        let mut other_volume = volume();
        other_volume.volume_id = "other-volume".into();
        other_volume.mount_paths = vec![PathBuf::from("D:\\")];
        other_volume.drive_letter = Some("D:".into());
        db.upsert_volume(&other_volume, VolumeState::Indexing)
            .unwrap();
        let other_generation = db.begin_ntfs_rebuild("other-volume").unwrap();
        db.insert_ntfs_staging(
            "other-volume",
            other_generation,
            &[node(5, 5, ".", true), node(11, 5, "Main.RS", false)],
        )
        .unwrap();
        db.finish_ntfs_rebuild(
            "other-volume",
            other_generation,
            JournalCheckpoint {
                journal_id: 7,
                next_usn: 1,
            },
            2,
        )
        .unwrap();
        let conn = db.reader.lock().unwrap();
        let same_frn: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ntfs_nodes WHERE frn = ?1;",
                params![frn_blob(11)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(same_frn, 2);
        drop(conn);

        let moved = node(11, 5, "lib.rs", false);
        db.apply_ntfs_changes("stable-volume", u64::MAX, 110, &[NtfsChange::Upsert(moved)])
            .unwrap();
        // An already committed cursor makes a replay a no-op, even if its
        // payload describes an older name.
        db.apply_ntfs_changes(
            "stable-volume",
            u64::MAX,
            110,
            &[NtfsChange::Upsert(node(11, 10, "Main.RS", false))],
        )
        .unwrap();
        let candidates = db.search_candidates("lib", 20).unwrap();
        assert_eq!(candidates[0].display_path, "C:\\lib.rs");

        db.apply_ntfs_changes(
            "stable-volume",
            u64::MAX,
            120,
            &[NtfsChange::Delete { frn: 11 }],
        )
        .unwrap();
        assert!(db.search_candidates("lib", 20).unwrap().is_empty());
        db.apply_ntfs_changes(
            "stable-volume",
            u64::MAX,
            130,
            &[NtfsChange::Delete { frn: 10 }],
        )
        .unwrap();
        assert!(db.search_candidates("child", 20).unwrap().is_empty());
        assert_eq!(
            db.get_ntfs_checkpoint("stable-volume")
                .unwrap()
                .unwrap()
                .next_usn,
            130
        );

        drop(db);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stale_recovery_copies_are_cleaned_but_recent_ones_kept() {
        let path = temp_db_path("recovery-cleanup");
        let parent = path.parent().unwrap().to_path_buf();
        {
            let _db = Database::open(&path).unwrap();
        }

        let stale = parent.join("catalog.recovery-1786807317418544300.db");
        std::fs::write(&stale, "old catalog snapshot").unwrap();
        let stale_wal = parent.join("catalog.recovery-1786807317418544300.db-wal");
        std::fs::write(&stale_wal, "wal").unwrap();
        // Age the copy past the retention window (write access is required
        // to set file times on Windows).
        let ancient = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&stale)
            .unwrap()
            .set_modified(ancient)
            .unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&stale_wal)
            .unwrap()
            .set_modified(ancient)
            .unwrap();
        let recent = parent.join("catalog.recovery-9999999999999999999.db");
        std::fs::write(&recent, "recent snapshot").unwrap();
        let unrelated = parent.join("otherfile-recovery-.db");
        std::fs::write(&unrelated, "not a recovery copy").unwrap();

        drop(Database::open(&path).unwrap());

        assert!(!stale.exists(), "week-old recovery copies are removed");
        assert!(!stale_wal.exists(), "their sidecar WAL files too");
        assert!(recent.exists(), "recent recovery copies are retained");
        assert!(
            unrelated.exists(),
            "only catalog recovery files are touched"
        );

        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn migrates_v5_resweeping_only_fallback_volumes() {
        let path = temp_db_path("schema-v5-full-coverage");
        {
            // Build a current-shape catalog, then rewind its version marker to
            // v5 with one fallback and one NTFS volume already scanned.
            let db = Database::open(&path).unwrap();
            db.upsert_volume(&volume(), VolumeState::Ready).unwrap();
            db.finish_volume_scan("stable-volume", 1, 5).unwrap();

            let mut ntfs_volume = volume();
            ntfs_volume.volume_id = "ntfs-volume".into();
            ntfs_volume.mount_paths = vec![PathBuf::from("D:\\")];
            ntfs_volume.drive_letter = Some("D:".into());
            db.upsert_volume(&ntfs_volume, VolumeState::Ready).unwrap();
            let conn = db.writer.lock().unwrap();
            conn.execute("UPDATE meta SET value = '5' WHERE key = 'version';", [])
                .unwrap();
            conn.execute(
                "UPDATE volumes SET backend = 'ntfs', journal_id = '7', next_usn = 100,
                    index_status = 'ready', last_scanned_at = CAST(strftime('%s','now') AS INTEGER) - 60
                 WHERE volume_id = 'ntfs-volume';",
                [],
            )
            .unwrap();
        }

        let db = Database::try_open(&path).unwrap();
        let conn = db.reader.lock().unwrap();
        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'version';", [], |row| {
                row.get(0)
            })
            .unwrap();
        drop(conn);
        assert_eq!(version, "6");
        // Full-coverage resweep: fallback volumes lose their freshness so the
        // next startup re-walks them; NTFS generations already cover those
        // directories through the MFT and keep their cursor.
        assert!(!db.is_volume_fresh("stable-volume", 86_400 * 365).unwrap());
        assert!(db.is_volume_fresh("ntfs-volume", 86_400 * 365).unwrap());

        drop(db);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn migrates_v4_without_destroying_path_catalog_rows() {
        let path = temp_db_path("schema-v4");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES('version', '4');
                 CREATE TABLE volumes(
                    volume_id TEXT PRIMARY KEY, mount_path TEXT NOT NULL,
                    drive_type INTEGER NOT NULL, label TEXT NOT NULL,
                    file_system TEXT NOT NULL, state TEXT NOT NULL,
                    scanned_entries INTEGER NOT NULL DEFAULT 0,
                    last_scanned_at INTEGER NOT NULL DEFAULT 0,
                    scan_generation INTEGER NOT NULL DEFAULT 0
                 );
                 INSERT INTO volumes VALUES('legacy', 'C:\\', 3, '', 'NTFS', 'ready', 1, 1, 1);
                 CREATE TABLE files(
                    id INTEGER PRIMARY KEY AUTOINCREMENT, volume_id TEXT NOT NULL,
                    normalized_path TEXT NOT NULL, display_path TEXT NOT NULL,
                    name TEXT NOT NULL, lower_name TEXT NOT NULL, parent TEXT NOT NULL,
                    is_directory INTEGER NOT NULL, extension TEXT,
                    scan_generation INTEGER NOT NULL, modified_at INTEGER NOT NULL DEFAULT 0,
                    size INTEGER NOT NULL DEFAULT 0, UNIQUE(volume_id, normalized_path)
                 );
                 INSERT INTO files(volume_id, normalized_path, display_path, name, lower_name,
                    parent, is_directory, scan_generation)
                 VALUES('legacy', 'c:\\keep.txt', 'C:\\keep.txt', 'keep.txt', 'keep.txt', 'C:\\', 0, 1);",
            )
            .unwrap();
        }

        let db = Database::try_open(&path).unwrap();
        let conn = db.reader.lock().unwrap();
        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'version';", [], |row| {
                row.get(0)
            })
            .unwrap();
        let legacy_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE volume_id = 'legacy';",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let backend: String = conn
            .query_row(
                "SELECT backend FROM volumes WHERE volume_id = 'legacy';",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "6");
        assert_eq!(legacy_rows, 1);
        assert_eq!(backend, "fallback");
        drop(conn);
        drop(db);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
