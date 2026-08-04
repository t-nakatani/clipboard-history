use clipboard_core::{
    ClipCandidate, ClipId, ClipKind, ClipSummary, HistoryCursor, HistoryPage, ImagePreview,
    MatchMode, PageDirection, PlannedQuery, Representation, UpsertOutcome,
};
use rusqlite::{Connection, OptionalExtension, named_params, params};

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
    direction: PageDirection,
    limit: usize,
) -> Result<HistoryPage, StoreError> {
    let limit = limit.clamp(1, 200);
    let fetch_limit = (limit + 1) as i64;
    let page = PageSql::new(cursor, direction)?;
    let items = query_summaries(
        connection,
        &recent_page_sql(page),
        named_params! {
            ":anchor_time": page.anchor.last_used_at_ms,
            ":anchor_id": page.anchor.id.0,
            ":limit": fetch_limit,
        },
    )?;
    Ok(history_page(items, limit, page.reverse))
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
                payload_store.read(crate::PayloadHash(hash), max_restore_bytes as u64)?
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
    direction: PageDirection,
    limit: usize,
) -> Result<HistoryPage, StoreError> {
    let limit = limit.clamp(1, 200);
    let fetch_limit = (limit + 1) as i64;
    let page = PageSql::new(cursor, direction)?;
    match query {
        PlannedQuery::Empty => recent_page(connection, cursor, direction, limit),
        PlannedQuery::RecentScan { mode, needle } => {
            let pattern = LikePattern::new(mode, &needle);
            let items = query_summaries(
                connection,
                &recent_scan_sql(page, &pattern),
                named_params! {
                    ":pattern": pattern.value,
                    ":anchor_time": page.anchor.last_used_at_ms,
                    ":anchor_id": page.anchor.id.0,
                    ":limit": fetch_limit,
                },
            )?;
            if items.len() > limit {
                return Ok(history_page(items, limit, page.reverse));
            }
            if let Some(continuation_cursor) = recent_scan_boundary(connection, page)? {
                let mut items = items;
                if page.reverse {
                    items.reverse();
                }
                return Ok(HistoryPage {
                    items,
                    continuation_cursor: Some(continuation_cursor),
                    has_more: true,
                    truncated: true,
                });
            }
            Ok(history_page(items, limit, page.reverse))
        }
        PlannedQuery::Indexed {
            mode: MatchMode::Prefix,
            needle,
        } => search_prefix_page(connection, &needle, page, limit),
        PlannedQuery::Indexed { mode, needle } => {
            let pattern = LikePattern::new(mode, &needle);
            let items = query_summaries(
                connection,
                &indexed_substring_sql(page, &pattern),
                named_params! {
                    ":pattern": pattern.value,
                    ":anchor_time": page.anchor.last_used_at_ms,
                    ":anchor_id": page.anchor.id.0,
                    ":limit": fetch_limit,
                },
            )?;
            Ok(history_page(items, limit, page.reverse))
        }
    }
}

/// Seeks `idx_clips_text_prefix` for a leading match.
///
/// Nothing in production reaches this today: `QueryPlanner::plan` only ever
/// emits `MatchMode::Substring`, so the arm above is dead until #37 brings
/// prefix matching back as a complement to substring search for short needles.
/// It stays because that issue is the one that decides how it is reached, and
/// because the store port is where the contract belongs.
///
/// The cost of keeping it is not zero: `idx_clips_text_prefix` is answered by
/// no production query at all right now, yet every insert and every
/// `normalized_text` update still pays to maintain it. If #37 is dropped or
/// deferred indefinitely, the index and this function should go together --
/// see the note in TODO.md.
fn search_prefix_page(
    connection: &Connection,
    needle: &str,
    page: PageSql,
    limit: usize,
) -> Result<HistoryPage, StoreError> {
    let pattern = LikePattern::new(MatchMode::Prefix, needle);
    let fetch_limit = (limit + 1) as i64;
    let key = prefix_equality_key(needle);
    let items = query_summaries(
        connection,
        &prefix_sql(page, &pattern, key.is_some()),
        named_params! {
            ":pattern": pattern.value,
            ":key": key,
            ":anchor_time": page.anchor.last_used_at_ms,
            ":anchor_id": page.anchor.id.0,
            ":limit": fetch_limit,
        },
    )?;
    Ok(history_page(items, limit, page.reverse))
}

// The four statements below are the whole SQL surface of history reads, and
// each one depends on an index staying reachable. They are built here rather
// than inline so that the query plan test can EXPLAIN the exact string
// production runs; a plan regression test that reads a copy of the SQL only
// proves things about the copy.

/// Keyset page over `idx_clips_recent`.
fn recent_page_sql(page: PageSql) -> String {
    format!(
        "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                c.payload_size, substr(c.normalized_text, 1, 256),
                EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
         FROM clips AS c
         WHERE {predicate}
         ORDER BY c.last_used_at {order}, c.id {order}
         LIMIT :limit",
        predicate = page.predicate("c"),
        order = page.order,
    )
}

/// Filters the most recent 2000 rows, for needles too short to hit a trigram.
fn recent_scan_sql(page: PageSql, pattern: &LikePattern) -> String {
    format!(
        "SELECT id, content_kind, last_used_at, pinned, copy_count,
                payload_size, substr(normalized_text, 1, 256), has_image_preview
         FROM (
             SELECT id, content_kind, last_used_at, pinned, copy_count,
                    payload_size, normalized_text,
                    EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = clips.id)
                        AS has_image_preview
             FROM clips
             WHERE normalized_text IS NOT NULL AND {predicate}
             ORDER BY last_used_at {order}, id {order}
             LIMIT 2000
         )
         WHERE {constraint}
         ORDER BY last_used_at {order}, id {order}
         LIMIT :limit",
        predicate = page.predicate("clips"),
        constraint = pattern.constraint("normalized_text"),
        order = page.order,
    )
}

/// Substring match handed to the fts5 trigram index.
fn indexed_substring_sql(page: PageSql, pattern: &LikePattern) -> String {
    format!(
        "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                c.payload_size, substr(c.normalized_text, 1, 256),
                EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
         FROM clips_fts
         JOIN clips AS c ON c.id = clips_fts.rowid
         WHERE {constraint}
           AND {predicate}
         ORDER BY c.last_used_at {order}, c.id {order}
         LIMIT :limit",
        constraint = pattern.constraint("clips_fts.normalized_text"),
        predicate = page.predicate("c"),
        order = page.order,
    )
}

/// Leading match seeking `idx_clips_text_prefix`.
///
/// `equality_key` selects the variant for needles that outrun the indexed
/// prefix; pass what `prefix_equality_key` returned for the same needle.
fn prefix_sql(page: PageSql, pattern: &LikePattern, equality_key: bool) -> String {
    // rusqlite requires every named parameter to exist in both SQL variants.
    // The NULL guard keeps `:key` present when the expression index uses LIKE.
    // LIKE ignores collation entirely, so NOCASE is not what makes the match
    // case-insensitive here; the case_sensitive_like pragma is. It is what lets
    // both variants seek idx_clips_text_prefix, which is collated the same way.
    // The equality variant does need it for correctness as well.
    let indexed_prefix =
        format!("substr(c.normalized_text, 1, {INDEXED_PREFIX_CHARS}) COLLATE NOCASE");
    let prefix_clause = if equality_key {
        format!("{indexed_prefix} = :key")
    } else {
        format!(
            ":key IS NULL AND {constraint}",
            constraint = pattern.constraint(&indexed_prefix),
        )
    };
    format!(
        "SELECT c.id, c.content_kind, c.last_used_at, c.pinned, c.copy_count,
                c.payload_size, substr(c.normalized_text, 1, 256),
                EXISTS(SELECT 1 FROM clip_previews p WHERE p.clip_id = c.id)
         FROM clips AS c
         WHERE {prefix_clause}
           AND {constraint}
           AND {predicate}
         ORDER BY c.last_used_at {order}, c.id {order}
         LIMIT :limit",
        constraint = pattern.constraint("c.normalized_text"),
        predicate = page.predicate("c"),
        order = page.order,
    )
}

/// How many leading characters `idx_clips_text_prefix` indexes.
const INDEXED_PREFIX_CHARS: usize = 64;

/// The `:key` binding for a prefix search, or `None` when a LIKE seek fits.
///
/// A needle longer than the indexed prefix can no longer be answered by a LIKE
/// seek, so the query compares the indexed expression for equality against the
/// leading characters instead and lets the unindexed `LIKE` filter the rest.
fn prefix_equality_key(needle: &str) -> Option<String> {
    (needle.chars().count() > INDEXED_PREFIX_CHARS)
        .then(|| needle.chars().take(INDEXED_PREFIX_CHARS).collect())
}

#[derive(Clone, Copy)]
struct PageSql {
    anchor: HistoryCursor,
    comparison: &'static str,
    order: &'static str,
    reverse: bool,
}

impl PageSql {
    fn new(cursor: Option<HistoryCursor>, direction: PageDirection) -> Result<Self, StoreError> {
        match (cursor, direction) {
            (None, PageDirection::Older) => Ok(Self {
                anchor: HistoryCursor {
                    last_used_at_ms: i64::MAX,
                    id: ClipId(i64::MAX),
                },
                comparison: "<",
                order: "DESC",
                reverse: false,
            }),
            (Some(anchor), PageDirection::Older) => Ok(Self {
                anchor,
                comparison: "<",
                order: "DESC",
                reverse: false,
            }),
            (Some(anchor), PageDirection::Newer) => Ok(Self {
                anchor,
                comparison: ">",
                order: "ASC",
                reverse: true,
            }),
            (None, PageDirection::Newer) => Err(StoreError::InvalidData(
                "newer page requests require an anchor",
            )),
        }
    }

    fn predicate(self, alias: &str) -> String {
        format!(
            "({alias}.last_used_at {comparison} :anchor_time OR \
             ({alias}.last_used_at = :anchor_time AND {alias}.id {comparison} :anchor_id))",
            comparison = self.comparison,
        )
    }
}

fn history_page(mut items: Vec<ClipSummary>, limit: usize, reverse: bool) -> HistoryPage {
    let has_more = items.len() > limit;
    items.truncate(limit);
    if reverse {
        items.reverse();
    }
    let continuation_cursor = has_more.then(|| {
        let item = if reverse {
            items.first().expect("a non-empty page with continuation")
        } else {
            items.last().expect("a non-empty page with continuation")
        };
        HistoryCursor {
            last_used_at_ms: item.last_used_at_ms,
            id: item.id,
        }
    });
    HistoryPage {
        items,
        continuation_cursor,
        has_more,
        truncated: false,
    }
}

fn recent_scan_boundary(
    connection: &Connection,
    page: PageSql,
) -> Result<Option<HistoryCursor>, StoreError> {
    let sql = format!(
        "SELECT clips.last_used_at, clips.id
         FROM clips
         WHERE clips.normalized_text IS NOT NULL AND {}
         ORDER BY clips.last_used_at {}, clips.id {}
         LIMIT 1 OFFSET 1999",
        page.predicate("clips"),
        page.order,
        page.order
    );
    connection
        .query_row(
            &sql,
            named_params! {
                ":anchor_time": page.anchor.last_used_at_ms,
                ":anchor_id": page.anchor.id.0,
            },
            |row| {
                Ok(HistoryCursor {
                    last_used_at_ms: row.get(0)?,
                    id: ClipId(row.get(1)?),
                })
            },
        )
        .optional()
        .map_err(Into::into)
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

/// A `LIKE` comparison: the operand, plus the `ESCAPE` clause it needs.
///
/// Callers get the whole comparison from `constraint` rather than pasting the
/// clause themselves, because forgetting to paste it is silent -- the query
/// still returns rows, it just stops escaping them.
///
/// The clause is only emitted when the needle actually carries a wildcard.
/// fts5's trigram tokenizer refuses to take a `LIKE` over into the trigram
/// index as soon as the comparison has an ESCAPE, so escaping unconditionally
/// demoted every substring search to a full scan of the index content. The
/// plain `clips` predicates keep their index either way; only the fts5 path
/// cares. Escaping is still correct when a wildcard is present -- that query
/// simply falls back to a scan, which is what it did before.
///
/// The takeover is a win exactly while the needle narrows things down. Measured
/// over 100k text clips: 2 hits went 15ms -> under 1ms and 10k hits went
/// 17ms -> 10ms, but 50k hits went 27ms -> 37ms and a needle matching every row
/// went 28ms -> 63ms. The crossover sits somewhere above a third of the table,
/// where the result set is too broad to be worth reading anyway. Picking the
/// plan per needle would need selectivity the planner does not have; that
/// belongs with the ranking rework, not here.
struct LikePattern {
    value: String,
    escape: &'static str,
}

impl LikePattern {
    fn new(mode: MatchMode, needle: &str) -> Self {
        let needs_escape = needle.contains(['\\', '%', '_']);
        let escaped = if needs_escape {
            needle
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        } else {
            needle.to_owned()
        };
        Self {
            value: match mode {
                MatchMode::Prefix => format!("{escaped}%"),
                MatchMode::Substring => format!("%{escaped}%"),
            },
            escape: if needs_escape { " ESCAPE '\\'" } else { "" },
        }
    }

    /// The comparison against `column`, binding the operand to `:pattern`.
    fn constraint(&self, column: &str) -> String {
        format!("{column} LIKE :pattern{escape}", escape = self.escape)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{configure_connection, migrate};

    // Every statement below is built by the function production calls, so
    // editing the SQL moves the test with it. Handing EXPLAIN a hand-copied
    // query instead would let the two drift: putting an unconditional
    // `ESCAPE '\'` back into indexed_substring_sql is exactly the regression
    // asserted against here, and a copy would keep passing right through it.
    #[test]
    fn production_search_sql_keeps_prefix_and_trigram_indexes() {
        let mut connection = Connection::open_in_memory().unwrap();
        configure_connection(&connection, 1024).unwrap();
        migrate(&mut connection).unwrap();

        let older = PageSql::new(None, PageDirection::Older).unwrap();
        let newer = PageSql::new(
            Some(HistoryCursor {
                last_used_at_ms: 1_000,
                id: ClipId(500),
            }),
            PageDirection::Newer,
        )
        .unwrap();

        let short_needle = "alpha";
        let short_pattern = LikePattern::new(MatchMode::Prefix, short_needle);
        let short_key = prefix_equality_key(short_needle);
        assert!(short_key.is_none(), "a short needle takes the LIKE seek");
        let prefix_plan = explain(
            &connection,
            &prefix_sql(older, &short_pattern, short_key.is_some()),
            named_params! {
                ":pattern": short_pattern.value,
                ":key": short_key,
                ":anchor_time": older.anchor.last_used_at_ms,
                ":anchor_id": older.anchor.id.0,
                ":limit": 51_i64,
            },
        );
        // A case-insensitive LIKE can only seek an index collated the same way,
        // so a plain SCAN here means the NOCASE collation drifted apart.
        assert!(
            prefix_plan.contains("SEARCH c USING INDEX idx_clips_text_prefix"),
            "prefix expression index missing: {prefix_plan}"
        );

        // Needles longer than the indexed prefix compare it for equality
        // instead. Dropping the collation there still returns the right rows,
        // so only the plan can catch it falling back to a recency scan.
        let long_needle = "alpha".repeat(13);
        let long_pattern = LikePattern::new(MatchMode::Prefix, &long_needle);
        let long_key = prefix_equality_key(&long_needle);
        assert!(long_key.is_some(), "a long needle takes the equality seek");
        let long_prefix_plan = explain(
            &connection,
            &prefix_sql(older, &long_pattern, long_key.is_some()),
            named_params! {
                ":pattern": long_pattern.value,
                ":key": long_key,
                ":anchor_time": older.anchor.last_used_at_ms,
                ":anchor_id": older.anchor.id.0,
                ":limit": 51_i64,
            },
        );
        assert!(
            long_prefix_plan.contains("SEARCH c USING INDEX idx_clips_text_prefix"),
            "prefix equality seek missing: {long_prefix_plan}"
        );

        // fts5 appends "L0" to its plan detail only when it takes the LIKE over
        // into the trigram index; a bare "VIRTUAL TABLE INDEX 0:" is the full
        // scan of the index content. An ESCAPE clause suppresses the takeover,
        // which is why LikePattern only emits one for needles that need it.
        let substring_pattern = LikePattern::new(MatchMode::Substring, "alpha");
        let substring_plan = explain(
            &connection,
            &indexed_substring_sql(older, &substring_pattern),
            named_params! {
                ":pattern": substring_pattern.value,
                ":anchor_time": older.anchor.last_used_at_ms,
                ":anchor_id": older.anchor.id.0,
                ":limit": 51_i64,
            },
        );
        assert!(
            substring_plan.contains("VIRTUAL TABLE INDEX 0:L0"),
            "FTS5 trigram takeover missing: {substring_plan}"
        );

        let recent_page_plan = explain(
            &connection,
            &recent_page_sql(older),
            named_params! {
                ":anchor_time": older.anchor.last_used_at_ms,
                ":anchor_id": older.anchor.id.0,
                ":limit": 51_i64,
            },
        );
        assert!(
            recent_page_plan.contains("idx_clips_recent"),
            "recent keyset index missing: {recent_page_plan}"
        );

        let newer_page_plan = explain(
            &connection,
            &recent_page_sql(newer),
            named_params! {
                ":anchor_time": newer.anchor.last_used_at_ms,
                ":anchor_id": newer.anchor.id.0,
                ":limit": 51_i64,
            },
        );
        assert!(
            newer_page_plan.contains("idx_clips_recent"),
            "newer keyset index missing: {newer_page_plan}"
        );

        // The bounded scan is only bounded because its inner query walks the
        // recency index and stops at 2000 rows. Losing the index there turns
        // every one- or two-character needle into a full table scan.
        let scan_pattern = LikePattern::new(MatchMode::Substring, "al");
        let recent_scan_plan = explain(
            &connection,
            &recent_scan_sql(older, &scan_pattern),
            named_params! {
                ":pattern": scan_pattern.value,
                ":anchor_time": older.anchor.last_used_at_ms,
                ":anchor_id": older.anchor.id.0,
                ":limit": 51_i64,
            },
        );
        assert!(
            recent_scan_plan.contains("idx_clips_recent"),
            "bounded scan lost its recency index: {recent_scan_plan}"
        );
    }

    #[test]
    fn like_pattern_escapes_only_when_the_needle_carries_a_wildcard() {
        // No wildcard: no ESCAPE, so the fts5 trigram takeover stays available.
        let plain = LikePattern::new(MatchMode::Substring, "alpha");
        assert_eq!(plain.value, "%alpha%");
        assert_eq!(
            plain.constraint("clips_fts.normalized_text"),
            "clips_fts.normalized_text LIKE :pattern"
        );

        // A wildcard still has to be escaped for correctness. Losing the
        // takeover for these needles is the deliberate trade.
        let percent = LikePattern::new(MatchMode::Substring, "100%");
        assert_eq!(percent.value, "%100\\%%");
        assert_eq!(
            percent.constraint("clips_fts.normalized_text"),
            "clips_fts.normalized_text LIKE :pattern ESCAPE '\\'"
        );

        let underscore = LikePattern::new(MatchMode::Prefix, "a_b");
        assert_eq!(underscore.value, "a\\_b%");
        assert_eq!(underscore.escape, " ESCAPE '\\'");

        let backslash = LikePattern::new(MatchMode::Prefix, "a\\b");
        assert_eq!(backslash.value, "a\\\\b%");
        assert_eq!(backslash.escape, " ESCAPE '\\'");
    }

    fn explain<P: rusqlite::Params>(connection: &Connection, sql: &str, parameters: P) -> String {
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        statement
            .query_map(parameters, |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" | ")
    }
}
