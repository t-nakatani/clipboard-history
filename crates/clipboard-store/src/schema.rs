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
    Ok(())
}

pub fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
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

            CREATE TABLE maintenance_state (
                id                           INTEGER PRIMARY KEY CHECK (id = 1),
                deleted_since_fts_optimize  INTEGER NOT NULL DEFAULT 0,
                last_fts_optimize_at        INTEGER NOT NULL DEFAULT 0
            );

            INSERT INTO maintenance_state(id) VALUES (1);

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

            CREATE TRIGGER clips_fts_maintenance_delete AFTER DELETE ON clips
            WHEN old.normalized_text IS NOT NULL
            BEGIN
                UPDATE maintenance_state
                SET deleted_since_fts_optimize = deleted_since_fts_optimize + 1
                WHERE id = 1;
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

            CREATE TRIGGER clips_fts_maintenance_update
            AFTER UPDATE OF normalized_text ON clips
            WHEN old.normalized_text IS NOT NULL
            BEGIN
                UPDATE maintenance_state
                SET deleted_since_fts_optimize = deleted_since_fts_optimize + 1
                WHERE id = 1;
            END;
            ",
        )?;
        transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        transaction.commit()?;
    }
    if version == 1 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "CREATE TABLE clip_previews (
                clip_id INTEGER PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE,
                uti     TEXT NOT NULL,
                data    BLOB NOT NULL CHECK (length(data) <= 65536)
            );

            CREATE TABLE maintenance_state (
                id                           INTEGER PRIMARY KEY CHECK (id = 1),
                deleted_since_fts_optimize  INTEGER NOT NULL DEFAULT 0,
                last_fts_optimize_at        INTEGER NOT NULL DEFAULT 0
            );

            INSERT INTO maintenance_state(id) VALUES (1);

            CREATE TRIGGER clips_fts_maintenance_delete AFTER DELETE ON clips
            WHEN old.normalized_text IS NOT NULL
            BEGIN
                UPDATE maintenance_state
                SET deleted_since_fts_optimize = deleted_since_fts_optimize + 1
                WHERE id = 1;
            END;

            CREATE TRIGGER clips_fts_maintenance_update
            AFTER UPDATE OF normalized_text ON clips
            WHEN old.normalized_text IS NOT NULL
            BEGIN
                UPDATE maintenance_state
                SET deleted_since_fts_optimize = deleted_since_fts_optimize + 1
                WHERE id = 1;
            END;",
        )?;
        transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        transaction.commit()?;
    }
    if version == 2 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "CREATE TABLE maintenance_state (
                id                           INTEGER PRIMARY KEY CHECK (id = 1),
                deleted_since_fts_optimize  INTEGER NOT NULL DEFAULT 0,
                last_fts_optimize_at        INTEGER NOT NULL DEFAULT 0
            );

            INSERT INTO maintenance_state(id) VALUES (1);

            CREATE TRIGGER clips_fts_maintenance_delete AFTER DELETE ON clips
            WHEN old.normalized_text IS NOT NULL
            BEGIN
                UPDATE maintenance_state
                SET deleted_since_fts_optimize = deleted_since_fts_optimize + 1
                WHERE id = 1;
            END;

            CREATE TRIGGER clips_fts_maintenance_update
            AFTER UPDATE OF normalized_text ON clips
            WHEN old.normalized_text IS NOT NULL
            BEGIN
                UPDATE maintenance_state
                SET deleted_since_fts_optimize = deleted_since_fts_optimize + 1
                WHERE id = 1;
            END;",
        )?;
        transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        transaction.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
