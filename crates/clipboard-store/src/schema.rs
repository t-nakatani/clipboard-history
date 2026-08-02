use rusqlite::Connection;

use crate::StoreError;

pub const CURRENT_SCHEMA_VERSION: i64 = 5;

// Search ignores case, so the prefix seek has to be collated the same way the
// LIKE comparison is. Shared by the fresh install and the version 4 migration so
// the index the planner sees cannot drift from the one the queries ask for.
const PREFIX_INDEX_SQL: &str = "
    CREATE INDEX idx_clips_text_prefix
        ON clips(substr(normalized_text, 1, 64) COLLATE NOCASE)
        WHERE normalized_text IS NOT NULL;
";

// Shared by the fresh install and the version 3 migration so the two paths
// cannot drift. The triggers count deleted text rows because FTS5 only marks
// them in the segments; the counter is what decides when an optimize is worth
// its cost.
const MAINTENANCE_STATE_SQL: &str = "
    CREATE TABLE maintenance_state (
        id                         INTEGER PRIMARY KEY CHECK (id = 1),
        deleted_since_fts_optimize INTEGER NOT NULL DEFAULT 0,
        last_fts_optimize_at       INTEGER NOT NULL DEFAULT 0
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
    END;
";

pub fn configure_connection(connection: &Connection, cache_kib: usize) -> Result<(), StoreError> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "cache_size", -(cache_kib as i64))?;
    connection.pragma_update(None, "mmap_size", 0)?;
    // LIKE stays case-insensitive (the SQLite default) because searching the
    // history for "http" must find "HTTP". This pragma is what decides it for
    // every LIKE predicate, and it only folds ASCII. The one search predicate
    // that is not a LIKE -- the indexed prefix equality for needles longer than
    // 64 characters -- carries an explicit NOCASE collation to match, as does
    // idx_clips_text_prefix so that the seek survives.
    connection.pragma_update(None, "case_sensitive_like", "OFF")?;
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
            CREATE INDEX idx_representations_payload
                ON representations(payload_hash)
                WHERE payload_hash IS NOT NULL;

            CREATE VIRTUAL TABLE clips_fts USING fts5(
                normalized_text,
                content='clips',
                content_rowid='id',
                tokenize='trigram'
            );

            -- FTS5 segments are append-only, so a plain delete leaves a tombstone
            -- and keeps the original trigrams in clips_fts_data where PRAGMA
            -- secure_delete cannot reach them. This option makes FTS5 rewrite the
            -- affected segment instead. It is stored in clips_fts_config and is
            -- therefore persistent across connections.
            INSERT INTO clips_fts(clips_fts, rank) VALUES('secure-delete', 1);

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
        transaction.execute_batch(PREFIX_INDEX_SQL)?;
        transaction.execute_batch(MAINTENANCE_STATE_SQL)?;
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
        // plaintext in free pages, and their FTS5 segments still hold the
        // trigrams of every text clip ever deleted.
        if has_table(connection, "clips_fts")? {
            // Order matters. Enabling the option only governs future deletes, so
            // the index is rebuilt from the surviving clips rows to drop the
            // legacy segments and tombstones, and only then does VACUUM evict the
            // pages they occupied from the file.
            connection.execute_batch(
                "INSERT INTO clips_fts(clips_fts, rank) VALUES('secure-delete', 1);
                 INSERT INTO clips_fts(clips_fts) VALUES('rebuild');",
            )?;
        }
        // A one-time full VACUUM rewrites the file without the freed pages; the
        // following checkpoint truncates the WAL so they do not survive there
        // either. This is the only place a full VACUUM runs, and the version bump
        // guards it against repeating.
        connection.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        connection.pragma_update(None, "user_version", 3)?;
        version = 3;
    }
    if version == 3 {
        // Idle maintenance needs to know how much FTS5 churn has accumulated
        // across restarts, so the counter is a table rather than in-memory state.
        let transaction = connection.transaction()?;
        transaction.execute_batch(MAINTENANCE_STATE_SQL)?;
        transaction.pragma_update(None, "user_version", 4)?;
        transaction.commit()?;
        version = 4;
    }
    if version == 4 {
        // The prefix index was built with the default BINARY collation, which a
        // case-insensitive LIKE cannot seek. Rebuilding it under NOCASE is what
        // keeps prefix search off a full table scan.
        let transaction = connection.transaction()?;
        transaction.execute_batch("DROP INDEX IF EXISTS idx_clips_text_prefix;")?;
        transaction.execute_batch(PREFIX_INDEX_SQL)?;
        transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        transaction.commit()?;
        version = CURRENT_SCHEMA_VERSION;
    }
    debug_assert_eq!(version, CURRENT_SCHEMA_VERSION);
    Ok(())
}

fn has_table(connection: &Connection, name: &str) -> Result<bool, StoreError> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
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
    fn configured_connections_compare_text_without_case() {
        let connection = Connection::open_in_memory().unwrap();
        configure_connection(&connection, 1024).unwrap();
        // Case-insensitive search rests on this per-connection pragma, so it is
        // worth pinning next to the connection setup and not only end to end.
        let matches_ignoring_case: i64 = connection
            .query_row("SELECT 'A' LIKE 'a'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(matches_ignoring_case, 1);
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
                 CREATE TABLE clips (id INTEGER PRIMARY KEY, normalized_text TEXT);
                 INSERT INTO clips(id, normalized_text) VALUES (1, '{secret}');
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
                 INSERT INTO clips(id, normalized_text) VALUES (2, '{secret}');
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

    #[test]
    fn version_three_database_gains_maintenance_state_and_counts_fts_churn() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE clips (id INTEGER PRIMARY KEY, normalized_text TEXT);
                 INSERT INTO clips(id, normalized_text) VALUES (1, 'counted'), (2, NULL);
                 PRAGMA user_version=3;",
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        // Migrating an existing database must not invent churn that was never
        // paid into the FTS index.
        assert_eq!(deleted_since_fts_optimize(&connection), 0);

        // Replacing text leaves a tombstone behind, so an update costs the index
        // as much as a delete does.
        connection
            .execute_batch("UPDATE clips SET normalized_text = 'replaced' WHERE id = 1;")
            .unwrap();
        assert_eq!(deleted_since_fts_optimize(&connection), 1);

        // Only rows that carried text were ever indexed, so the NULL row is free.
        connection.execute_batch("DELETE FROM clips;").unwrap();
        assert_eq!(deleted_since_fts_optimize(&connection), 2);
    }

    fn deleted_since_fts_optimize(connection: &Connection) -> i64 {
        connection
            .query_row(
                "SELECT deleted_since_fts_optimize FROM maintenance_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn file_contains(path: &std::path::Path, needle: &[u8]) -> bool {
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        bytes.windows(needle.len()).any(|window| window == needle)
    }

    #[test]
    fn version_two_migration_rebuilds_fts_and_drops_legacy_trigrams() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("clipboard-legacy-fts-{unique}"));
        // Rare trigrams; the FTS5 trigram tokenizer only ever stores three
        // character tokens, so the whole string would never be found in the index.
        let deleted = format!("deleted-zqxjvw-{unique}");
        let kept = format!("kept-wjvxqz-{unique}");
        let deleted_trigrams = ["zqx", "qxj", "xjv", "jvw"];

        // A version 2 database: external content FTS5 without the secure-delete
        // option, holding one clip that was already deleted the legacy way.
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "PRAGMA journal_mode=WAL;
                 PRAGMA secure_delete=OFF;
                 CREATE TABLE clips (id INTEGER PRIMARY KEY, normalized_text TEXT);
                 CREATE TABLE clip_previews (clip_id INTEGER PRIMARY KEY);
                 CREATE VIRTUAL TABLE clips_fts USING fts5(
                     normalized_text,
                     content='clips',
                     content_rowid='id',
                     tokenize='trigram'
                 );
                 INSERT INTO clips(id, normalized_text) VALUES (1, '{deleted}'), (2, '{kept}');
                 INSERT INTO clips_fts(rowid, normalized_text)
                     SELECT id, normalized_text FROM clips;
                 INSERT INTO clips_fts(clips_fts, rowid, normalized_text)
                     VALUES ('delete', 1, '{deleted}');
                 DELETE FROM clips WHERE id = 1;
                 PRAGMA user_version=2;
                 PRAGMA wal_checkpoint(TRUNCATE);"
            ))
            .unwrap();
        for trigram in deleted_trigrams {
            assert!(
                file_contains(&path, trigram.as_bytes()),
                "legacy FTS segments should still hold {trigram} before migrating"
            );
        }

        configure_connection(&connection, 1024).unwrap();
        migrate(&mut connection).unwrap();

        for trigram in deleted_trigrams {
            assert!(
                !file_contains(&path, trigram.as_bytes()),
                "migration left the legacy trigram {trigram} in the file"
            );
        }
        // The rebuild must preserve the clip that was never deleted.
        let hits: i64 = connection
            .query_row(
                "SELECT count(*) FROM clips_fts WHERE clips_fts MATCH ?1",
                [format!("\"{kept}\"")],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);

        // Deletes made after the migration are covered by the persisted option.
        connection
            .execute_batch(&format!(
                "INSERT INTO clips_fts(clips_fts, rowid, normalized_text)
                     VALUES ('delete', 2, '{kept}');
                 DELETE FROM clips WHERE id = 2;
                 PRAGMA wal_checkpoint(TRUNCATE);"
            ))
            .unwrap();
        assert!(!file_contains(&path, b"wjv"));

        drop(connection);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn fts_secure_delete_option_survives_reopening() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("clipboard-fts-option-{unique}"));
        let mut connection = Connection::open(&path).unwrap();
        configure_connection(&connection, 1024).unwrap();
        migrate(&mut connection).unwrap();
        drop(connection);

        // The option lives in the clips_fts_config shadow table, so a fresh
        // connection that never sets it must still observe it.
        let reopened = Connection::open(&path).unwrap();
        configure_connection(&reopened, 1024).unwrap();
        let value: i64 = reopened
            .query_row(
                "SELECT v FROM clips_fts_config WHERE k = 'secure-delete'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, 1);

        drop(reopened);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn version_four_database_recollates_the_prefix_index() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE clips (id INTEGER PRIMARY KEY, normalized_text TEXT);
                 CREATE INDEX idx_clips_text_prefix
                     ON clips(substr(normalized_text, 1, 64))
                     WHERE normalized_text IS NOT NULL;
                 INSERT INTO clips(id, normalized_text) VALUES (1, 'Alpha');
                 PRAGMA user_version=4;",
            )
            .unwrap();
        migrate(&mut connection).unwrap();

        let definition: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name='idx_clips_text_prefix'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            definition.contains("COLLATE NOCASE"),
            "the migrated prefix index is still case-sensitive: {definition}"
        );
        // The rebuild has to carry the existing rows over, not just the shape.
        // INDEXED BY forces the read through the index; without it the planner
        // scans this one-row table and the assertion would pass on an empty
        // index. The NOT NULL term is required because the index is partial.
        let hits: i64 = connection
            .query_row(
                "SELECT count(*) FROM clips INDEXED BY idx_clips_text_prefix
                 WHERE normalized_text IS NOT NULL
                   AND substr(normalized_text, 1, 64) COLLATE NOCASE LIKE 'alph%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);
    }

    #[test]
    fn version_one_database_gains_preview_table() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE clips (id INTEGER PRIMARY KEY, normalized_text TEXT);
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
