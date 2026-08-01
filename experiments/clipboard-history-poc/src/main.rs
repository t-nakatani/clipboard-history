use rusqlite::{Connection, params};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const PAGE_SIZE: i64 = 100;
const QUERY_RUNS: usize = 200;
const UPSERT_CLIP_SQL: &str = "INSERT INTO clips(
         first_copied_at, last_used_at, normalized_text, content_kind,
         content_hash, payload_size, pinned, copy_count
     ) VALUES (?1, ?1, ?2, ?3, ?4, ?5, 0, 1)
     ON CONFLICT(content_hash) DO UPDATE SET
         last_used_at = excluded.last_used_at,
         copy_count = clips.copy_count + 1";

fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::parse()?;
    if config.db_path.exists() {
        fs::remove_file(&config.db_path)?;
    }

    let rss_before = current_rss_bytes();
    let mut connection = Connection::open(&config.db_path)?;
    configure(&connection)?;
    create_schema(&connection)?;

    let insert_started = Instant::now();
    insert_rows(&mut connection, config.count)?;
    let insert_elapsed = insert_started.elapsed();
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;

    let rss_after_insert = current_rss_bytes();
    let recent = benchmark_recent_page(&connection)?;
    let exact = benchmark_exact_hash(&connection, config.count / 2)?;
    let recopy = benchmark_recopy_touch(&connection, config.count / 2, config.count)?;
    let prefix = benchmark_prefix(&connection, "clipboard item 0000%")?;
    let substring_common = benchmark_substring(&connection, "%project alpha%")?;
    let substring_rare = benchmark_substring(&connection, "%needle-4242%")?;
    let query_plan = substring_query_plan(&connection)?;
    let prefix_query_plan = prefix_query_plan(&connection)?;
    if !query_plan.contains("VIRTUAL TABLE INDEX") {
        return Err(format!("FTS5 LIKE optimization is not active: {query_plan}").into());
    }
    if !prefix_query_plan.contains("idx_clips_text_prefix") {
        return Err(format!("prefix expression index is not active: {prefix_query_plan}").into());
    }
    let rss_after_queries = current_rss_bytes();
    let row_count: i64 =
        connection.query_row("SELECT count(*) FROM clips", [], |row| row.get(0))?;
    let db_size = database_size(&config.db_path);

    println!("count={row_count}");
    println!("insert_ms={:.3}", millis(insert_elapsed));
    println!(
        "insert_rows_per_second={:.1}",
        config.count as f64 / insert_elapsed.as_secs_f64()
    );
    println!("recent_page_p50_us={}", recent.p50_us);
    println!("recent_page_p95_us={}", recent.p95_us);
    println!("exact_hash_p50_us={}", exact.p50_us);
    println!("exact_hash_p95_us={}", exact.p95_us);
    println!("recopy_touch_p50_us={}", recopy.p50_us);
    println!("recopy_touch_p95_us={}", recopy.p95_us);
    println!("prefix_p50_us={}", prefix.p50_us);
    println!("prefix_p95_us={}", prefix.p95_us);
    println!("substring_common_p50_us={}", substring_common.p50_us);
    println!("substring_common_p95_us={}", substring_common.p95_us);
    println!("substring_rare_p50_us={}", substring_rare.p50_us);
    println!("substring_rare_p95_us={}", substring_rare.p95_us);
    println!("rss_before_bytes={rss_before}");
    println!("rss_after_insert_bytes={rss_after_insert}");
    println!("rss_after_queries_bytes={rss_after_queries}");
    println!("database_bytes={db_size}");
    println!("substring_query_plan={query_plan}");
    println!("prefix_query_plan={prefix_query_plan}");

    Ok(())
}

struct Config {
    count: usize,
    db_path: PathBuf,
}

impl Config {
    fn parse() -> Result<Self, Box<dyn Error>> {
        let mut args = env::args().skip(1);
        let count = args
            .next()
            .ok_or("usage: clipboard-history-poc <count> <db-path>")?
            .parse()?;
        let db_path = args
            .next()
            .map(PathBuf::from)
            .ok_or("usage: clipboard-history-poc <count> <db-path>")?;
        Ok(Self { count, db_path })
    }
}

fn configure(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA cache_size = -4096;
        PRAGMA mmap_size = 0;
        PRAGMA temp_store = MEMORY;
        PRAGMA foreign_keys = ON;
        PRAGMA case_sensitive_like = ON;
        ",
    )
}

fn create_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        CREATE TABLE clips (
            id                INTEGER PRIMARY KEY,
            first_copied_at   INTEGER NOT NULL,
            last_used_at      INTEGER NOT NULL,
            normalized_text   TEXT,
            content_kind      INTEGER NOT NULL,
            content_hash      BLOB NOT NULL UNIQUE,
            payload_size      INTEGER NOT NULL,
            pinned            INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
            copy_count        INTEGER NOT NULL DEFAULT 1
        ) STRICT;

        CREATE INDEX idx_clips_recent
            ON clips(last_used_at DESC, id DESC);
        CREATE INDEX idx_clips_retention
            ON clips(last_used_at, id) WHERE pinned = 0;
        CREATE INDEX idx_clips_text_prefix
            ON clips(substr(normalized_text, 1, 64))
            WHERE normalized_text IS NOT NULL;

        CREATE VIRTUAL TABLE clips_fts USING fts5(
            normalized_text,
            content = 'clips',
            content_rowid = 'id',
            tokenize = 'trigram'
        );

        CREATE TRIGGER clips_ai AFTER INSERT ON clips BEGIN
            INSERT INTO clips_fts(rowid, normalized_text)
            VALUES (new.id, new.normalized_text);
        END;
        CREATE TRIGGER clips_ad AFTER DELETE ON clips BEGIN
            INSERT INTO clips_fts(clips_fts, rowid, normalized_text)
            VALUES ('delete', old.id, old.normalized_text);
        END;
        CREATE TRIGGER clips_au AFTER UPDATE OF normalized_text ON clips BEGIN
            INSERT INTO clips_fts(clips_fts, rowid, normalized_text)
            VALUES ('delete', old.id, old.normalized_text);
            INSERT INTO clips_fts(rowid, normalized_text)
            VALUES (new.id, new.normalized_text);
        END;
        ",
    )
}

fn insert_rows(connection: &mut Connection, count: usize) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    {
        let mut statement = transaction.prepare_cached(UPSERT_CLIP_SQL)?;
        for index in 0..count {
            let text = synthetic_clip(index);
            let hash = canonical_clip_hash(&[Representation {
                uti: "public.utf8-plain-text",
                bytes: text.as_bytes(),
            }]);
            statement.execute(params![
                index as i64,
                text,
                ContentKind::Text as i64,
                hash.as_bytes().as_slice(),
                text.len() as i64
            ])?;
        }
    }
    transaction.commit()
}

fn synthetic_clip(index: usize) -> String {
    let category = match index % 5 {
        0 => "project alpha architecture",
        1 => "release checklist and review",
        2 => "meeting notes and follow up",
        3 => "rust sqlite performance experiment",
        _ => "日本語のクリップボード履歴と検索",
    };
    let rare = if index % 10_000 == 4_242 {
        " needle-4242"
    } else {
        ""
    };
    format!(
        "clipboard item {index:08} {category}{rare} — deterministic sample text for exact substring indexing and bounded-memory paging"
    )
}

fn benchmark_recent_page(connection: &Connection) -> rusqlite::Result<Latency> {
    benchmark(|| {
        let mut statement = connection.prepare_cached(
            "SELECT id, last_used_at, normalized_text
             FROM clips
             ORDER BY last_used_at DESC, id DESC
             LIMIT ?1",
        )?;
        let mut rows = statement.query([PAGE_SIZE])?;
        let mut seen = 0;
        while let Some(row) = rows.next()? {
            let _: i64 = row.get(0)?;
            let _: i64 = row.get(1)?;
            let _: Option<String> = row.get(2)?;
            seen += 1;
        }
        assert!(seen <= PAGE_SIZE);
        Ok(())
    })
}

fn benchmark_exact_hash(connection: &Connection, index: usize) -> rusqlite::Result<Latency> {
    let text = synthetic_clip(index);
    let hash = canonical_clip_hash(&[Representation {
        uti: "public.utf8-plain-text",
        bytes: text.as_bytes(),
    }]);
    benchmark(|| {
        let _: i64 = connection.query_row(
            "SELECT id FROM clips WHERE content_hash = ?1",
            [hash.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        Ok(())
    })
}

fn benchmark_recopy_touch(
    connection: &Connection,
    index: usize,
    initial_timestamp: usize,
) -> rusqlite::Result<Latency> {
    let text = synthetic_clip(index);
    let hash = canonical_clip_hash(&[Representation {
        uti: "public.utf8-plain-text",
        bytes: text.as_bytes(),
    }]);
    let mut timestamp = initial_timestamp as i64;
    benchmark(|| {
        timestamp += 1;
        connection.execute(
            UPSERT_CLIP_SQL,
            params![
                timestamp,
                text,
                ContentKind::Text as i64,
                hash.as_bytes().as_slice(),
                text.len() as i64
            ],
        )?;
        Ok(())
    })
}

fn benchmark_substring(connection: &Connection, pattern: &str) -> rusqlite::Result<Latency> {
    benchmark(|| {
        let mut statement = connection.prepare_cached(
            "SELECT rowid, normalized_text
             FROM clips_fts
             WHERE normalized_text LIKE ?1
             ORDER BY rowid DESC
             LIMIT ?2",
        )?;
        let mut rows = statement.query(params![pattern, PAGE_SIZE])?;
        while let Some(row) = rows.next()? {
            let _: i64 = row.get(0)?;
            let _: String = row.get(1)?;
        }
        Ok(())
    })
}

fn benchmark_prefix(connection: &Connection, pattern: &str) -> rusqlite::Result<Latency> {
    benchmark(|| {
        let mut statement = connection.prepare_cached(
            "SELECT id, normalized_text
             FROM clips
             WHERE substr(normalized_text, 1, 64) LIKE ?1
               AND normalized_text LIKE ?1
             ORDER BY last_used_at DESC, id DESC
             LIMIT ?2",
        )?;
        let mut rows = statement.query(params![pattern, PAGE_SIZE])?;
        while let Some(row) = rows.next()? {
            let _: i64 = row.get(0)?;
            let _: Option<String> = row.get(1)?;
        }
        Ok(())
    })
}

fn benchmark<F>(mut operation: F) -> rusqlite::Result<Latency>
where
    F: FnMut() -> rusqlite::Result<()>,
{
    for _ in 0..10 {
        operation()?;
    }
    let mut samples = Vec::with_capacity(QUERY_RUNS);
    for _ in 0..QUERY_RUNS {
        let started = Instant::now();
        operation()?;
        samples.push(started.elapsed().as_micros());
    }
    samples.sort_unstable();
    Ok(Latency {
        p50_us: samples[QUERY_RUNS / 2],
        p95_us: samples[QUERY_RUNS * 95 / 100],
    })
}

fn substring_query_plan(connection: &Connection) -> rusqlite::Result<String> {
    let mut statement = connection.prepare(
        "EXPLAIN QUERY PLAN
         SELECT rowid FROM clips_fts
         WHERE normalized_text LIKE '%project alpha%'
         ORDER BY rowid DESC LIMIT 100",
    )?;
    let plan = statement
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(plan.join(" | "))
}

fn prefix_query_plan(connection: &Connection) -> rusqlite::Result<String> {
    let mut statement = connection.prepare(
        "EXPLAIN QUERY PLAN
         SELECT id FROM clips
         WHERE substr(normalized_text, 1, 64) LIKE 'clipboard item 0000%'
           AND normalized_text LIKE 'clipboard item 0000%'
         ORDER BY last_used_at DESC, id DESC LIMIT 100",
    )?;
    let plan = statement
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(plan.join(" | "))
}

#[repr(i64)]
enum ContentKind {
    Text = 1,
}

struct Representation<'a> {
    uti: &'a str,
    bytes: &'a [u8],
}

fn canonical_clip_hash(representations: &[Representation<'_>]) -> blake3::Hash {
    let mut ordered = representations.iter().collect::<Vec<_>>();
    ordered.sort_unstable_by(|left, right| {
        left.uti
            .cmp(right.uti)
            .then_with(|| left.bytes.cmp(right.bytes))
    });

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"clipboard-history.clip.v1\0");
    for representation in ordered {
        hasher.update(&(representation.uti.len() as u64).to_le_bytes());
        hasher.update(representation.uti.as_bytes());
        hasher.update(&(representation.bytes.len() as u64).to_le_bytes());
        hasher.update(representation.bytes);
    }
    hasher.finalize()
}

struct Latency {
    p50_us: u128,
    p95_us: u128,
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn current_rss_bytes() -> u64 {
    let pid = std::process::id().to_string();
    let output = Command::new("ps").args(["-o", "rss=", "-p", &pid]).output();
    output
        .ok()
        .and_then(|result| String::from_utf8(result.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|kib| kib * 1024)
        .unwrap_or(0)
}

fn database_size(path: &Path) -> u64 {
    [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ]
    .iter()
    .filter_map(|candidate| fs::metadata(candidate).ok())
    .map(|metadata| metadata.len())
    .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_hash_does_not_depend_on_representation_order() {
        let first = canonical_clip_hash(&[
            Representation {
                uti: "public.html",
                bytes: b"<b>hello</b>",
            },
            Representation {
                uti: "public.utf8-plain-text",
                bytes: b"hello",
            },
        ]);
        let second = canonical_clip_hash(&[
            Representation {
                uti: "public.utf8-plain-text",
                bytes: b"hello",
            },
            Representation {
                uti: "public.html",
                bytes: b"<b>hello</b>",
            },
        ]);
        assert_eq!(first, second);
    }

    #[test]
    fn recopy_touches_existing_row_instead_of_inserting() -> rusqlite::Result<()> {
        let mut connection = Connection::open_in_memory()?;
        configure(&connection)?;
        create_schema(&connection)?;
        insert_rows(&mut connection, 1)?;

        let text = synthetic_clip(0);
        let hash = canonical_clip_hash(&[Representation {
            uti: "public.utf8-plain-text",
            bytes: text.as_bytes(),
        }]);
        connection.execute(
            UPSERT_CLIP_SQL,
            params![
                99_i64,
                text,
                ContentKind::Text as i64,
                hash.as_bytes().as_slice(),
                text.len() as i64
            ],
        )?;

        let (count, last_used_at, copy_count): (i64, i64, i64) = connection.query_row(
            "SELECT count(*), max(last_used_at), max(copy_count) FROM clips",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(count, 1);
        assert_eq!(last_used_at, 99);
        assert_eq!(copy_count, 2);
        Ok(())
    }
}
