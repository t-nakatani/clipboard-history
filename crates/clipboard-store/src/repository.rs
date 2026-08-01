use clipboard_core::{
    ClipCandidate, ClipId, ClipKind, ClipSummary, HistoryCursor, HistoryPage, ImagePreview,
    MatchMode, PlannedQuery, Representation, UpsertOutcome,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{PayloadStore, StoreError};

pub(crate) struct RepositoryUpsertResult {
    pub outcome: UpsertOutcome,
    pub pruned: usize,
}

pub(crate) fn upsert(
    connection: &mut Connection,
    payload_store: &PayloadStore,
    inline_threshold: usize,
    candidate: ClipCandidate,
    prune_count: usize,
) -> Result<RepositoryUpsertResult, StoreError> {
    let existing = connection
        .query_row(
            "SELECT id FROM clips WHERE content_hash = ?1",
            [candidate.identity.0.as_slice()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(id) = existing {
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE clips
             SET last_used_at = ?2, copy_count = copy_count + 1
             WHERE id = ?1",
            params![id, candidate.copied_at_ms],
        )?;
        if let Some(preview) = &candidate.image_preview {
            insert_image_preview(&transaction, id, preview)?;
        }
        transaction.commit()?;
        return Ok(RepositoryUpsertResult {
            outcome: UpsertOutcome {
                id: ClipId(id),
                inserted: false,
            },
            pruned: 0,
        });
    }

    // Large values are made durable before the SQLite row can reference them.
    let mut staged = Vec::with_capacity(candidate.representations.len());
    for representation in &candidate.representations {
        if representation.bytes.len() > inline_threshold {
            staged.push(Some(payload_store.put(&representation.bytes)?));
        } else {
            staged.push(None);
        }
    }

    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO clips(
            content_hash, content_kind, first_copied_at, last_used_at,
            pinned, copy_count, payload_size, normalized_text
         ) VALUES (?1, ?2, ?3, ?3, 0, 1, ?4, ?5)",
        params![
            candidate.identity.0.as_slice(),
            candidate.kind.as_i64(),
            candidate.copied_at_ms,
            candidate.payload_size() as i64,
            candidate.normalized_text,
        ],
    )?;
    let id = transaction.last_insert_rowid();

    for (ordinal, (representation, payload)) in candidate
        .representations
        .iter()
        .zip(staged.iter())
        .enumerate()
    {
        insert_representation(&transaction, id, ordinal, representation, payload.as_ref())?;
    }
    if let Some(preview) = &candidate.image_preview {
        insert_image_preview(&transaction, id, preview)?;
    }
    let pruned = prune_oldest_in_connection(&transaction, prune_count)?;
    transaction.commit()?;
    Ok(RepositoryUpsertResult {
        outcome: UpsertOutcome {
            id: ClipId(id),
            inserted: true,
        },
        pruned,
    })
}

fn insert_image_preview(
    connection: &Connection,
    clip_id: i64,
    preview: &ImagePreview,
) -> Result<(), StoreError> {
    if preview.bytes.is_empty() || preview.bytes.len() > 64 * 1024 {
        return Err(StoreError::InvalidData(
            "image preview must contain between 1 and 65536 bytes",
        ));
    }
    connection.execute(
        "INSERT INTO clip_previews(clip_id, uti, data) VALUES (?1, ?2, ?3)
         ON CONFLICT(clip_id) DO UPDATE SET uti = excluded.uti, data = excluded.data",
        params![clip_id, preview.uti, preview.bytes],
    )?;
    Ok(())
}

pub(crate) fn count(connection: &Connection) -> Result<usize, StoreError> {
    connection
        .query_row("SELECT count(*) FROM clips", [], |row| row.get::<_, i64>(0))
        .map(|value| value as usize)
        .map_err(Into::into)
}

pub(crate) fn prune_oldest(connection: &mut Connection, limit: usize) -> Result<usize, StoreError> {
    let transaction = connection.transaction()?;
    let deleted = prune_oldest_in_connection(&transaction, limit)?;
    transaction.commit()?;
    Ok(deleted)
}

fn prune_oldest_in_connection(connection: &Connection, limit: usize) -> Result<usize, StoreError> {
    if limit == 0 {
        return Ok(0);
    }
    connection
        .execute(
            "DELETE FROM clips
             WHERE id IN (
                 SELECT id FROM clips
                 WHERE pinned = 0
                 ORDER BY last_used_at ASC, id ASC
                 LIMIT ?1
             )",
            [limit as i64],
        )
        .map_err(Into::into)
}

fn insert_representation(
    connection: &Connection,
    clip_id: i64,
    ordinal: usize,
    representation: &Representation,
    payload: Option<&crate::StoredPayload>,
) -> Result<(), StoreError> {
    if let Some(payload) = payload {
        connection.execute(
            "INSERT OR IGNORE INTO payloads(payload_hash, payload_size, created_at)
             VALUES (?1, ?2, unixepoch('subsec') * 1000)",
            params![payload.hash.0.as_slice(), payload.size as i64],
        )?;
        connection.execute(
            "INSERT INTO representations(clip_id, ordinal, uti, payload_hash, inline_data)
             VALUES (?1, ?2, ?3, ?4, NULL)",
            params![
                clip_id,
                ordinal as i64,
                representation.uti,
                payload.hash.0.as_slice()
            ],
        )?;
    } else {
        connection.execute(
            "INSERT INTO representations(clip_id, ordinal, uti, payload_hash, inline_data)
             VALUES (?1, ?2, ?3, NULL, ?4)",
            params![
                clip_id,
                ordinal as i64,
                representation.uti,
                representation.bytes
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn recent_page(
    connection: &Connection,
    cursor: Option<HistoryCursor>,
    limit: usize,
) -> Result<HistoryPage, StoreError> {
    let limit = limit.clamp(1, 200);
    let fetch_limit = (limit + 1) as i64;
    let items = if let Some(cursor) = cursor {
        query_summaries(
            connection,
            "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                    c.payload_size, substr(c.normalized_text, 1, 256),
                    EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
             FROM clips AS c
             WHERE c.last_used_at < ?1
                OR (c.last_used_at = ?1 AND c.id < ?2)
             ORDER BY c.last_used_at DESC, c.id DESC
             LIMIT ?3",
            params![cursor.last_used_at_ms, cursor.id.0, fetch_limit],
        )?
    } else {
        query_summaries(
            connection,
            "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                    c.payload_size, substr(c.normalized_text, 1, 256),
                    EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
             FROM clips AS c
             ORDER BY c.last_used_at DESC, c.id DESC
             LIMIT ?1",
            [fetch_limit],
        )?
    };
    Ok(history_page(items, limit))
}

pub(crate) fn image_preview(
    connection: &Connection,
    id: ClipId,
) -> Result<Option<ImagePreview>, StoreError> {
    connection
        .query_row(
            "SELECT uti, data FROM clip_previews WHERE clip_id = ?1",
            [id.0],
            |row| {
                Ok(ImagePreview {
                    uti: row.get(0)?,
                    bytes: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

pub(crate) fn representations(
    connection: &Connection,
    payload_store: &PayloadStore,
    id: ClipId,
    max_restore_bytes: usize,
) -> Result<Vec<Representation>, StoreError> {
    let payload_size = connection
        .query_row(
            "SELECT payload_size FROM clips WHERE id = ?1",
            [id.0],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let Some(payload_size) = payload_size else {
        return Ok(Vec::new());
    };
    if payload_size < 0 || payload_size as usize > max_restore_bytes {
        return Err(StoreError::InvalidData(
            "clip exceeds the configured restore byte limit",
        ));
    }

    let mut statement = connection.prepare(
        "SELECT uti, payload_hash, inline_data
         FROM representations
         WHERE clip_id = ?1
         ORDER BY ordinal",
    )?;
    let rows = statement.query_map([id.0], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<Vec<u8>>>(1)?,
            row.get::<_, Option<Vec<u8>>>(2)?,
        ))
    })?;

    let mut result = Vec::new();
    for row in rows {
        let (uti, payload_hash, inline_data) = row?;
        let bytes = match (payload_hash, inline_data) {
            (None, Some(bytes)) => bytes,
            (Some(raw_hash), None) => {
                let hash: [u8; 32] = raw_hash
                    .try_into()
                    .map_err(|_| StoreError::InvalidData("invalid payload hash length"))?;
                payload_store.read(crate::PayloadHash(hash))?
            }
            _ => {
                return Err(StoreError::InvalidData(
                    "representation has invalid payload storage",
                ));
            }
        };
        result.push(Representation { uti, bytes });
    }
    Ok(result)
}

pub(crate) fn search_page(
    connection: &Connection,
    query: PlannedQuery,
    cursor: Option<HistoryCursor>,
    limit: usize,
) -> Result<HistoryPage, StoreError> {
    let limit = limit.clamp(1, 200);
    let fetch_limit = (limit + 1) as i64;
    match query {
        PlannedQuery::Empty => recent_page(connection, cursor, limit),
        PlannedQuery::RecentScan { mode, needle } => {
            let pattern = like_pattern(mode, &needle);
            let items = if let Some(cursor) = cursor {
                query_summaries(
                    connection,
                    "SELECT id, content_kind, last_used_at, pinned, copy_count,
                            payload_size, substr(normalized_text, 1, 256), has_image_preview
                     FROM (
                         SELECT id, content_kind, last_used_at, pinned, copy_count,
                                payload_size, normalized_text,
                                EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = clips.id)
                                    AS has_image_preview
                         FROM clips
                         WHERE normalized_text IS NOT NULL
                         ORDER BY last_used_at DESC, id DESC
                         LIMIT 2000
                     )
                     WHERE normalized_text LIKE ?1 ESCAPE '\\'
                       AND (last_used_at < ?2 OR (last_used_at = ?2 AND id < ?3))
                     ORDER BY last_used_at DESC, id DESC
                     LIMIT ?4",
                    params![pattern, cursor.last_used_at_ms, cursor.id.0, fetch_limit],
                )?
            } else {
                query_summaries(
                    connection,
                    "SELECT id, content_kind, last_used_at, pinned, copy_count,
                            payload_size, substr(normalized_text, 1, 256), has_image_preview
                     FROM (
                         SELECT id, content_kind, last_used_at, pinned, copy_count,
                                payload_size, normalized_text,
                                EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = clips.id)
                                    AS has_image_preview
                         FROM clips
                         WHERE normalized_text IS NOT NULL
                         ORDER BY last_used_at DESC, id DESC
                         LIMIT 2000
                     )
                     WHERE normalized_text LIKE ?1 ESCAPE '\\'
                     ORDER BY last_used_at DESC, id DESC
                     LIMIT ?2",
                    params![pattern, fetch_limit],
                )?
            };
            Ok(history_page(items, limit))
        }
        PlannedQuery::Indexed {
            mode: MatchMode::Prefix,
            needle,
        } => search_prefix_page(connection, &needle, cursor, limit),
        PlannedQuery::Indexed { mode, needle } => {
            let pattern = like_pattern(mode, &needle);
            let items = if let Some(cursor) = cursor {
                query_summaries(
                    connection,
                    "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                            c.payload_size, substr(c.normalized_text, 1, 256),
                            EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
                     FROM clips_fts
                     JOIN clips AS c ON c.id = clips_fts.rowid
                     WHERE clips_fts.normalized_text LIKE ?1 ESCAPE '\\'
                       AND (c.last_used_at < ?2 OR (c.last_used_at = ?2 AND c.id < ?3))
                     ORDER BY c.last_used_at DESC, c.id DESC
                     LIMIT ?4",
                    params![pattern, cursor.last_used_at_ms, cursor.id.0, fetch_limit],
                )?
            } else {
                query_summaries(
                    connection,
                    "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                            c.payload_size, substr(c.normalized_text, 1, 256),
                            EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
                     FROM clips_fts
                     JOIN clips AS c ON c.id = clips_fts.rowid
                     WHERE clips_fts.normalized_text LIKE ?1 ESCAPE '\\'
                     ORDER BY c.last_used_at DESC, c.id DESC
                     LIMIT ?2",
                    params![pattern, fetch_limit],
                )?
            };
            Ok(history_page(items, limit))
        }
    }
}

fn search_prefix_page(
    connection: &Connection,
    needle: &str,
    cursor: Option<HistoryCursor>,
    limit: usize,
) -> Result<HistoryPage, StoreError> {
    let pattern = like_pattern(MatchMode::Prefix, needle);
    let fetch_limit = (limit + 1) as i64;
    let items = if needle.chars().count() <= 64 {
        if let Some(cursor) = cursor {
            query_summaries(
                connection,
                "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                        c.payload_size, substr(c.normalized_text, 1, 256),
                        EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
                 FROM clips AS c
                 WHERE substr(normalized_text, 1, 64) LIKE ?1 ESCAPE '\\'
                   AND normalized_text LIKE ?1 ESCAPE '\\'
                   AND (c.last_used_at < ?2 OR (c.last_used_at = ?2 AND c.id < ?3))
                 ORDER BY c.last_used_at DESC, c.id DESC
                 LIMIT ?4",
                params![pattern, cursor.last_used_at_ms, cursor.id.0, fetch_limit],
            )?
        } else {
            query_summaries(
                connection,
                "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                        c.payload_size, substr(c.normalized_text, 1, 256),
                        EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
                 FROM clips AS c
                 WHERE substr(normalized_text, 1, 64) LIKE ?1 ESCAPE '\\'
                   AND normalized_text LIKE ?1 ESCAPE '\\'
                 ORDER BY c.last_used_at DESC, c.id DESC
                 LIMIT ?2",
                params![pattern, fetch_limit],
            )?
        }
    } else {
        let key: String = needle.chars().take(64).collect();
        if let Some(cursor) = cursor {
            query_summaries(
                connection,
                "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                        c.payload_size, substr(c.normalized_text, 1, 256),
                        EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
                 FROM clips AS c
                 WHERE substr(normalized_text, 1, 64) = ?1
                   AND normalized_text LIKE ?2 ESCAPE '\\'
                   AND (c.last_used_at < ?3 OR (c.last_used_at = ?3 AND c.id < ?4))
                 ORDER BY c.last_used_at DESC, c.id DESC
                 LIMIT ?5",
                params![
                    key,
                    pattern,
                    cursor.last_used_at_ms,
                    cursor.id.0,
                    fetch_limit
                ],
            )?
        } else {
            query_summaries(
                connection,
                "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                        c.payload_size, substr(c.normalized_text, 1, 256),
                        EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
                 FROM clips AS c
                 WHERE substr(normalized_text, 1, 64) = ?1
                   AND normalized_text LIKE ?2 ESCAPE '\\'
                 ORDER BY c.last_used_at DESC, c.id DESC
                 LIMIT ?3",
                params![key, pattern, fetch_limit],
            )?
        }
    };
    Ok(history_page(items, limit))
}

fn history_page(mut items: Vec<ClipSummary>, limit: usize) -> HistoryPage {
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = if has_more {
        items.last().map(|last| HistoryCursor {
            last_used_at_ms: last.last_used_at_ms,
            id: last.id,
        })
    } else {
        None
    };
    HistoryPage {
        next_cursor,
        items,
        has_more,
    }
}

fn query_summaries<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    parameters: P,
) -> Result<Vec<ClipSummary>, StoreError> {
    let mut statement = connection.prepare_cached(sql)?;
    let rows = statement.query_map(parameters, summary_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClipSummary> {
    let kind = match row.get::<_, i64>(1)? {
        0 => ClipKind::Text,
        1 => ClipKind::Image,
        2 => ClipKind::File,
        _ => ClipKind::Mixed,
    };
    Ok(ClipSummary {
        id: ClipId(row.get(0)?),
        kind,
        last_used_at_ms: row.get(2)?,
        pinned: row.get(3)?,
        copy_count: row.get::<_, i64>(4)? as u64,
        payload_size: row.get::<_, i64>(5)? as u64,
        preview: row.get(6)?,
        has_image_preview: row.get(7)?,
    })
}

fn like_pattern(mode: MatchMode, needle: &str) -> String {
    let escaped = needle
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    match mode {
        MatchMode::Exact => escaped,
        MatchMode::Prefix => format!("{escaped}%"),
        MatchMode::Substring => format!("%{escaped}%"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{configure_connection, migrate};

    #[test]
    fn production_search_sql_keeps_prefix_and_trigram_indexes() {
        let mut connection = Connection::open_in_memory().unwrap();
        configure_connection(&connection, 1024).unwrap();
        migrate(&mut connection).unwrap();

        let prefix_plan = explain(
            &connection,
            "EXPLAIN QUERY PLAN
             SELECT id FROM clips
             WHERE substr(normalized_text, 1, 64) LIKE ?1 ESCAPE '\\'
               AND normalized_text LIKE ?1 ESCAPE '\\'
             ORDER BY last_used_at DESC, id DESC
             LIMIT ?2",
            params!["alpha%", 50_i64],
        );
        assert!(
            prefix_plan.contains("idx_clips_text_prefix"),
            "prefix expression index missing: {prefix_plan}"
        );

        let substring_plan = explain(
            &connection,
            "EXPLAIN QUERY PLAN
             SELECT clips_fts.rowid
             FROM clips_fts
             WHERE clips_fts.normalized_text LIKE ?1 ESCAPE '\\'
             ORDER BY clips_fts.rowid DESC
             LIMIT ?2",
            params!["%alpha%", 50_i64],
        );
        assert!(
            substring_plan.contains("VIRTUAL TABLE INDEX"),
            "FTS5 virtual index missing: {substring_plan}"
        );

        let recent_page_plan = explain(
            &connection,
            "EXPLAIN QUERY PLAN
             SELECT id FROM clips
             WHERE last_used_at < ?1 OR (last_used_at = ?1 AND id < ?2)
             ORDER BY last_used_at DESC, id DESC
             LIMIT ?3",
            params![1_000_i64, 500_i64, 51_i64],
        );
        assert!(
            recent_page_plan.contains("idx_clips_recent"),
            "recent keyset index missing: {recent_page_plan}"
        );
    }

    fn explain<P: rusqlite::Params>(connection: &Connection, sql: &str, parameters: P) -> String {
        let mut statement = connection.prepare(sql).unwrap();
        statement
            .query_map(parameters, |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" | ")
    }
}
