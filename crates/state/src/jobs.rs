use std::collections::BTreeMap;

use buizz_core::market::{DataSource, Dataset, Symbol};
use buizz_core::ops::{FetchStatus, JobOutcome, JobRun, SourceFetch};
use buizz_core::time::Timestamp;
use chrono::TimeDelta;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::codec::{enum_text, int, read_enum, read_optional, read_timestamp, read_u64};
use crate::{ContractHistory, FetchUsage, Result, SymbolFetches};

const TRANSIENT_BACKOFF: TimeDelta = TimeDelta::hours(6);
const PERMANENT_BACKOFF: TimeDelta = TimeDelta::days(30);

const RESET: &str = "(SELECT coalesce(max(at), 0) FROM market_reset)";
const JOB_COLUMNS: &str = "id, job, started_at, finished_at, outcome, rows, message";
const FETCH_COLUMNS: &str = "source, dataset, symbol, requested_from, requested_to, at, status, \
    http_status, rows, retry_after, message";

fn job_run(row: &Row) -> rusqlite::Result<JobRun> {
    Ok(JobRun {
        id: row.get(0)?,
        job: row.get(1)?,
        started_at: read_timestamp(row, 2)?,
        finished_at: read_optional(row, 3, read_timestamp)?,
        outcome: read_optional(row, 4, read_enum)?,
        rows: read_u64(row, 5)?,
        message: row.get(6)?,
    })
}

fn source_fetch(row: &Row) -> rusqlite::Result<SourceFetch> {
    Ok(SourceFetch {
        source: read_enum(row, 0)?,
        dataset: read_enum(row, 1)?,
        symbol: row
            .get::<_, Option<String>>(2)?
            .map(|text| Symbol::new(&text)),
        requested_from: row.get(3)?,
        requested_to: row.get(4)?,
        at: read_timestamp(row, 5)?,
        status: read_enum(row, 6)?,
        http_status: row.get(7)?,
        rows: read_u64(row, 8)?,
        retry_after: read_optional(row, 9, read_timestamp)?,
        message: row.get(10)?,
    })
}

pub fn start_job(connection: &Connection, job: &str, at: Timestamp) -> Result<i64> {
    connection.execute(
        "INSERT INTO job_run (job, started_at) VALUES (?, ?)",
        params![job, at.nanos()],
    )?;
    Ok(connection.last_insert_rowid())
}

pub fn finish_job(
    connection: &Connection,
    job_run_id: i64,
    at: Timestamp,
    outcome: JobOutcome,
    rows: u64,
    message: Option<&str>,
) -> Result<()> {
    connection.execute(
        "UPDATE job_run SET finished_at = ?, outcome = ?, rows = ?, message = ? WHERE id = ?",
        params![
            at.nanos(),
            enum_text(&outcome)?,
            int(rows)?,
            message,
            job_run_id
        ],
    )?;
    Ok(())
}

pub fn latest_jobs(connection: &Connection) -> Result<Vec<JobRun>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT {JOB_COLUMNS} FROM job_run WHERE id IN \
         (SELECT MAX(id) FROM job_run WHERE started_at >= {RESET} GROUP BY job) ORDER BY job"
    ))?;
    let jobs = statement
        .query_map([], job_run)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(jobs)
}

pub fn last_job_success(connection: &Connection, job: &str) -> Result<Option<JobRun>> {
    Ok(connection
        .query_row(
            &format!(
                "SELECT {JOB_COLUMNS} FROM job_run \
                 WHERE job = ? AND outcome IN ('ok', 'partial', 'noop', 'waiting') \
                 AND started_at >= {RESET} ORDER BY id DESC LIMIT 1"
            ),
            [job],
            job_run,
        )
        .optional()?)
}

pub fn first_job_start(connection: &Connection, job: &str) -> Result<Option<Timestamp>> {
    let nanos: Option<i64> = connection.query_row(
        &format!("SELECT min(started_at) FROM job_run WHERE job = ? AND started_at >= {RESET}"),
        [job],
        |row| row.get(0),
    )?;
    Ok(nanos.map(Timestamp::from_nanos))
}

fn retry_after(fetch: &SourceFetch) -> Option<Timestamp> {
    fetch
        .retry_after
        .or_else(|| match (fetch.status, &fetch.symbol) {
            (FetchStatus::RateLimited, _) | (FetchStatus::Failed, Some(_)) => {
                Some(fetch.at + TRANSIENT_BACKOFF)
            }
            (FetchStatus::Rejected, Some(_)) => Some(fetch.at + PERMANENT_BACKOFF),
            _ => None,
        })
}

pub fn rate_limited_until(
    connection: &Connection,
    source: DataSource,
) -> Result<Option<Timestamp>> {
    let until: Option<i64> = connection.query_row(
        "SELECT MAX(retry_after) FROM source_fetch WHERE source = ? AND status = 'rate_limited'",
        [enum_text(&source)?],
        |row| row.get(0),
    )?;
    Ok(until.map(Timestamp::from_nanos))
}

pub fn failure_streaks(
    connection: &Connection,
    unfinished_before: Timestamp,
) -> Result<BTreeMap<String, u32>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT job, COUNT(*) FROM job_run failed \
         WHERE (outcome = 'failed' OR (outcome IS NULL AND started_at < ?)) \
         AND started_at >= {RESET} AND id > \
         COALESCE((SELECT MAX(id) FROM job_run other WHERE other.job = failed.job \
         AND other.outcome <> 'failed'), 0) GROUP BY job"
    ))?;
    let streaks = statement
        .query_map([unfinished_before.nanos()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(streaks)
}

pub fn reset_market(connection: &Connection, at: Timestamp) -> Result<bool> {
    let inserted = connection.execute(
        &format!(
            "INSERT INTO market_reset (at) SELECT ? WHERE \
             EXISTS (SELECT 1 FROM job_run WHERE started_at >= {RESET}) \
             OR EXISTS (SELECT 1 FROM source_fetch WHERE at >= {RESET})"
        ),
        [at.nanos()],
    )?;
    Ok(inserted > 0)
}

pub fn record_fetch(connection: &Connection, fetch: &SourceFetch) -> Result<()> {
    connection.execute(
        &format!(
            "INSERT INTO source_fetch ({FETCH_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ),
        params![
            enum_text(&fetch.source)?,
            enum_text(&fetch.dataset)?,
            fetch.symbol.as_ref().map(Symbol::as_str),
            fetch.requested_from,
            fetch.requested_to,
            fetch.at.nanos(),
            enum_text(&fetch.status)?,
            fetch.http_status,
            int(fetch.rows)?,
            retry_after(fetch).map(Timestamp::nanos),
            fetch.message,
        ],
    )?;
    Ok(())
}

pub fn fetch_usage(
    connection: &Connection,
    source: DataSource,
    since: Timestamp,
) -> Result<FetchUsage> {
    let usage = connection.query_row(
        "SELECT COUNT(*), COUNT(DISTINCT symbol) FROM source_fetch WHERE source = ? AND at >= ?",
        params![enum_text(&source)?, since.nanos()],
        |row| {
            Ok(FetchUsage {
                requests: read_u64(row, 0)?,
                symbols: read_u64(row, 1)?,
            })
        },
    )?;
    Ok(usage)
}

pub fn recent_fetches(
    connection: &Connection,
    source: DataSource,
    dataset: Dataset,
    symbol: Option<&Symbol>,
    limit: u32,
) -> Result<Vec<SourceFetch>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT {FETCH_COLUMNS} FROM source_fetch WHERE source = ? AND dataset = ? \
         AND symbol IS ? AND NOT (status IN ('ok', 'empty') AND at < {RESET}) \
         ORDER BY at DESC, id DESC LIMIT ?"
    ))?;
    let fetches = statement
        .query_map(
            params![
                enum_text(&source)?,
                enum_text(&dataset)?,
                symbol.map(Symbol::as_str),
                limit
            ],
            source_fetch,
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(fetches)
}

pub fn symbol_fetches(
    connection: &Connection,
    source: DataSource,
    dataset: Dataset,
) -> Result<BTreeMap<Symbol, SymbolFetches>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT f.symbol, max(f.status IN ('ok', 'empty') AND f.at >= {RESET}), \
         (SELECT l.status FROM source_fetch l WHERE l.source = f.source AND l.dataset = f.dataset \
         AND l.symbol = f.symbol ORDER BY l.at DESC, l.id DESC LIMIT 1) \
         FROM source_fetch f WHERE f.source = ? AND f.dataset = ? AND f.symbol IS NOT NULL \
         GROUP BY f.symbol"
    ))?;
    let symbols = statement
        .query_map(params![enum_text(&source)?, enum_text(&dataset)?], |row| {
            Ok((
                Symbol::new(&row.get::<_, String>(0)?),
                SymbolFetches {
                    read: row.get(1)?,
                    latest: read_enum(row, 2)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(symbols)
}

pub fn track_contracts(
    connection: &Connection,
    at: Timestamp,
    window: TimeDelta,
    done: &[(Dataset, DataSource, u64)],
) -> Result<BTreeMap<(Dataset, DataSource), ContractHistory>> {
    let horizon = (at - window).nanos();
    let mut histories = BTreeMap::new();
    for (dataset, source, count) in done {
        let (dataset_text, source_text) = (enum_text(dataset)?, enum_text(source)?);
        let latest: Option<u64> = connection
            .query_row(
                "SELECT done FROM contract_progress WHERE dataset = ? AND source = ? \
                 ORDER BY at DESC LIMIT 1",
                params![dataset_text, source_text],
                |row| read_u64(row, 0),
            )
            .optional()?;
        if latest != Some(*count) {
            connection.execute(
                "INSERT OR REPLACE INTO contract_progress (dataset, source, at, done) \
                 VALUES (?, ?, ?, ?)",
                params![dataset_text, source_text, at.nanos(), int(*count)?],
            )?;
        }
        connection.execute(
            "DELETE FROM contract_progress WHERE dataset = ?1 AND source = ?2 AND at < \
             (SELECT max(at) FROM contract_progress WHERE dataset = ?1 AND source = ?2 AND at <= ?3)",
            params![dataset_text, source_text, horizon],
        )?;
        let history = connection.query_row(
            "SELECT min(at), (SELECT done FROM contract_progress WHERE dataset = ?1 AND source = ?2 \
             ORDER BY at LIMIT 1), max(at) FROM contract_progress WHERE dataset = ?1 AND source = ?2",
            params![dataset_text, source_text],
            |row| {
                Ok(ContractHistory {
                    base_at: read_timestamp(row, 0)?,
                    base_done: read_u64(row, 1)?,
                    moved_at: read_timestamp(row, 2)?,
                })
            },
        )?;
        histories.insert((*dataset, *source), history);
    }
    Ok(histories)
}

pub fn latest_rejections(
    connection: &Connection,
    source: DataSource,
    dataset: Dataset,
) -> Result<Vec<SourceFetch>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT {FETCH_COLUMNS} FROM source_fetch latest \
         WHERE source = ?1 AND dataset = ?2 AND status = 'rejected' AND id = \
         (SELECT MAX(id) FROM source_fetch other WHERE other.source = ?1 \
         AND other.dataset = ?2 AND other.symbol = latest.symbol) \
         ORDER BY symbol"
    ))?;
    let fetches = statement
        .query_map(
            params![enum_text(&source)?, enum_text(&dataset)?],
            source_fetch,
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(fetches)
}
