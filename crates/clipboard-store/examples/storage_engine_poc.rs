use std::{
    collections::VecDeque,
    env,
    error::Error,
    fs::{self, OpenOptions},
    hint::black_box,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use clipboard_core::{
    CaptureOutcome, ClipKind, ClipboardSnapshot, HistoryRepository, HistoryService, Representation,
    SearchTextPolicy, canonical_clip_identity,
};
use clipboard_store::{
    PayloadHash, PayloadStore, StoreHandle, StoreOptions, configure_connection, migrate,
};
use rusqlite::{Connection, Transaction, params};

const PAGE_LIMIT: i64 = 100;
const QUERY_RUNS: usize = 200;
const PREFIX_PATTERN: &str = "clipboard item 0000%";
const SUBSTRING_PATTERN: &str = "%project alpha%";

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("seed") => {
            let path = required_path(&mut args, "database path")?;
            let count = required_usize(&mut args, "row count")?;
            seed(&path, count)?;
        }
        Some("cold") => {
            let path = required_path(&mut args, "database path")?;
            let cache_kib = required_usize(&mut args, "cache KiB")?;
            let warm_up = required_usize(&mut args, "warm-up 0/1")? != 0;
            cold_open(&path, cache_kib, warm_up)?;
        }
        Some("wal") => {
            let path = required_path(&mut args, "database path")?;
            let writes = required_usize(&mut args, "write count")?;
            wal_starvation(&path, writes)?;
        }
        Some("prune") => {
            let path = required_path(&mut args, "database path")?;
            let target = required_usize(&mut args, "target rows")?;
            let batch = required_usize(&mut args, "batch rows")?;
            prune_and_vacuum(&path, target, batch)?;
        }
        Some("overflow") => {
            let directory = required_path(&mut args, "output directory")?;
            overflow_matrix(&directory)?;
        }
        Some("payload") => {
            let directory = required_path(&mut args, "output directory")?;
            payload_matrix(&directory)?;
        }
        Some("crash-prepare") => {
            let directory = required_path(&mut args, "case directory")?;
            let stage = args.next().ok_or("missing crash stage")?;
            crash_prepare(&directory, &stage)?;
        }
        Some("crash-verify") => {
            let directory = required_path(&mut args, "case directory")?;
            let stage = args.next().ok_or("missing crash stage")?;
            crash_verify(&directory, &stage)?;
        }
        Some("soak") => {
            let directory = required_path(&mut args, "output directory")?;
            let operations = required_usize(&mut args, "operation count")?;
            let max_live = required_usize(&mut args, "maximum live clips")?;
            let payload_every = required_usize(&mut args, "payload interval")?;
            soak(&directory, operations, max_live, payload_every)?;
        }
        Some("orphan-scan") => {
            let directory = required_path(&mut args, "output directory")?;
            let files = required_usize(&mut args, "file count")?;
            orphan_scan(&directory, files)?;
        }
        _ => print_usage(),
    }
    Ok(())
}

fn print_usage() {
    eprintln!(
        "usage:\n  storage_engine_poc seed <db> <rows>\n  storage_engine_poc cold <db> <cache-kib> <warm-up:0|1>\n  storage_engine_poc wal <db> <writes>\n  storage_engine_poc prune <db> <target-rows> <batch-rows>\n  storage_engine_poc overflow <output-dir>\n  storage_engine_poc payload <output-dir>\n  storage_engine_poc crash-prepare <case-dir> <stage>\n  storage_engine_poc crash-verify <case-dir> <stage>\n  storage_engine_poc soak <output-dir> <operations> <max-live> <payload-every>\n  storage_engine_poc orphan-scan <output-dir> <files>"
    );
}

fn required_path(
    args: &mut impl Iterator<Item = String>,
    name: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}").into())
}

fn required_usize(
    args: &mut impl Iterator<Item = String>,
    name: &str,
) -> Result<usize, Box<dyn Error>> {
    args.next()
        .ok_or_else(|| format!("missing {name}"))?
        .parse()
        .map_err(Into::into)
}

fn seed(path: &Path, count: usize) -> Result<(), Box<dyn Error>> {
    remove_database_files(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut connection = Connection::open(path)?;
    configure_connection(&connection, 4 * 1024)?;
    migrate(&mut connection)?;

    let started = Instant::now();
    let transaction = connection.transaction()?;
    insert_synthetic_rows(&transaction, count)?;
    transaction.commit()?;
    let elapsed = started.elapsed();
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;

    println!("command=seed");
    println!("rows={count}");
    println!("elapsed_ms={:.3}", millis(elapsed));
    println!(
        "rows_per_second={:.1}",
        count as f64 / elapsed.as_secs_f64()
    );
    println!("database_bytes={}", database_bytes(path));
    print_page_stats(&connection)?;
    Ok(())
}

fn insert_synthetic_rows(transaction: &Transaction<'_>, count: usize) -> rusqlite::Result<()> {
    let mut statement = transaction.prepare_cached(
        "INSERT INTO clips(
            content_hash, content_kind, first_copied_at, last_used_at,
            pinned, copy_count, payload_size, normalized_text
         ) VALUES (?1, 0, ?2, ?2, ?3, 1, ?4, ?5)",
    )?;
    for index in 0..count {
        let text = synthetic_clip(index);
        let identity = canonical_clip_identity(&[Representation {
            uti: "public.utf8-plain-text".into(),
            bytes: text.as_bytes().to_vec(),
        }]);
        statement.execute(params![
            identity.0.as_slice(),
            index as i64,
            i64::from(index % 10_000 == 0),
            text.len() as i64,
            text,
        ])?;
    }
    Ok(())
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

fn cold_open(path: &Path, cache_kib: usize, warm_up: bool) -> Result<(), Box<dyn Error>> {
    let rss_before = current_rss_bytes();
    let open_started = Instant::now();
    let mut connection = Connection::open(path)?;
    configure_connection(&connection, cache_kib)?;
    migrate(&mut connection)?;
    let open_elapsed = open_started.elapsed();

    if warm_up {
        run_recent(&connection)?;
    }

    let first_recent = measured(|| run_recent(&connection))?;
    let first_prefix = measured(|| run_prefix(&connection))?;
    let first_substring = measured(|| run_substring(&connection))?;
    let recent = benchmark(|| run_recent(&connection), QUERY_RUNS)?;
    let prefix = benchmark(|| run_prefix(&connection), QUERY_RUNS)?;
    let substring = benchmark(|| run_substring(&connection), QUERY_RUNS)?;
    let quick_check = measured(|| {
        let result: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        assert_eq!(result, "ok");
        Ok(())
    })?;

    println!("command=cold");
    println!("cold_definition=new_process_and_connection_os_cache_uncontrolled");
    println!("cache_kib={cache_kib}");
    println!("warm_up={}", u8::from(warm_up));
    println!("open_us={}", open_elapsed.as_micros());
    println!("first_recent_us={}", first_recent.as_micros());
    println!("first_prefix_us={}", first_prefix.as_micros());
    println!("first_substring_us={}", first_substring.as_micros());
    recent.print("recent");
    prefix.print("prefix");
    substring.print("substring");
    println!("quick_check_us={}", quick_check.as_micros());
    println!("rss_before_bytes={rss_before}");
    println!("rss_after_bytes={}", current_rss_bytes());
    println!("prefix_query_plan={}", prefix_query_plan(&connection)?);
    println!(
        "substring_query_plan={}",
        substring_query_plan(&connection)?
    );
    print_page_stats(&connection)?;
    Ok(())
}

fn run_recent(connection: &Connection) -> rusqlite::Result<()> {
    let mut statement = connection.prepare_cached(
        "SELECT id, last_used_at, substr(normalized_text, 1, 256)
         FROM clips ORDER BY last_used_at DESC, id DESC LIMIT ?1",
    )?;
    let mut rows = statement.query([PAGE_LIMIT])?;
    while let Some(row) = rows.next()? {
        black_box(row.get::<_, i64>(0)?);
        black_box(row.get::<_, i64>(1)?);
        black_box(row.get::<_, Option<String>>(2)?);
    }
    Ok(())
}

fn run_prefix(connection: &Connection) -> rusqlite::Result<()> {
    let mut statement = connection.prepare_cached(
        "SELECT id, substr(normalized_text, 1, 256)
         FROM clips
         WHERE substr(normalized_text, 1, 64) LIKE ?1
           AND normalized_text LIKE ?1
         ORDER BY last_used_at DESC, id DESC LIMIT ?2",
    )?;
    let mut rows = statement.query(params![PREFIX_PATTERN, PAGE_LIMIT])?;
    while let Some(row) = rows.next()? {
        black_box(row.get::<_, i64>(0)?);
        black_box(row.get::<_, Option<String>>(1)?);
    }
    Ok(())
}

fn run_substring(connection: &Connection) -> rusqlite::Result<()> {
    let mut statement = connection.prepare_cached(
        "SELECT rowid, normalized_text FROM clips_fts
         WHERE normalized_text LIKE ?1
         ORDER BY rowid DESC LIMIT ?2",
    )?;
    let mut rows = statement.query(params![SUBSTRING_PATTERN, PAGE_LIMIT])?;
    while let Some(row) = rows.next()? {
        black_box(row.get::<_, i64>(0)?);
        black_box(row.get::<_, String>(1)?);
    }
    Ok(())
}

fn prefix_query_plan(connection: &Connection) -> rusqlite::Result<String> {
    explain(
        connection,
        "EXPLAIN QUERY PLAN
         SELECT id FROM clips
         WHERE substr(normalized_text, 1, 64) LIKE 'clipboard item 0000%'
           AND normalized_text LIKE 'clipboard item 0000%'
         ORDER BY last_used_at DESC, id DESC LIMIT 100",
    )
}

fn substring_query_plan(connection: &Connection) -> rusqlite::Result<String> {
    explain(
        connection,
        "EXPLAIN QUERY PLAN
         SELECT rowid FROM clips_fts
         WHERE normalized_text LIKE '%project alpha%'
         ORDER BY rowid DESC LIMIT 100",
    )
}

fn explain(connection: &Connection, sql: &str) -> rusqlite::Result<String> {
    let mut statement = connection.prepare(sql)?;
    let plan = statement
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(plan.join(" | "))
}

fn wal_starvation(path: &Path, writes: usize) -> Result<(), Box<dyn Error>> {
    let writer = Connection::open(path)?;
    configure_connection(&writer, 4 * 1024)?;
    writer.pragma_update(None, "wal_autocheckpoint", 0)?;
    writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;

    let baseline_started = Instant::now();
    write_touch_batches(&writer, writes, 100)?;
    let baseline_write = baseline_started.elapsed();
    let baseline_wal = wal_bytes(path);
    let baseline_checkpoint = checkpoint(&writer, "TRUNCATE")?;
    let baseline_after = wal_bytes(path);

    let reader = Connection::open(path)?;
    configure_connection(&reader, 1024)?;
    reader.execute_batch("BEGIN")?;
    let _: i64 = reader.query_row("SELECT max(id) FROM clips", [], |row| row.get(0))?;

    let held_started = Instant::now();
    write_touch_batches(&writer, writes, 100)?;
    let held_write = held_started.elapsed();
    let held_wal = wal_bytes(path);
    let held_checkpoint = checkpoint(&writer, "PASSIVE")?;
    let held_after_checkpoint = wal_bytes(path);
    reader.execute_batch("COMMIT")?;
    let released_checkpoint = checkpoint(&writer, "TRUNCATE")?;
    let released_wal = wal_bytes(path);

    println!("command=wal");
    println!("writes_per_phase={writes}");
    println!("baseline_write_ms={:.3}", millis(baseline_write));
    println!("baseline_wal_before_checkpoint={baseline_wal}");
    baseline_checkpoint.print("baseline_checkpoint");
    println!("baseline_wal_after_checkpoint={baseline_after}");
    println!("held_reader_write_ms={:.3}", millis(held_write));
    println!("held_reader_wal_before_checkpoint={held_wal}");
    held_checkpoint.print("held_reader_checkpoint");
    println!("held_reader_wal_after_checkpoint={held_after_checkpoint}");
    released_checkpoint.print("released_checkpoint");
    println!("released_reader_wal_after_truncate={released_wal}");
    Ok(())
}

fn write_touch_batches(
    connection: &Connection,
    writes: usize,
    batch_size: usize,
) -> rusqlite::Result<()> {
    let row_count: i64 =
        connection.query_row("SELECT count(*) FROM clips", [], |row| row.get(0))?;
    for batch_start in (0..writes).step_by(batch_size) {
        connection.execute_batch("BEGIN IMMEDIATE")?;
        let batch_end = (batch_start + batch_size).min(writes);
        for index in batch_start..batch_end {
            let id = (index as i64 % row_count) + 1;
            connection.execute(
                "UPDATE clips SET last_used_at = last_used_at + 1 WHERE id = ?1",
                [id],
            )?;
        }
        connection.execute_batch("COMMIT")?;
    }
    Ok(())
}

fn checkpoint(connection: &Connection, mode: &str) -> rusqlite::Result<Checkpoint> {
    connection.query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |row| {
        Ok(Checkpoint {
            busy: row.get(0)?,
            log_frames: row.get(1)?,
            checkpointed_frames: row.get(2)?,
        })
    })
}

struct Checkpoint {
    busy: i64,
    log_frames: i64,
    checkpointed_frames: i64,
}

impl Checkpoint {
    fn print(&self, prefix: &str) {
        println!("{prefix}_busy={}", self.busy);
        println!("{prefix}_log_frames={}", self.log_frames);
        println!("{prefix}_checkpointed_frames={}", self.checkpointed_frames);
    }
}

fn prune_and_vacuum(path: &Path, target: usize, batch: usize) -> Result<(), Box<dyn Error>> {
    let connection = Connection::open(path)?;
    configure_connection(&connection, 4 * 1024)?;
    connection.pragma_update(None, "wal_autocheckpoint", 0)?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;

    let rows_before = row_count(&connection)?;
    let file_before = database_bytes(path);
    let freelist_before = freelist_count(&connection)?;
    let mut latencies = Vec::new();
    let prune_started = Instant::now();

    while row_count(&connection)? > target as i64 {
        let excess = (row_count(&connection)? - target as i64).min(batch as i64);
        let started = Instant::now();
        connection.execute(
            "DELETE FROM clips WHERE id IN (
                SELECT id FROM clips
                WHERE pinned = 0
                ORDER BY last_used_at ASC, id ASC
                LIMIT ?1
             )",
            [excess],
        )?;
        latencies.push(started.elapsed());
    }
    let prune_elapsed = prune_started.elapsed();
    checkpoint(&connection, "TRUNCATE")?;
    let rows_after_prune = row_count(&connection)?;
    let freelist_after_prune = freelist_count(&connection)?;
    let file_after_prune = database_bytes(path);

    let fts_optimize_started = Instant::now();
    connection.execute("INSERT INTO clips_fts(clips_fts) VALUES ('optimize')", [])?;
    let fts_optimize_elapsed = fts_optimize_started.elapsed();
    checkpoint(&connection, "TRUNCATE")?;
    let freelist_after_fts_optimize = freelist_count(&connection)?;
    let file_after_fts_optimize = database_bytes(path);

    let vacuum_started = Instant::now();
    let mut vacuum_calls = 0_u64;
    let mut vacuum_latencies = Vec::new();
    let mut stalled_calls = 0_u64;
    let mut previous_freelist = freelist_count(&connection)?;
    while previous_freelist > 0 && stalled_calls < 100 {
        let started = Instant::now();
        connection.execute_batch("PRAGMA incremental_vacuum(256)")?;
        vacuum_latencies.push(started.elapsed());
        vacuum_calls += 1;
        let current_freelist = freelist_count(&connection)?;
        if current_freelist < previous_freelist {
            stalled_calls = 0;
        } else {
            stalled_calls += 1;
        }
        previous_freelist = current_freelist;
    }
    checkpoint(&connection, "TRUNCATE")?;
    let vacuum_elapsed = vacuum_started.elapsed();
    let freelist_after_vacuum = freelist_count(&connection)?;
    let file_after_vacuum = database_bytes(path);

    let delete_latency = Latency::from_durations(latencies);
    let vacuum_latency = Latency::from_durations(vacuum_latencies);
    println!("command=prune");
    println!("rows_before={rows_before}");
    println!("rows_after_prune={rows_after_prune}");
    println!("batch_size={batch}");
    println!("prune_total_ms={:.3}", millis(prune_elapsed));
    delete_latency.print("delete_batch");
    println!("freelist_before={freelist_before}");
    println!("freelist_after_prune={freelist_after_prune}");
    println!("database_before_bytes={file_before}");
    println!("database_after_prune_bytes={file_after_prune}");
    println!("fts_optimize_ms={:.3}", millis(fts_optimize_elapsed));
    println!("freelist_after_fts_optimize={freelist_after_fts_optimize}");
    println!("database_after_fts_optimize_bytes={file_after_fts_optimize}");
    println!("vacuum_calls={vacuum_calls}");
    println!("vacuum_total_ms={:.3}", millis(vacuum_elapsed));
    vacuum_latency.print("vacuum_call");
    println!("freelist_after_vacuum={freelist_after_vacuum}");
    println!("database_after_vacuum_bytes={file_after_vacuum}");
    Ok(())
}

fn overflow_matrix(directory: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(directory)?;
    println!("command=overflow");
    for size in [512_usize, 2 * 1024, 16 * 1024, 128 * 1024, 1024 * 1024] {
        let path = directory.join(format!("overflow-{size}.sqlite"));
        remove_database_files(&path)?;
        let connection = Connection::open(&path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA synchronous=OFF;
             PRAGMA cache_size=-4096;
             PRAGMA mmap_size=0;
             CREATE TABLE samples(
                id INTEGER PRIMARY KEY,
                created_at INTEGER NOT NULL,
                content_hash BLOB NOT NULL,
                payload BLOB NOT NULL
             );",
        )?;
        let payload = vec![b'x'; size];
        connection.execute_batch("BEGIN")?;
        for index in 0..100 {
            let hash = blake3::hash(&[payload.as_slice(), &(index as u64).to_le_bytes()].concat());
            connection.execute(
                "INSERT INTO samples(created_at, content_hash, payload) VALUES (?1, ?2, ?3)",
                params![index, hash.as_bytes().as_slice(), payload],
            )?;
        }
        connection.execute_batch("COMMIT")?;

        let metadata = benchmark(
            || {
                black_box(connection.query_row(
                    "SELECT created_at, content_hash FROM samples WHERE id=50",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )?);
                Ok(())
            },
            100,
        )?;
        let payload_read = benchmark(
            || {
                let value: Vec<u8> =
                    connection.query_row("SELECT payload FROM samples WHERE id=50", [], |row| {
                        row.get(0)
                    })?;
                black_box(value);
                Ok(())
            },
            100,
        )?;
        let overflow_pages = dbstat_overflow_pages(&connection).unwrap_or(-1);
        println!("size_{size}_database_bytes={}", database_bytes(&path));
        println!("size_{size}_overflow_pages={overflow_pages}");
        metadata.print(&format!("size_{size}_metadata"));
        payload_read.print(&format!("size_{size}_payload"));
    }
    Ok(())
}

fn dbstat_overflow_pages(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT count(*) FROM dbstat WHERE name='samples' AND pagetype='overflow'",
        [],
        |row| row.get(0),
    )
}

fn payload_matrix(directory: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(directory)?;
    let store = PayloadStore::new(directory.join("payloads"));
    println!("command=payload");
    for size in [2 * 1024_usize, 16 * 1024, 128 * 1024, 1024 * 1024] {
        let mut write_durations = Vec::new();
        let mut last_bytes = Vec::new();
        let mut last_path = None;
        for index in 0_u64..20 {
            let mut bytes = vec![b'p'; size];
            bytes[..8].copy_from_slice(&index.to_le_bytes());
            let started = Instant::now();
            let stored = store.put(&bytes)?;
            write_durations.push(started.elapsed());
            assert!(stored.created);
            last_path = Some(stored.path);
            last_bytes = bytes;
        }
        let mut dedup_durations = Vec::new();
        for _ in 0..50 {
            let started = Instant::now();
            let dedup = store.put(&last_bytes)?;
            dedup_durations.push(started.elapsed());
            assert!(!dedup.created);
        }
        Latency::from_durations(write_durations).print(&format!("size_{size}_write"));
        Latency::from_durations(dedup_durations).print(&format!("size_{size}_dedup"));
        println!(
            "size_{size}_file_bytes={}",
            fs::metadata(last_path.expect("at least one payload"))?.len()
        );
    }
    println!("rss_after_bytes={}", current_rss_bytes());
    Ok(())
}

fn crash_prepare(directory: &Path, stage: &str) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(directory)?;
    let database = directory.join("history.sqlite");
    let payload_directory = directory.join("payloads");
    let options = StoreOptions::new(&database, &payload_directory);
    let payload = crash_payload();

    match stage {
        "staged_temp" => {
            drop(StoreHandle::open(options)?);
            let stage_directory = payload_directory.join("aa").join("bb");
            fs::create_dir_all(&stage_directory)?;
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(stage_directory.join(".stage-crash-injected"))?;
            file.write_all(&payload[..payload.len() / 2])?;
            file.sync_all()?;
        }
        "after_rename_before_row" => {
            drop(StoreHandle::open(options)?);
            PayloadStore::new(payload_directory).put(&payload)?;
        }
        "after_row_commit" => {
            let service = crash_service(options, payload.clone())?;
            std::mem::forget(service);
        }
        "after_delete_commit" => {
            let service = crash_service(options, payload.clone())?;
            let id = service.repository().recent(1)?[0].id;
            service.repository().delete(id)?;
            std::mem::forget(service);
        }
        "after_file_delete" => {
            let service = crash_service(options, payload.clone())?;
            let id = service.repository().recent(1)?[0].id;
            service.repository().delete(id)?;
            PayloadStore::new(payload_directory).remove_if_exists(PayloadHash::of(&payload))?;
            std::mem::forget(service);
        }
        _ => return Err(format!("unknown crash stage: {stage}").into()),
    }

    // exit() skips Rust destructors and leaves recovery to the next process.
    std::process::exit(86);
}

fn crash_service(
    options: StoreOptions,
    payload: Vec<u8>,
) -> Result<HistoryService<StoreHandle>, Box<dyn Error>> {
    let service = HistoryService::new(StoreHandle::open(options)?, SearchTextPolicy::default());
    let outcome = service.capture(
        ClipboardSnapshot {
            representations: vec![Representation {
                uti: "public.data".into(),
                bytes: payload,
            }],
            image_preview: None,
        },
        ClipKind::Image,
        1,
    )?;
    assert!(matches!(outcome, CaptureOutcome::Stored(_)));
    Ok(service)
}

fn crash_verify(directory: &Path, stage: &str) -> Result<(), Box<dyn Error>> {
    let database = directory.join("history.sqlite");
    let payload_directory = directory.join("payloads");
    let store = StoreHandle::open(StoreOptions::new(&database, &payload_directory))?;
    let recovery = store.recover_orphans()?;
    let stats = store.collect_garbage(100)?;
    drop(store);

    let connection = Connection::open(&database)?;
    configure_connection(&connection, 1024)?;
    let clips = scalar(&connection, "SELECT count(*) FROM clips")?;
    let representations = scalar(&connection, "SELECT count(*) FROM representations")?;
    let payload_rows = scalar(&connection, "SELECT count(*) FROM payloads")?;
    let queue = scalar(&connection, "SELECT count(*) FROM payload_gc_queue")?;
    let dangling = scalar(
        &connection,
        "SELECT count(*) FROM representations AS r
         LEFT JOIN payloads AS p ON p.payload_hash = r.payload_hash
         WHERE r.payload_hash IS NOT NULL AND p.payload_hash IS NULL",
    )?;
    let files = count_payload_files(&payload_directory)?;

    assert_eq!(dangling, 0, "committed representation lost its payload row");
    match stage {
        "after_row_commit" => {
            assert_eq!(
                (clips, representations, payload_rows, queue, files),
                (1, 1, 1, 0, 1)
            );
        }
        "staged_temp" | "after_rename_before_row" | "after_delete_commit" | "after_file_delete" => {
            assert_eq!(
                (clips, representations, payload_rows, queue, files),
                (0, 0, 0, 0, 0)
            );
        }
        _ => return Err(format!("unknown crash stage: {stage}").into()),
    }

    println!("stage={stage}");
    println!("clips={clips}");
    println!("representations={representations}");
    println!("payload_rows={payload_rows}");
    println!("gc_queue={queue}");
    println!("payload_files={files}");
    println!("dangling_references={dangling}");
    println!("gc_queued_scanned={}", stats.queued_scanned);
    println!("gc_payload_files_deleted={}", stats.payload_files_deleted);
    println!("gc_missing_payload_files={}", stats.missing_payload_files);
    println!("gc_orphan_files_deleted={}", recovery.orphan_files_deleted);
    println!("gc_staged_files_deleted={}", recovery.staged_files_deleted);
    Ok(())
}

fn crash_payload() -> Vec<u8> {
    vec![0x5a; 32 * 1024]
}

fn scalar(connection: &Connection, sql: &str) -> rusqlite::Result<i64> {
    connection.query_row(sql, [], |row| row.get(0))
}

fn count_payload_files(root: &Path) -> Result<i64, Box<dyn Error>> {
    if !root.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for first in fs::read_dir(root)? {
        let first = first?;
        if !first.file_type()?.is_dir() {
            continue;
        }
        for second in fs::read_dir(first.path())? {
            let second = second?;
            if !second.file_type()?.is_dir() {
                continue;
            }
            for file in fs::read_dir(second.path())? {
                if file?.file_type()?.is_file() {
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

fn soak(
    directory: &Path,
    operations: usize,
    max_live: usize,
    payload_every: usize,
) -> Result<(), Box<dyn Error>> {
    if payload_every == 0 || max_live == 0 {
        return Err("max-live and payload-every must be greater than zero".into());
    }
    fs::create_dir_all(directory)?;
    let database = directory.join("history.sqlite");
    let payload_directory = directory.join("payloads");
    let service = HistoryService::new(
        StoreHandle::open(StoreOptions::new(&database, &payload_directory))?,
        SearchTextPolicy::default(),
    );
    let mut live = VecDeque::with_capacity(max_live + 1);
    let mut capture_latency = SampledLatency::default();
    let mut delete_latency = SampledLatency::default();
    let mut gc_latency = SampledLatency::default();
    let mut checkpoint_latency = SampledLatency::default();
    let mut max_wal_bytes = 0_u64;
    let mut max_rss_bytes = current_rss_bytes();
    let mut rss_quarters = [0_u64; 4];
    let started = Instant::now();

    for index in 0..operations {
        let text = format!(
            "soak clipboard item {index:010} project alpha sustained capture delete checkpoint"
        );
        let mut representations = vec![Representation {
            uti: "public.utf8-plain-text".into(),
            bytes: text.into_bytes(),
        }];
        if index % payload_every == 0 {
            let mut payload = vec![0x6b; 32 * 1024];
            payload[..8].copy_from_slice(&(index as u64).to_le_bytes());
            representations.push(Representation {
                uti: "public.data".into(),
                bytes: payload,
            });
        }

        let capture_started = Instant::now();
        let outcome = service.capture(
            ClipboardSnapshot {
                representations,
                image_preview: None,
            },
            if index % payload_every == 0 {
                ClipKind::Mixed
            } else {
                ClipKind::Text
            },
            index as i64,
        )?;
        if index % 10 == 0 {
            capture_latency.record(capture_started.elapsed());
        }
        let CaptureOutcome::Stored(stored) = outcome else {
            return Err("soak capture was not stored".into());
        };
        live.push_back(stored.id);

        if live.len() > max_live {
            let id = live.pop_front().expect("live queue is not empty");
            let delete_started = Instant::now();
            service.repository().delete(id)?;
            if index % 10 == 0 {
                delete_latency.record(delete_started.elapsed());
            }
        }

        if index % 1_000 == 999 {
            let gc_started = Instant::now();
            service.repository().collect_garbage(1_000)?;
            gc_latency.record(gc_started.elapsed());
            let stats = service.repository().stats()?;
            max_wal_bytes = max_wal_bytes.max(stats.wal_bytes);
        }
        if index % 5_000 == 4_999 {
            let checkpoint_started = Instant::now();
            service.repository().checkpoint_passive()?;
            checkpoint_latency.record(checkpoint_started.elapsed());
        }

        for (quarter, sample) in rss_quarters.iter_mut().enumerate() {
            if index + 1 == operations * (quarter + 1) / 4 {
                *sample = current_rss_bytes();
                max_rss_bytes = max_rss_bytes.max(*sample);
            }
        }
    }

    loop {
        let stats = service.repository().collect_garbage(1_000)?;
        if stats.queued_scanned == 0 {
            break;
        }
    }
    service.repository().checkpoint_passive()?;
    let elapsed = started.elapsed();
    let final_stats = service.repository().stats()?;
    service.repository().quick_check()?;
    let truncate = service.repository().checkpoint_truncate()?;
    let wal_after_truncate = service.repository().stats()?.wal_bytes;
    drop(service);

    let connection = Connection::open(&database)?;
    configure_connection(&connection, 1024)?;
    let final_rows = scalar(&connection, "SELECT count(*) FROM clips")?;
    let final_queue = scalar(&connection, "SELECT count(*) FROM payload_gc_queue")?;
    let final_payload_rows = scalar(&connection, "SELECT count(*) FROM payloads")?;
    let freelist = freelist_count(&connection)?;

    println!("command=soak");
    println!("operations={operations}");
    println!(
        "equivalent_days_at_2s_interval={:.3}",
        operations as f64 * 2.0 / 86_400.0
    );
    println!("max_live={max_live}");
    println!("payload_every={payload_every}");
    println!("elapsed_ms={:.3}", millis(elapsed));
    println!(
        "operations_per_second={:.1}",
        operations as f64 / elapsed.as_secs_f64()
    );
    println!("latency_sample_every=10");
    capture_latency.print("capture");
    delete_latency.print("delete");
    gc_latency.print("gc");
    checkpoint_latency.print("checkpoint");
    println!("rss_25_bytes={}", rss_quarters[0]);
    println!("rss_50_bytes={}", rss_quarters[1]);
    println!("rss_75_bytes={}", rss_quarters[2]);
    println!("rss_100_bytes={}", rss_quarters[3]);
    println!("max_rss_bytes={max_rss_bytes}");
    println!("max_observed_wal_bytes={max_wal_bytes}");
    println!("final_wal_bytes={}", final_stats.wal_bytes);
    println!("truncate_checkpoint_busy={}", truncate.busy);
    println!("wal_after_truncate_bytes={wal_after_truncate}");
    println!("final_rows={final_rows}");
    println!("final_gc_queue={final_queue}");
    println!("final_payload_rows={final_payload_rows}");
    println!(
        "final_payload_files={}",
        count_payload_files(&payload_directory)?
    );
    println!("freelist_count={freelist}");
    println!("database_bytes={}", database_bytes(&database));
    Ok(())
}

fn orphan_scan(directory: &Path, files: usize) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(directory)?;
    let database = directory.join("history.sqlite");
    let payload_directory = directory.join("payloads");
    drop(StoreHandle::open(StoreOptions::new(
        &database,
        &payload_directory,
    ))?);

    let payload_store = PayloadStore::new(&payload_directory);
    let mut connection = Connection::open(&database)?;
    configure_connection(&connection, 1024)?;
    let create_started = Instant::now();
    let transaction = connection.transaction()?;
    for index in 0..files {
        let hash = PayloadHash(*blake3::hash(&(index as u64).to_le_bytes()).as_bytes());
        let path = payload_store.path_for(hash);
        fs::create_dir_all(path.parent().expect("payload path has parent"))?;
        fs::write(&path, (index as u64).to_le_bytes())?;
        if index % 10 != 0 {
            transaction.execute(
                "INSERT INTO payloads(payload_hash, payload_size, created_at)
                 VALUES (?1, 8, ?2)",
                params![hash.0.as_slice(), index as i64],
            )?;
        }
    }
    transaction.commit()?;
    drop(connection);
    let create_elapsed = create_started.elapsed();

    let rss_before = current_rss_bytes();
    let store = StoreHandle::open(StoreOptions::new(&database, &payload_directory))?;
    let scan_started = Instant::now();
    let recovery = store.recover_orphans()?;
    let scan_elapsed = scan_started.elapsed();
    let rss_after = current_rss_bytes();
    drop(store);
    let remaining_files = count_payload_files(&payload_directory)?;

    println!("command=orphan-scan");
    println!("files_before={files}");
    println!("expected_orphans={}", files.div_ceil(10));
    println!("create_ms={:.3}", millis(create_elapsed));
    println!("scan_ms={:.3}", millis(scan_elapsed));
    println!(
        "scan_files_per_second={:.1}",
        files as f64 / scan_elapsed.as_secs_f64()
    );
    println!("orphan_files_deleted={}", recovery.orphan_files_deleted);
    println!("staged_files_deleted={}", recovery.staged_files_deleted);
    println!("files_after={remaining_files}");
    println!("rss_before_bytes={rss_before}");
    println!("rss_after_bytes={rss_after}");
    println!("rss_delta_bytes={}", rss_after.saturating_sub(rss_before));
    Ok(())
}

#[derive(Default)]
struct SampledLatency {
    micros: Vec<u32>,
}

impl SampledLatency {
    fn record(&mut self, duration: Duration) {
        self.micros
            .push(duration.as_micros().min(u32::MAX as u128) as u32);
    }

    fn print(&mut self, prefix: &str) {
        if self.micros.is_empty() {
            println!("{prefix}_samples=0");
            return;
        }
        self.micros.sort_unstable();
        println!("{prefix}_samples={}", self.micros.len());
        println!("{prefix}_p50_us={}", sampled_percentile(&self.micros, 50));
        println!("{prefix}_p95_us={}", sampled_percentile(&self.micros, 95));
        println!("{prefix}_p99_us={}", sampled_percentile(&self.micros, 99));
        println!("{prefix}_max_us={}", self.micros[self.micros.len() - 1]);
    }
}

fn sampled_percentile(values: &[u32], percentile: usize) -> u32 {
    values[(values.len() - 1) * percentile / 100]
}

fn benchmark<F>(mut operation: F, runs: usize) -> rusqlite::Result<Latency>
where
    F: FnMut() -> rusqlite::Result<()>,
{
    for _ in 0..10 {
        operation()?;
    }
    let mut durations = Vec::with_capacity(runs);
    for _ in 0..runs {
        let started = Instant::now();
        operation()?;
        durations.push(started.elapsed());
    }
    Ok(Latency::from_durations(durations))
}

fn measured<F>(operation: F) -> rusqlite::Result<Duration>
where
    F: FnOnce() -> rusqlite::Result<()>,
{
    let started = Instant::now();
    operation()?;
    Ok(started.elapsed())
}

#[derive(Clone, Copy, Debug, Default)]
struct Latency {
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
}

impl Latency {
    fn from_durations(mut durations: Vec<Duration>) -> Self {
        if durations.is_empty() {
            return Self::default();
        }
        durations.sort_unstable();
        Self {
            p50_us: percentile(&durations, 50).as_micros(),
            p95_us: percentile(&durations, 95).as_micros(),
            p99_us: percentile(&durations, 99).as_micros(),
        }
    }

    fn print(&self, prefix: &str) {
        println!("{prefix}_p50_us={}", self.p50_us);
        println!("{prefix}_p95_us={}", self.p95_us);
        println!("{prefix}_p99_us={}", self.p99_us);
    }
}

fn percentile(values: &[Duration], percentile: usize) -> Duration {
    values[(values.len() - 1) * percentile / 100]
}

fn row_count(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row("SELECT count(*) FROM clips", [], |row| row.get(0))
}

fn freelist_count(connection: &Connection) -> rusqlite::Result<i64> {
    connection.pragma_query_value(None, "freelist_count", |row| row.get(0))
}

fn print_page_stats(connection: &Connection) -> rusqlite::Result<()> {
    let page_size: i64 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let page_count: i64 = connection.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let freelist = freelist_count(connection)?;
    let auto_vacuum: i64 = connection.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
    println!("page_size={page_size}");
    println!("page_count={page_count}");
    println!("freelist_count={freelist}");
    println!("auto_vacuum={auto_vacuum}");
    Ok(())
}

fn current_rss_bytes() -> u64 {
    let pid = std::process::id().to_string();
    Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()
        .and_then(|result| String::from_utf8(result.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|kib| kib * 1024)
        .unwrap_or(0)
}

fn database_bytes(path: &Path) -> u64 {
    [path.to_path_buf(), wal_path(path), shm_path(path)]
        .iter()
        .filter_map(|candidate| fs::metadata(candidate).ok())
        .map(|metadata| metadata.len())
        .sum()
}

fn wal_bytes(path: &Path) -> u64 {
    fs::metadata(wal_path(path))
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn wal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

fn shm_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", path.display()))
}

fn remove_database_files(path: &Path) -> std::io::Result<()> {
    for candidate in [path.to_path_buf(), wal_path(path), shm_path(path)] {
        if candidate.exists() {
            fs::remove_file(candidate)?;
        }
    }
    Ok(())
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
