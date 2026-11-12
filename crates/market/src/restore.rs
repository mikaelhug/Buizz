use std::path::{Path, PathBuf};

use buizz_core::market::Dataset;
use buizz_core::time::Timestamp;
use duckdb::{Connection, params};

use crate::codec::{Ts, wire_name};
use crate::{APPENDED, MarketError, REPLACED, Result};

const BATCHES: &str = "ingest_batch.parquet";
const INDEX: &str = "index_history.parquet";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Restored {
    pub parts: usize,
    pub batches: i64,
    pub rows: u64,
}

pub(crate) fn parts(backup: &Path) -> Result<Vec<PathBuf>> {
    if backup.join(BATCHES).exists() {
        return Ok(vec![backup.to_owned()]);
    }
    let mut parts = Vec::new();
    for entry in std::fs::read_dir(backup)? {
        let path = entry?.path();
        if path.join(BATCHES).exists() && path.extension().is_none_or(|ext| ext != "partial") {
            parts.push(path);
        }
    }
    parts.sort();
    if parts.is_empty() {
        return Err(MarketError::Restore(format!(
            "{} holds no market backup",
            backup.display()
        )));
    }
    Ok(parts)
}

fn load(connection: &Connection, table: &str, file: &Path) -> Result<u64> {
    let path = file.to_string_lossy().replace('\'', "''");
    Ok(connection.execute(
        &format!("INSERT INTO {table} BY NAME SELECT * FROM read_parquet('{path}')"),
        [],
    )? as u64)
}

pub(crate) fn restore(connection: &mut Connection, parts: &[PathBuf]) -> Result<Restored> {
    let transaction = connection.transaction()?;
    let existing: i64 =
        transaction.query_row("SELECT count(*) FROM ingest_batch", [], |row| row.get(0))?;
    if existing > 0 {
        return Err(MarketError::Restore(format!(
            "the market store already holds {existing} ingest batches; restore into an empty \
             store before ingest starts"
        )));
    }
    let mut rows = 0;
    for part in parts {
        for (table, _) in APPENDED {
            let file = part.join(format!("{table}.parquet"));
            if file.exists() {
                rows += load(&transaction, table, &file)?;
            }
        }
    }
    if let Some(latest) = parts.iter().rev().find(|part| part.join(INDEX).exists()) {
        for table in REPLACED {
            transaction.execute(&format!("DELETE FROM {table}"), [])?;
            rows += load(
                &transaction,
                table,
                &latest.join(format!("{table}.parquet")),
            )?;
        }
    }
    let batches: i64 =
        transaction.query_row("SELECT coalesce(max(id), 0) FROM ingest_batch", [], |row| {
            row.get(0)
        })?;
    let next: i64 =
        transaction.query_row("SELECT nextval('ingest_batch_id')", [], |row| row.get(0))?;
    if next < batches {
        transaction.query_row(
            "SELECT max(nextval('ingest_batch_id')) FROM range(?)",
            [batches - next],
            |row| row.get::<_, i64>(0),
        )?;
    }
    cover_borrow(&transaction)?;
    transaction.commit()?;
    Ok(Restored {
        parts: parts.len(),
        batches,
        rows,
    })
}

fn cover_borrow(connection: &Connection) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT b.source, min(a.recorded_at), max(a.recorded_at) FROM borrow_availability a \
         JOIN ingest_batch b ON b.id = a.batch_id GROUP BY b.source",
    )?;
    let ranges = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Ts>(1)?.0,
                row.get::<_, Ts>(2)?.0,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let dataset = wire_name(&Dataset::Borrow);
    for (source, first, last) in ranges {
        connection.execute(
            "INSERT OR REPLACE INTO coverage VALUES (?, '', ?, ?, ?, ?)",
            params![
                dataset,
                source,
                first.exchange_date(),
                last.exchange_date(),
                Ts(Timestamp::now())
            ],
        )?;
    }
    Ok(())
}
