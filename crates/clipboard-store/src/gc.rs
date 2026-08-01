use rusqlite::{Connection, OptionalExtension};

use crate::{PayloadHash, PayloadStore, StoreError, payload::PayloadEntry};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GarbageCollectionStats {
    pub queued_scanned: u64,
    pub referenced_skipped: u64,
    pub payload_files_deleted: u64,
    pub missing_payload_files: u64,
    pub orphan_files_deleted: u64,
    pub staged_files_deleted: u64,
}

pub(crate) fn collect_orphans(
    connection: &Connection,
    payload_store: &PayloadStore,
) -> Result<GarbageCollectionStats, StoreError> {
    let mut stats = GarbageCollectionStats::default();
    payload_store.visit_entries(|entry| {
        match entry {
            PayloadEntry::Staged(path) => {
                if payload_store.remove_staged(&path)? {
                    stats.staged_files_deleted += 1;
                }
            }
            PayloadEntry::Payload(hash) => {
                let known = connection
                    .query_row(
                        "SELECT 1 FROM payloads WHERE payload_hash = ?1",
                        [hash.0.as_slice()],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !known && payload_store.remove_if_exists(hash)? {
                    stats.orphan_files_deleted += 1;
                }
            }
        }
        Ok(())
    })?;
    Ok(stats)
}

pub(crate) fn collect_queued(
    connection: &mut Connection,
    payload_store: &PayloadStore,
    limit: usize,
) -> Result<GarbageCollectionStats, StoreError> {
    let candidates = {
        let mut statement = connection.prepare(
            "SELECT payload_hash FROM payload_gc_queue
             ORDER BY enqueued_at, payload_hash LIMIT ?1",
        )?;
        statement
            .query_map([limit.min(10_000) as i64], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut stats = GarbageCollectionStats::default();
    for bytes in candidates {
        stats.queued_scanned += 1;
        let hash = payload_hash(bytes)?;
        let referenced: bool = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM representations WHERE payload_hash = ?1 LIMIT 1
             )",
            [hash.0.as_slice()],
            |row| row.get(0),
        )?;
        if referenced {
            connection.execute(
                "DELETE FROM payload_gc_queue WHERE payload_hash = ?1",
                [hash.0.as_slice()],
            )?;
            stats.referenced_skipped += 1;
            continue;
        }

        if payload_store.remove_if_exists(hash)? {
            stats.payload_files_deleted += 1;
        } else {
            stats.missing_payload_files += 1;
        }
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM payloads WHERE payload_hash = ?1",
            [hash.0.as_slice()],
        )?;
        transaction.execute(
            "DELETE FROM payload_gc_queue WHERE payload_hash = ?1",
            [hash.0.as_slice()],
        )?;
        transaction.commit()?;
    }
    Ok(stats)
}

fn payload_hash(bytes: Vec<u8>) -> Result<PayloadHash, StoreError> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| StoreError::InvalidData("payload hash is not 32 bytes"))?;
    Ok(PayloadHash(bytes))
}
