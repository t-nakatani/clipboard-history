use std::{env, fs, path::Path};

use clipboard_store::{configure_connection, migrate};
use rusqlite::{Connection, params};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = env::args().collect();
    if arguments.len() < 3 {
        return Err("usage: image_preview_storage_poc PREVIEW_DIRECTORY ROW_COUNT".into());
    }
    let preview_directory = Path::new(&arguments[1]);
    let row_count: usize = arguments[2].parse()?;
    let mut previews = fs::read_dir(preview_directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("-preview."))
        })
        .map(fs::read)
        .collect::<Result<Vec<_>, _>>()?;
    previews.sort_by_key(Vec::len);
    if previews.is_empty() {
        return Err("preview directory does not contain generated preview files".into());
    }

    let temporary = env::temp_dir().join(format!(
        "clipboard-image-preview-storage-{}-{}.sqlite",
        std::process::id(),
        row_count
    ));
    let mut connection = Connection::open(&temporary)?;
    configure_connection(&connection, 1024)?;
    migrate(&mut connection)?;

    {
        let transaction = connection.transaction()?;
        let mut clip = transaction.prepare_cached(
            "INSERT INTO clips(
                id, content_hash, content_kind, first_copied_at, last_used_at,
                pinned, copy_count, payload_size, normalized_text
             ) VALUES (?1, ?2, 1, ?1, ?1, 0, 1, 0, NULL)",
        )?;
        for index in 0..row_count {
            let id = i64::try_from(index + 1)?;
            let mut hash = [0_u8; 32];
            hash[..8].copy_from_slice(&id.to_le_bytes());
            clip.execute(params![id, hash])?;
        }
        drop(clip);
        transaction.commit()?;
    }
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let page_size: i64 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let baseline_page_count: i64 =
        connection.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let baseline_database_bytes = u64::try_from(page_size)? * u64::try_from(baseline_page_count)?;

    let logical_preview_bytes: u64;
    {
        let transaction = connection.transaction()?;
        let mut preview = transaction.prepare_cached(
            "INSERT INTO clip_previews(clip_id, uti, data) VALUES (?1, 'public.jpeg', ?2)",
        )?;
        let mut total = 0_u64;
        for index in 0..row_count {
            let id = i64::try_from(index + 1)?;
            let bytes = &previews[index % previews.len()];
            preview.execute(params![id, bytes])?;
            total += u64::try_from(bytes.len())?;
        }
        drop(preview);
        transaction.commit()?;
        logical_preview_bytes = total;
    }
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let page_size: i64 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let page_count: i64 = connection.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let database_bytes = u64::try_from(page_size)? * u64::try_from(page_count)?;
    let preview_database_bytes = database_bytes - baseline_database_bytes;

    println!("rows={row_count}");
    println!("fixture_count={}", previews.len());
    println!("logical_preview_bytes={logical_preview_bytes}");
    println!("baseline_database_bytes={baseline_database_bytes}");
    println!("preview_database_bytes={preview_database_bytes}");
    println!("database_bytes={database_bytes}");
    println!(
        "database_bytes_per_clip={:.2}",
        database_bytes as f64 / row_count as f64
    );
    println!(
        "preview_database_bytes_per_clip={:.2}",
        preview_database_bytes as f64 / row_count as f64
    );
    println!(
        "preview_storage_amplification={:.4}",
        preview_database_bytes as f64 / logical_preview_bytes as f64
    );

    drop(connection);
    fs::remove_file(temporary)?;
    Ok(())
}
