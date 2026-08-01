use rusqlite::Connection;

use crate::StoreError;

pub const CURRENT_SCHEMA_VERSION: i64 = 3;

pub fn configure_connection(connection: &Connection, cache_kib: usize) -> Result<(), StoreError> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "cache_size", -(cache_kib as i64))?;
    connection.pragma_update(None, "mmap_size", 0)?;
    connection.pragma_update(None, "case_sensitive_like", "ON")?;
    // Clipboard history holds secrets, so freed pages must not keep plaintext.
    // secure_delete makes SQLite zero deleted content in clips, representations,
    // clip_previews and the FTS index instead of only unlinking it from the
    // b-tree. It is a per-connection setting and must be enabled on every
    // connection before any write.
    connection.pragma_update(None, "secure_delete", "ON")?;
    Ok(())
}

pub fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let mut version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > CURRENT_SCHEMA_VERSION {
        return Err(StoreError::InvalidData("schema is newer than this binary"));
    }
    if version == 0 {
        // auto_vacuum changes the on-disk page layout. VACUUM is required to
        // persist the mode reliably before the first application table exists.
        connection.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM;")?;
        let auto_vacuum: i64 =
            connection.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
        if auto_vacuum != 2 {
            return Err(StoreError::InvalidData(
                "failed to enable incremental auto-vacuum",
            ));
        }
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "
            CREATE TABLE clips (
                id              INTEGER PRIMARY KEY,
                content_hash    BLOB NOT NULL UNIQUE,
                content_kind    INTEGER NOT NULL,
                first_copied_at INTEGER NOT NULL,
                last_used_at    INTEGER NOT NULL,
                pinned          INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
                copy_count      INTEGER NOT NULL DEFAULT 1,
                payload_size    INTEGER NOT NULL,
                normalized_text TEXT
            );

            CREATE TABLE payloads (
                payload_hash BLOB PRIMARY KEY,
                payload_size INTEGER NOT NULL,
                created_at   INTEGER NOT NULL
            ) WITHOUT ROWID;

            CREATE TABLE representations (
                id           INTEGER PRIMARY KEY,
                clip_id      INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
                ordinal      INTEGER NOT NULL,
                uti          TEXT NOT NULL,
                payload_hash BLOB REFERENCES payloads(payload_hash),
                inline_data  BLOB,
                UNIQUE (clip_id, ordinal),
                CHECK ((payload_hash IS NULL) != (inline_data IS NULL))
            );

            CREATE TABLE clip_previews (
                clip_id INTEGER PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE,
                uti     TEXT NOT NULL,
                data    BLOB NOT NULL CHECK (length(data) <= 65536)
            );

            CREATE TABLE payload_gc_queue (
                payload_hash BLOB PRIMARY KEY,
                enqueued_at  INTEGER NOT NULL
            ) WITHOUT ROWID;

            CREATE TRIGGER representations_queue_payload_gc
            AFTER DELETE ON representations
            WHEN old.payload_hash IS NOT NULL
            BEGIN
                INSERT OR IGNORE INTO payload_gc_queue(payload_hash, enqueued_at)
                VALUES (old.payload_hash, unixepoch('subsec') * 1000);
            END;

            CREATE INDEX idx_clips_recent
                ON clips(last_used_at DESC, id DESC);
            CREATE INDEX idx_clips_retention
                ON clips(pinned, last_used_at, id);
            CREATE INDEX idx_clips_text_prefix
                ON clips(substr(normalized_text, 1, 64))
                WHERE normalized_text IS NOT NULL;
            CREATE INDEX idx_representations_payload
                ON representations(payload_hash)
                WHERE payload_hash IS NOT NULL;

            CREATE VIRTUAL TABLE clips_fts USING fts5(
                normalized_text,
                content='clips',
                content_rowid='id',
                tokenize='trigram'
            );

            CREATE TRIGGER clips_fts_insert AFTER INSERT ON clips
            WHEN new.normalized_text IS NOT NULL
            BEGIN
                INSERT INTO clips_fts(rowid, normalized_text)
                VALUES (new.id, new.normalized_text);
            END;

            CREATE TRIGGER clips_fts_delete AFTER DELETE ON clips
            WHEN old.normalized_text IS NOT NULL
            BEGIN
                INSERT INTO clips_fts(clips_fts, rowid, normalized_text)
                VALUES ('delete', old.id, old.normalized_text);
            END;

            CREATE TRIGGER clips_fts_update AFTER UPDATE OF normalized_text ON clips
            BEGIN
                INSERT INTO clips_fts(clips_fts, rowid, normalized_text)
                SELECT 'delete', old.id, old.normalized_text
                WHERE old.normalized_text IS NOT NULL;
                INSERT INTO clips_fts(rowid, normalized_text)
                SELECT new.id, new.normalized_text
                WHERE new.normalized_text IS NOT NULL;
            END;
            ",
        )?;
        transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        transaction.commit()?;
        version = CURRENT_SCHEMA_VERSION;
    }
    if version == 1 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "CREATE TABLE clip_previews (
                clip_id INTEGER PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE,
                uti     TEXT NOT NULL,
                data    BLOB NOT NULL CHECK (length(data) <= 65536)
            );",
        )?;
        transaction.pragma_update(None, "user_version", 2)?;
        transaction.commit()?;
        version = 2;
    }
    if version == 2 {
        // Databases written before secure_delete was enabled can still hold
        // plaintext in free pages. A one-time full VACUUM rewrites the file
        // without them; the following checkpoint truncates the WAL so the old
        // pages do not survive there either. This is the only place a full
        // VACUUM runs, and the version bump guards it against repeating.
        connection.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        connection.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        version = CURRENT_SCHEMA_VERSION;
    }
    debug_assert_eq!(version, CURRENT_SCHEMA_VERSION);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn migration_is_idempotent_and_versioned() {
        let mut connection = Connection::open_in_memory().unwrap();
        configure_connection(&connection, 1024).unwrap();
        migrate(&mut connection).unwrap();
        migrate(&mut connection).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let auto_vacuum: i64 = connection
            .pragma_query_value(None, "auto_vacuum", |row| row.get(0))
            .unwrap();
        assert_eq!(auto_vacuum, 2);
    }

    #[test]
    fn configured_connections_enable_secure_delete() {
        let connection = Connection::open_in_memory().unwrap();
        configure_connection(&connection, 1024).unwrap();
        let secure_delete: i64 = connection
            .pragma_query_value(None, "secure_delete", |row| row.get(0))
            .unwrap();
        assert_eq!(secure_delete, 1);
    }

    #[test]
    fn version_two_database_is_scrubbed_once_by_migration() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("clipboard-secure-delete-migration-{unique}"));
        let secret = format!("legacy-plaintext-{unique}");
        // A database written before secure_delete existed keeps deleted plaintext
        // in its free pages.
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "PRAGMA journal_mode=WAL;
                 PRAGMA secure_delete=OFF;
                 CREATE TABLE clips (id INTEGER PRIMARY KEY, text TEXT);
                 INSERT INTO clips(id, text) VALUES (1, '{secret}');
                 DELETE FROM clips;
                 PRAGMA user_version=2;
                 PRAGMA wal_checkpoint(TRUNCATE);"
            ))
            .unwrap();
        assert!(
            file_contains(&path, secret.as_bytes()),
            "the test is only meaningful if the legacy residue is really there"
        );

        configure_connection(&connection, 1024).unwrap();
        migrate(&mut connection).unwrap();

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        assert!(!file_contains(&path, secret.as_bytes()));

        // The version bump guards the VACUUM: fresh residue is not rewritten by a
        // second migration.
        connection
            .execute_batch(&format!(
                "PRAGMA secure_delete=OFF;
                 INSERT INTO clips(id, text) VALUES (2, '{secret}');
                 DELETE FROM clips;
                 PRAGMA wal_checkpoint(TRUNCATE);"
            ))
            .unwrap();
        migrate(&mut connection).unwrap();
        assert!(file_contains(&path, secret.as_bytes()));

        drop(connection);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    fn file_contains(path: &std::path::Path, needle: &[u8]) -> bool {
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        bytes.windows(needle.len()).any(|window| window == needle)
    }

    #[test]
    fn version_one_database_gains_preview_table() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE clips (id INTEGER PRIMARY KEY);
                 PRAGMA user_version=1;",
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        let table_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='clip_previews'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }
}
