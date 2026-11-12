use std::ops::RangeInclusive;

use buizz_core::api::{BotFootprint, BotUsage, RunSummary, SparkPoint};
use buizz_core::ledger::{LedgerEntry, LedgerKind, Portfolio};
use buizz_core::market::Symbol;
use buizz_core::order::{Fill, Order, OrderId};
use buizz_core::run::{
    DesiredState, EquityPoint, Reproducibility, RunEvent, RunEventKind, RunEventType, RunId,
    RunStatus,
};
use buizz_core::time::Timestamp;
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::Serialize;

use crate::codec::{
    enum_text, int, json_text, read_decimal, read_enum, read_json, read_optional, read_timestamp,
    read_u64,
};
use crate::{Checkpoint, NewRun, Result, RunFilter, RunOutcome, StateError, StepCommit};

const LIVE_RUN_CONFLICT: &str = "UNIQUE constraint failed: run.bot";

const RUN_COLUMNS: &str = "id, bot, mode, status, desired, config, created_at, started_at, \
    finished_at, clock, equity, metrics, result_hash, badges, bot_hash, engine_version, seed, \
    data_watermark, message, bot_processes, bot_seconds, bot_peak_memory";

fn tag_of<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value)?.get("type") {
        Some(serde_json::Value::String(tag)) => Ok(tag.clone()),
        _ => Err(StateError::Encoding(
            <serde_json::Error as serde::ser::Error>::custom("missing type tag"),
        )),
    }
}

fn run_summary(row: &Row) -> rusqlite::Result<RunSummary> {
    Ok(RunSummary {
        id: RunId(row.get(0)?),
        bot: row.get(1)?,
        mode: read_enum(row, 2)?,
        status: read_enum(row, 3)?,
        desired: read_enum(row, 4)?,
        config: read_json(row, 5)?,
        created_at: read_timestamp(row, 6)?,
        started_at: read_optional(row, 7, read_timestamp)?,
        finished_at: read_optional(row, 8, read_timestamp)?,
        clock: read_optional(row, 9, read_timestamp)?,
        equity: read_optional(row, 10, read_decimal)?,
        metrics: read_optional(row, 11, read_json)?,
        result_hash: row.get(12)?,
        badges: read_json(row, 13)?,
        reproducibility: Some(Reproducibility {
            bot_hash: row.get(14)?,
            engine_version: row.get(15)?,
            seed: read_u64(row, 16)?,
            data_watermark: read_optional(row, 17, read_timestamp)?,
        }),
        message: row.get(18)?,
        usage: BotUsage {
            processes: row.get(19)?,
            seconds: row.get(20)?,
            peak_memory: read_u64(row, 21)?,
        },
    })
}

pub fn create(connection: &Connection, run: &NewRun) -> Result<RunId> {
    let inserted = connection.execute(
        "INSERT INTO run (bot, mode, status, desired, config, holdout, bot_hash, engine_version, \
         seed, data_watermark, created_at, badges) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            run.config.bot,
            enum_text(&run.config.mode)?,
            enum_text(&run.status)?,
            enum_text(&run.desired)?,
            json_text(&run.config)?,
            run.config.holdout,
            run.bot_hash,
            run.engine_version,
            int(run.config.seed)?,
            run.data_watermark.map(Timestamp::nanos),
            run.created_at.nanos(),
            json_text(&run.badges)?,
        ],
    );
    match inserted {
        Ok(_) => Ok(RunId(connection.last_insert_rowid())),
        Err(rusqlite::Error::SqliteFailure(_, Some(message))) if message == LIVE_RUN_CONFLICT => {
            Err(StateError::LiveRunExists(run.config.bot.clone()))
        }
        Err(error) => Err(error.into()),
    }
}

pub fn get(connection: &Connection, run_id: RunId) -> Result<Option<RunSummary>> {
    Ok(connection
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM run WHERE id = ?"),
            [run_id.0],
            run_summary,
        )
        .optional()?)
}

pub fn list(connection: &Connection, filter: &RunFilter) -> Result<Vec<RunSummary>> {
    let mut conditions = Vec::new();
    let mut values: Vec<Sql> = Vec::new();
    if let Some(bot) = &filter.bot {
        conditions.push("bot = ?".to_owned());
        values.push(bot.clone().into());
    }
    if let Some(mode) = filter.mode {
        conditions.push("mode = ?".to_owned());
        values.push(enum_text(&mode)?.into());
    }
    if let Some(desired) = filter.desired {
        conditions.push("desired = ?".to_owned());
        values.push(enum_text(&desired)?.into());
    }
    if let Some(before) = filter.before {
        conditions.push("id < ?".to_owned());
        values.push(before.0.into());
    }
    if !filter.statuses.is_empty() {
        let marks = vec!["?"; filter.statuses.len()].join(", ");
        conditions.push(format!("status IN ({marks})"));
        for status in &filter.statuses {
            values.push(enum_text(status)?.into());
        }
    }
    let predicate = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    values.push(filter.limit.map_or(-1, i64::from).into());
    let mut statement = connection.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM run {predicate} ORDER BY id DESC LIMIT ?"
    ))?;
    let runs = statement
        .query_map(params_from_iter(values), run_summary)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(runs)
}

fn require_changed(changed: usize, run_id: RunId) -> Result<()> {
    if changed == 0 {
        Err(StateError::UnknownRun(run_id))
    } else {
        Ok(())
    }
}

pub fn set_status(
    connection: &Connection,
    run_id: RunId,
    status: RunStatus,
    message: Option<&str>,
    at: Timestamp,
) -> Result<()> {
    let started = (status == RunStatus::Running).then_some(at.nanos());
    let finished = status.is_finished().then_some(at.nanos());
    let changed = connection.execute(
        "UPDATE run SET status = ?, message = ?, started_at = COALESCE(started_at, ?), \
         finished_at = COALESCE(?, finished_at) WHERE id = ?",
        params![enum_text(&status)?, message, started, finished, run_id.0],
    )?;
    require_changed(changed, run_id)
}

pub fn set_desired(connection: &Connection, run_id: RunId, desired: DesiredState) -> Result<()> {
    let changed = connection.execute(
        "UPDATE run SET desired = ? WHERE id = ?",
        params![enum_text(&desired)?, run_id.0],
    )?;
    require_changed(changed, run_id)
}

pub fn add_usage(
    connection: &Connection,
    run_id: RunId,
    seconds: f64,
    peak_memory: u64,
) -> Result<BotUsage> {
    Ok(connection.query_row(
        "UPDATE run SET bot_processes = bot_processes + 1, bot_seconds = bot_seconds + ?, \
         bot_peak_memory = MAX(bot_peak_memory, ?) WHERE id = ? \
         RETURNING bot_processes, bot_seconds, bot_peak_memory",
        params![seconds, int(peak_memory)?, run_id.0],
        |row| {
            Ok(BotUsage {
                processes: row.get(0)?,
                seconds: row.get(1)?,
                peak_memory: read_u64(row, 2)?,
            })
        },
    )?)
}

pub fn footprints(connection: &Connection) -> Result<Vec<BotFootprint>> {
    let mut statement = connection.prepare(
        "SELECT bot, mode, COUNT(*), SUM(bot_processes), SUM(bot_seconds), MAX(bot_peak_memory) \
         FROM run WHERE bot_processes > 0 GROUP BY bot, mode \
         ORDER BY MAX(bot_peak_memory) DESC, bot, mode",
    )?;
    let footprints = statement
        .query_map([], |row| {
            Ok(BotFootprint {
                bot: row.get(0)?,
                mode: read_enum(row, 1)?,
                runs: row.get(2)?,
                usage: BotUsage {
                    processes: row.get(3)?,
                    seconds: row.get(4)?,
                    peak_memory: read_u64(row, 5)?,
                },
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(footprints)
}

pub fn finish(connection: &Connection, run_id: RunId, outcome: &RunOutcome) -> Result<()> {
    let metrics = outcome.metrics.as_ref().map(json_text).transpose()?;
    let badge = outcome.badge.as_ref().map(enum_text).transpose()?;
    let changed = connection.execute(
        "UPDATE run SET status = ?1, finished_at = ?2, result_hash = ?3, metrics = ?4, message = ?5, \
         badges = CASE WHEN ?6 IS NULL THEN badges ELSE json_insert(badges, '$[#]', ?6) END \
         WHERE id = ?7",
        params![
            enum_text(&outcome.status)?,
            outcome.finished_at.nanos(),
            outcome.result_hash,
            metrics,
            outcome.message,
            badge,
            run_id.0,
        ],
    )?;
    require_changed(changed, run_id)
}

fn insert_ledger(connection: &Connection, run_id: RunId, entries: &[LedgerEntry]) -> Result<()> {
    let mut statement = connection.prepare_cached(
        "INSERT INTO ledger_entry (run_id, seq, at, kind, symbol, cash_delta, body) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )?;
    for entry in entries {
        let symbol = entry.kind.symbol().map(|symbol| symbol.as_str());
        statement.execute(params![
            run_id.0,
            int(entry.seq)?,
            entry.at.nanos(),
            tag_of(&entry.kind)?,
            symbol,
            entry.kind.cash_delta().to_string(),
            json_text(&entry.kind)?,
        ])?;
    }
    Ok(())
}

fn upsert_orders(connection: &Connection, run_id: RunId, orders: &[Order]) -> Result<()> {
    let mut statement = connection.prepare_cached(
        "INSERT INTO run_order (run_id, order_id, client_id, symbol, status, decided_at, \
         updated_at, body) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (run_id, order_id) DO UPDATE SET status = excluded.status, \
         updated_at = excluded.updated_at, body = excluded.body",
    )?;
    for order in orders {
        statement.execute(params![
            run_id.0,
            int(order.id.0)?,
            order.client_id,
            order.symbol.as_str(),
            enum_text(&order.status)?,
            order.decided_at.nanos(),
            order.updated_at.nanos(),
            json_text(order)?,
        ])?;
    }
    Ok(())
}

fn insert_equity(connection: &Connection, run_id: RunId, points: &[EquityPoint]) -> Result<()> {
    let mut statement = connection.prepare_cached(
        "INSERT INTO equity_point (run_id, at, equity, cash, gross_exposure) \
         VALUES (?, ?, ?, ?, ?)",
    )?;
    for point in points {
        statement.execute(params![
            run_id.0,
            point.at.nanos(),
            point.equity.to_string(),
            point.cash.to_string(),
            point.gross_exposure.to_string(),
        ])?;
    }
    Ok(())
}

pub fn insert_events(connection: &Connection, run_id: RunId, events: &[RunEvent]) -> Result<()> {
    let mut statement = connection.prepare_cached(
        "INSERT INTO run_event (run_id, seq, at, clock, kind, body) VALUES (?, ?, ?, ?, ?, ?)",
    )?;
    for event in events {
        statement.execute(params![
            run_id.0,
            int(event.seq)?,
            event.at.nanos(),
            event.clock.map(Timestamp::nanos),
            enum_text(&event.kind.event_type())?,
            json_text(&event.kind)?,
        ])?;
    }
    Ok(())
}

pub fn commit_step(connection: &Connection, commit: &StepCommit) -> Result<()> {
    let last: Option<i64> = connection
        .query_row(
            "SELECT step_seq FROM run WHERE id = ?",
            [commit.run_id.0],
            |row| row.get(0),
        )
        .optional()?;
    let last = last.ok_or(StateError::UnknownRun(commit.run_id))?;
    if int(commit.step_seq)? != last + 1 {
        return Err(StateError::StaleStep {
            run_id: commit.run_id,
            step_seq: commit.step_seq,
        });
    }
    insert_ledger(connection, commit.run_id, &commit.ledger)?;
    upsert_orders(connection, commit.run_id, &commit.orders)?;
    insert_equity(connection, commit.run_id, &commit.equity_points)?;
    insert_events(connection, commit.run_id, &commit.events)?;
    connection.execute(
        "UPDATE run SET step_seq = ?, clock = ?, equity = ?, checkpoint = ?, \
         bot_state = COALESCE(?, bot_state), undelivered = ? WHERE id = ?",
        params![
            int(commit.step_seq)?,
            commit.clock.nanos(),
            commit.equity.to_string(),
            commit.engine_checkpoint,
            commit.bot_state,
            commit.undelivered,
            commit.run_id.0,
        ],
    )?;
    Ok(())
}

pub fn checkpoint(connection: &Connection, run_id: RunId) -> Result<Option<Checkpoint>> {
    let found = connection
        .query_row(
            "SELECT step_seq, clock, checkpoint, bot_state, undelivered FROM run \
             WHERE id = ? AND checkpoint IS NOT NULL",
            [run_id.0],
            |row| {
                Ok(Checkpoint {
                    step_seq: read_u64(row, 0)?,
                    clock: read_timestamp(row, 1)?,
                    engine_checkpoint: row.get(2)?,
                    bot_state: row.get(3)?,
                    undelivered: row.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(found)
}

pub fn ledger(connection: &Connection, run_id: RunId) -> Result<Vec<LedgerEntry>> {
    let mut statement = connection
        .prepare_cached("SELECT seq, at, body FROM ledger_entry WHERE run_id = ? ORDER BY seq")?;
    let entries = statement
        .query_map([run_id.0], |row| {
            Ok(LedgerEntry {
                seq: read_u64(row, 0)?,
                at: read_timestamp(row, 1)?,
                kind: read_json(row, 2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(entries)
}

pub fn portfolio(connection: &Connection, run_id: RunId) -> Result<Portfolio> {
    Ok(Portfolio::fold(&ledger(connection, run_id)?))
}

pub fn orders(
    connection: &Connection,
    run_id: RunId,
    after: Option<u64>,
    limit: u32,
) -> Result<Vec<Order>> {
    let after = after.map(int).transpose()?.unwrap_or(-1);
    let mut statement = connection.prepare_cached(
        "SELECT body FROM run_order WHERE run_id = ? AND order_id > ? ORDER BY order_id LIMIT ?",
    )?;
    let orders = statement
        .query_map(params![run_id.0, after, limit], |row| read_json(row, 0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(orders)
}

fn reasoned_fills(
    connection: &Connection,
    condition: &str,
    values: Vec<Sql>,
) -> Result<Vec<(Fill, String)>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT ledger_entry.at, ledger_entry.body, run_order.client_id, \
         json_extract(run_order.body, '$.reason') FROM ledger_entry \
         JOIN run_order ON run_order.run_id = ledger_entry.run_id \
         AND run_order.order_id = json_extract(ledger_entry.body, '$.order_id') \
         WHERE ledger_entry.run_id = ? AND ledger_entry.kind = 'fill' AND {condition} \
         ORDER BY ledger_entry.seq"
    ))?;
    let fills = statement
        .query_map(params_from_iter(values), |row| {
            let at = read_timestamp(row, 0)?;
            let kind: LedgerKind = read_json(row, 1)?;
            let client_id: String = row.get(2)?;
            let reason: String = row.get(3)?;
            Ok((at, kind, client_id, reason))
        })?
        .filter_map(|found| match found {
            Ok((
                at,
                LedgerKind::Fill {
                    order_id,
                    symbol,
                    qty,
                    price,
                    commission,
                    feed,
                },
                client_id,
                reason,
            )) => Some(Ok((
                Fill {
                    order_id,
                    client_id,
                    symbol,
                    at,
                    qty,
                    price,
                    commission,
                    feed,
                },
                reason,
            ))),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<rusqlite::Result<_>>()?;
    Ok(fills)
}

pub fn fills(
    connection: &Connection,
    run_id: RunId,
    orders: RangeInclusive<OrderId>,
) -> Result<Vec<Fill>> {
    let fills = reasoned_fills(
        connection,
        "json_extract(ledger_entry.body, '$.order_id') BETWEEN ? AND ?",
        vec![
            run_id.0.into(),
            int(orders.start().0)?.into(),
            int(orders.end().0)?.into(),
        ],
    )?;
    Ok(fills.into_iter().map(|(fill, _)| fill).collect())
}

pub fn symbol_fills(
    connection: &Connection,
    run_id: RunId,
    symbol: &Symbol,
) -> Result<Vec<(Fill, String)>> {
    reasoned_fills(
        connection,
        "ledger_entry.symbol = ?",
        vec![run_id.0.into(), symbol.as_str().to_owned().into()],
    )
}

pub fn equity(connection: &Connection, run_id: RunId) -> Result<Vec<EquityPoint>> {
    let mut statement = connection.prepare_cached(
        "SELECT at, equity, cash, gross_exposure FROM equity_point WHERE run_id = ? ORDER BY at",
    )?;
    let points = statement
        .query_map([run_id.0], |row| {
            Ok(EquityPoint {
                at: read_timestamp(row, 0)?,
                equity: read_decimal(row, 1)?,
                cash: read_decimal(row, 2)?,
                gross_exposure: read_decimal(row, 3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(points)
}

pub fn sparkline(connection: &Connection, run_id: RunId, points: u32) -> Result<Vec<SparkPoint>> {
    let mut statement = connection.prepare_cached(
        "SELECT at, equity FROM (SELECT at, equity, \
         ROW_NUMBER() OVER (ORDER BY at) - 1 AS position, COUNT(*) OVER () AS total \
         FROM equity_point WHERE run_id = ?1) \
         WHERE position % MAX((total + ?2 - 1) / ?2, 1) = 0 OR position = total - 1 \
         ORDER BY at",
    )?;
    let points = statement
        .query_map(params![run_id.0, points.max(1)], |row| {
            Ok(SparkPoint {
                at: read_timestamp(row, 0)?,
                equity: read_decimal(row, 1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(points)
}

pub fn events(
    connection: &Connection,
    run_id: RunId,
    after: Option<u64>,
    limit: u32,
    types: &[RunEventType],
) -> Result<Vec<RunEvent>> {
    let mut values: Vec<Sql> = vec![
        run_id.0.into(),
        after.map(int).transpose()?.unwrap_or(-1).into(),
    ];
    let mut predicate = String::new();
    if !types.is_empty() {
        predicate = format!(" AND kind IN ({})", vec!["?"; types.len()].join(", "));
        for event_type in types {
            values.push(enum_text(event_type)?.into());
        }
    }
    values.push(i64::from(limit).into());
    let mut statement = connection.prepare_cached(&format!(
        "SELECT seq, at, clock, body FROM run_event WHERE run_id = ? AND seq > ?{predicate} \
         ORDER BY seq LIMIT ?"
    ))?;
    let events = statement
        .query_map(params_from_iter(values), |row| {
            Ok(RunEvent {
                seq: read_u64(row, 0)?,
                at: read_timestamp(row, 1)?,
                clock: read_optional(row, 2, read_timestamp)?,
                kind: read_json::<RunEventKind>(row, 3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(events)
}

pub fn holdout_runs(connection: &Connection, bot: &str) -> Result<u32> {
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM run WHERE bot = ? AND holdout = 1",
        [bot],
        |row| row.get(0),
    )?)
}
