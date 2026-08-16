mod mft;
mod path;
mod usn;
mod volume;

use std::time::Instant;

use super::db::Database;
use super::types::{JournalCheckpoint, VolumeInfo};

pub use path::{resolve_path, PathNode, PathResolution};
pub use usn::{journal_continuity, JournalContinuity};
use volume::NtfsTransport;
pub use volume::NtfsVolume;

const INGEST_BATCH_SIZE: usize = 8_192;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub records: u64,
    pub rebuilt: bool,
}

pub struct NtfsBackend {
    transport: Box<dyn NtfsTransport>,
}

impl NtfsBackend {
    pub fn open(info: &VolumeInfo) -> Result<Self, String> {
        Ok(Self {
            transport: Box::new(NtfsVolume::open(info)?),
        })
    }

    pub fn synchronize(&mut self, info: &VolumeInfo, db: &Database) -> Result<SyncStats, String> {
        let query_started = Instant::now();
        let journal = self.transport.query_journal()?;
        // synchronize runs on every poll; only surface journal queries slow
        // enough to matter instead of logging each pass.
        if query_started.elapsed().as_millis() > 50 {
            eprintln!(
                "[Prism Catalog] ntfs_journal_query volume={} elapsed_ms={}",
                info.volume_id,
                query_started.elapsed().as_millis()
            );
        }

        let checkpoint = db.get_ntfs_checkpoint(&info.volume_id)?;
        match journal_continuity(checkpoint, journal) {
            JournalContinuity::Current => {
                db.mark_ntfs_ready(&info.volume_id)?;
                Ok(SyncStats::default())
            }
            JournalContinuity::CatchUp => self.catch_up(info, db, journal.next_usn),
            JournalContinuity::Rebuild(_) => self.rebuild(info, db, journal),
        }
    }

    fn rebuild(
        &mut self,
        info: &VolumeInfo,
        db: &Database,
        journal: super::types::JournalMetadata,
    ) -> Result<SyncStats, String> {
        let generation = db.begin_ntfs_rebuild(&info.volume_id)?;
        let started = Instant::now();
        let mut batch = Vec::with_capacity(INGEST_BATCH_SIZE);
        let mut records = 0u64;
        let mut db_elapsed_ms = 0u128;

        let mut consume = |node| {
            batch.push(node);
            if batch.len() >= INGEST_BATCH_SIZE {
                let db_started = Instant::now();
                records += db.insert_ntfs_staging(&info.volume_id, generation, &batch)?;
                db_elapsed_ms += db_started.elapsed().as_millis();
                batch.clear();
            }
            Ok(())
        };
        let enumeration = self.transport.enumerate_mft(journal.next_usn, &mut consume);

        if let Err(error) = enumeration {
            let _ = db.abort_ntfs_rebuild(&info.volume_id, generation);
            return Err(error);
        }
        if !batch.is_empty() {
            let db_started = Instant::now();
            records += db.insert_ntfs_staging(&info.volume_id, generation, &batch)?;
            db_elapsed_ms += db_started.elapsed().as_millis();
        }

        let finalize_started = Instant::now();
        db.finish_ntfs_rebuild(
            &info.volume_id,
            generation,
            JournalCheckpoint {
                journal_id: journal.journal_id,
                next_usn: journal.next_usn,
            },
            records,
        )?;
        let fts_and_swap_ms = finalize_started.elapsed().as_millis();

        eprintln!(
            "[Prism Catalog] ntfs_mft_rebuild volume={} rows={} enumerate_total_ms={} db_ingest_ms={} swap_fts_ms={}",
            info.volume_id,
            records,
            started.elapsed().as_millis(),
            db_elapsed_ms,
            fts_and_swap_ms
        );

        // Changes after the MFT snapshot remain in the journal. Consume them
        // immediately so a long rebuild does not wait for the next poll.
        let catch_up = match self.transport.query_journal() {
            Ok(current) => match self.catch_up(info, db, current.next_usn) {
                Ok(stats) => stats,
                Err(error) => {
                    // The snapshot is already committed and remains a valid
                    // restart point. A transient read failure is retried on
                    // the next poll; do not discard the freshly built NTFS
                    // generation by starting a recursive fallback walk.
                    eprintln!(
                        "[Prism Catalog] NTFS post-rebuild journal catch-up deferred for {}: {error}",
                        info.volume_id
                    );
                    SyncStats::default()
                }
            },
            Err(error) => {
                eprintln!(
                    "[Prism Catalog] NTFS post-rebuild journal query deferred for {}: {error}",
                    info.volume_id
                );
                SyncStats::default()
            }
        };
        Ok(SyncStats {
            records: records + catch_up.records,
            rebuilt: true,
        })
    }

    fn catch_up(
        &mut self,
        info: &VolumeInfo,
        db: &Database,
        target_usn: i64,
    ) -> Result<SyncStats, String> {
        let started = Instant::now();
        let checkpoint = db
            .get_ntfs_checkpoint(&info.volume_id)?
            .ok_or_else(|| "missing NTFS journal checkpoint".to_string())?;
        let mut cursor = checkpoint.next_usn;
        let mut records = 0u64;

        while cursor < target_usn {
            let batch = self
                .transport
                .read_journal(cursor, checkpoint.journal_id, target_usn)?;
            if batch.next_usn < cursor {
                return Err("USN journal returned a regressing cursor".to_string());
            }
            db.apply_ntfs_changes(
                &info.volume_id,
                checkpoint.journal_id,
                batch.next_usn,
                &batch.changes,
            )?;
            records += batch.record_count;
            if batch.next_usn == cursor {
                break;
            }
            cursor = batch.next_usn;
        }

        db.mark_ntfs_ready(&info.volume_id)?;
        eprintln!(
            "[Prism Catalog] ntfs_usn_catchup volume={} records={} elapsed_ms={} cursor={}",
            info.volume_id,
            records,
            started.elapsed().as_millis(),
            cursor
        );
        Ok(SyncStats {
            records,
            rebuilt: false,
        })
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    #[ignore = "requires direct access to a local fixed NTFS volume"]
    fn queries_native_ntfs_journal() {
        let volume = crate::catalog::volume::discover_volumes()
            .into_iter()
            .find(|volume| {
                crate::catalog::backend::select_backend(volume, true)
                    == crate::catalog::backend::BackendKind::Ntfs
            })
            .expect("local fixed NTFS volume");
        let native = NtfsVolume::open(&volume).expect("open raw NTFS volume");
        let journal = native.query_journal().expect("query USN journal");
        assert!(journal.next_usn >= journal.first_usn);
    }
}
