use std::{env, time::Instant};

use rusqlite::{Connection, params};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = env::args().collect();
    if arguments.len() != 3 {
        return Err("usage: keyset_paging_poc DATABASE PAGE_SIZE".into());
    }
    let connection = Connection::open(&arguments[1])?;
    let page_size: usize = arguments[2].parse()?;
    if page_size == 0 {
        return Err("page size must be greater than zero".into());
    }

    let started = Instant::now();
    let mut cursor: Option<(i64, i64)> = None;
    let mut previous: Option<(i64, i64)> = None;
    let mut rows_seen = 0_usize;
    let mut pages = 0_usize;
    let mut max_page_rows = 0_usize;
    let mut page_latencies_us = Vec::new();

    loop {
        let page_started = Instant::now();
        let mut rows = if let Some((last_used_at, id)) = cursor {
            let mut statement = connection.prepare_cached(
                "SELECT last_used_at, id FROM clips
                 WHERE last_used_at < ?1 OR (last_used_at = ?1 AND id < ?2)
                 ORDER BY last_used_at DESC, id DESC
                 LIMIT ?3",
            )?;
            statement
                .query_map(params![last_used_at, id, page_size as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let mut statement = connection.prepare_cached(
                "SELECT last_used_at, id FROM clips
                 ORDER BY last_used_at DESC, id DESC
                 LIMIT ?1",
            )?;
            statement
                .query_map([page_size as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        page_latencies_us.push(page_started.elapsed().as_micros() as u64);
        if rows.is_empty() {
            break;
        }
        pages += 1;
        max_page_rows = max_page_rows.max(rows.len());
        for current in &rows {
            if let Some(previous) = previous
                && previous <= *current
            {
                return Err("page order is not strictly descending".into());
            }
            previous = Some(*current);
        }
        rows_seen += rows.len();
        cursor = rows.pop();
    }

    page_latencies_us.sort_unstable();
    let p95_index = (page_latencies_us.len().saturating_sub(1) * 95) / 100;
    println!("rows_seen={rows_seen}");
    println!("pages={pages}");
    println!("max_page_rows={max_page_rows}");
    println!("page_p95_us={}", page_latencies_us[p95_index]);
    println!("elapsed_ms={:.3}", started.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}
