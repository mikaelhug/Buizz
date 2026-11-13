use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;

use buizz_core::market::{
    Bar, BorrowAvailability, Company, CorporateAction, CorporateActionKind, DataSource, Dataset,
    DocumentSections, EarningsInsight, EarningsRelease, EstimateFact, Filing, FilingDocument,
    FilingText, FomcMeeting, FundamentalFact, IndexHistory, InsiderFiling, InsiderPlan, Instrument,
    MacroObservation, NewsArticle, PressExhibit, RiskFreeRate, SecFiling, Section, Session,
    ShortInterestRecord, ShortVolumeRecord, Symbol, split_rates,
};
use buizz_core::ops::ActionDisagreement;
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use duckdb::{Appender, Connection, Transaction, params};

use crate::codec::{ActionColumns, Dec, Ts, feed_name, wire_name};
use crate::text::TextVolumes;
use crate::{ActionWindow, MarketError, Result, WriteSummary};

const STAGED: [&str; 12] = [
    "instrument",
    "calendar",
    "bar_1d",
    "corporate_action",
    "corporate_action_check",
    "fundamental_fact",
    "rate_daily",
    "macro_observation",
    "fomc_meeting",
    "company",
    "earnings_release",
    "earnings_insight_fact",
];

pub(crate) fn prepare(connection: &Connection) -> Result<()> {
    for table in STAGED {
        connection.execute_batch(&format!(
            "CREATE TEMP TABLE stage_{table} AS SELECT * FROM main.{table} LIMIT 0"
        ))?;
    }
    connection.execute_batch(
        "CREATE TEMP TABLE bar_span (symbol VARCHAR, lo TIMESTAMP, hi TIMESTAMP); \
         CREATE TEMP TABLE action_scope (symbol VARCHAR); \
         CREATE TEMP TABLE shadow_span (ticker VARCHAR, lo DATE, hi DATE, listing VARCHAR); \
         CREATE TEMP TABLE report_scope (report VARCHAR); \
         CREATE TEMP TABLE coverage_stage AS SELECT * FROM main.coverage LIMIT 0; \
         CREATE TEMP TABLE stage_text_blob (doc_id VARCHAR, content_hash VARCHAR, text VARCHAR, raw BLOB)",
    )?;
    Ok(())
}

type Coverage = BTreeMap<Option<Symbol>, (NaiveDate, NaiveDate)>;

fn cover(coverage: &mut Coverage, symbol: Option<&Symbol>, date: NaiveDate) {
    coverage
        .entry(symbol.cloned())
        .and_modify(|(first, last)| {
            *first = (*first).min(date);
            *last = (*last).max(date);
        })
        .or_insert((date, date));
}

fn write_batch(
    connection: &mut Connection,
    source: DataSource,
    dataset: Dataset,
    coverage: Coverage,
    write: impl FnOnce(&Transaction<'_>, i64) -> Result<u64>,
) -> Result<WriteSummary> {
    let transaction = connection.transaction()?;
    let batch_id: i64 =
        transaction.query_row("SELECT nextval('ingest_batch_id')", [], |row| row.get(0))?;
    let rows = write(&transaction, batch_id)?;
    let source = wire_name(&source);
    let dataset = wire_name(&dataset);
    let now = Ts(Timestamp::now());
    transaction.execute(
        "INSERT INTO ingest_batch (id, source, dataset, created_at, rows) VALUES (?, ?, ?, ?, ?)",
        params![batch_id, source, dataset, now, rows],
    )?;
    transaction.execute("DELETE FROM coverage_stage", [])?;
    let mut appender = transaction.appender_to_catalog_and_db("coverage_stage", "temp", "main")?;
    for (symbol, (first, last)) in coverage {
        let symbol = symbol.as_ref().map_or("", Symbol::as_str);
        appender.append_row(params![dataset, symbol, source, first, last, now])?;
    }
    appender.flush()?;
    drop(appender);
    transaction.execute(
        "INSERT INTO coverage SELECT * FROM coverage_stage ON CONFLICT (dataset, symbol, source) \
         DO UPDATE SET first = least(coverage.first, excluded.first), \
         last = greatest(coverage.last, excluded.last), updated_at = excluded.updated_at",
        [],
    )?;
    transaction.commit()?;
    Ok(WriteSummary { batch_id, rows })
}

fn stage<T>(
    transaction: &Transaction<'_>,
    table: &str,
    rows: impl IntoIterator<Item = T>,
    mut append: impl FnMut(&mut Appender<'_>, T) -> duckdb::Result<()>,
) -> Result<u64> {
    transaction.execute(&format!("DELETE FROM stage_{table}"), [])?;
    let mut appender =
        transaction.appender_to_catalog_and_db(&format!("stage_{table}"), "temp", "main")?;
    let mut count = 0;
    for row in rows {
        append(&mut appender, row)?;
        count += 1;
    }
    appender.flush()?;
    Ok(count)
}

const KEYS: [(&str, &[&str]); 11] = [
    ("instrument", &["symbol"]),
    ("calendar", &["date"]),
    ("bar_1d", &["symbol", "date"]),
    (
        "corporate_action",
        &["symbol", "ex_date", "kind", "other_symbol"],
    ),
    ("corporate_action_check", &["symbol", "ex_date", "action"]),
    ("rate_daily", &["date"]),
    ("macro_observation", &["series", "date", "published"]),
    ("fomc_meeting", &["date", "scheduled"]),
    ("company", &["symbol"]),
    ("earnings_release", &["symbol", "accession"]),
    (
        "earnings_insight_fact",
        &["report", "metric", "scope", "period"],
    ),
];

fn replace_staged(transaction: &Transaction<'_>, table: &str, batch_id: i64) -> Result<u64> {
    let keys = KEYS
        .iter()
        .find_map(|(name, keys)| (*name == table).then_some(*keys))
        .expect("every staged table has its keys");
    let mut statement = transaction.prepare_cached(
        "SELECT column_name FROM duckdb_columns() WHERE schema_name = 'main' \
         AND table_name = ? AND column_name <> 'batch_id' ORDER BY column_index",
    )?;
    let columns = statement
        .query_map(params![table], |row| row.get::<_, String>(0))?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let same_key = keys
        .iter()
        .map(|key| format!("m.{key} = s.{key}"))
        .collect::<Vec<_>>()
        .join(" AND ");
    let differs = columns
        .iter()
        .map(|column| format!("m.{column} IS DISTINCT FROM s.{column}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    transaction.execute(
        &format!(
            "DELETE FROM stage_{table} s WHERE EXISTS (SELECT 1 FROM main.{table} m \
             WHERE {same_key} AND NOT ({differs}))"
        ),
        [],
    )?;
    transaction.execute(
        &format!(
            "INSERT INTO main.{table}_revision SELECT m.*, ? FROM main.{table} m \
             JOIN stage_{table} s ON {same_key}"
        ),
        params![batch_id],
    )?;
    let written = transaction.execute(
        &format!("INSERT OR REPLACE INTO main.{table} SELECT * FROM stage_{table}"),
        [],
    )?;
    transaction.execute(&format!("DELETE FROM stage_{table}"), [])?;
    Ok(written as u64)
}

pub(crate) fn instruments(
    connection: &mut Connection,
    source: DataSource,
    instruments: Vec<Instrument>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<Symbol, Instrument> = instruments
        .into_iter()
        .map(|instrument| (instrument.symbol.clone(), instrument))
        .collect();
    write_batch(
        connection,
        source,
        Dataset::Instruments,
        Coverage::new(),
        |transaction, batch_id| {
            stage(
                transaction,
                "instrument",
                unique.values(),
                |appender, instrument| {
                    appender.append_row(params![
                        instrument.symbol.as_str(),
                        instrument.name,
                        instrument.exchange,
                        wire_name(&instrument.asset_class),
                        instrument.active,
                        instrument.shortable,
                        instrument.first_session,
                        instrument.last_session,
                        batch_id,
                    ])
                },
            )?;
            replace_staged(transaction, "instrument", batch_id)
        },
    )
}

pub(crate) fn sessions(
    connection: &mut Connection,
    source: DataSource,
    replaced: Option<(NaiveDate, NaiveDate)>,
    sessions: Vec<Session>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<NaiveDate, Session> = sessions
        .into_iter()
        .map(|session| (session.date, session))
        .collect();
    let mut coverage = Coverage::new();
    for date in unique.keys() {
        cover(&mut coverage, None, *date);
    }
    write_batch(
        connection,
        source,
        Dataset::Calendar,
        coverage,
        |transaction, batch_id| {
            stage(
                transaction,
                "calendar",
                unique.values(),
                |appender, session| {
                    appender.append_row(params![
                        session.date,
                        Ts(session.open),
                        Ts(session.close),
                        batch_id
                    ])
                },
            )?;
            if let Some((from, to)) = replaced {
                let removed = "FROM main.calendar WHERE date BETWEEN ? AND ? \
                     AND date NOT IN (SELECT date FROM stage_calendar)";
                transaction.execute(
                    &format!("INSERT INTO main.calendar_revision SELECT *, ? {removed}"),
                    params![batch_id, from, to],
                )?;
                transaction.execute(&format!("DELETE {removed}"), params![from, to])?;
            }
            replace_staged(transaction, "calendar", batch_id)
        },
    )
}

pub(crate) fn daily_bars(
    connection: &mut Connection,
    source: DataSource,
    bars: Vec<Bar>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<(Symbol, NaiveDate), Bar> = bars
        .into_iter()
        .map(|bar| ((bar.symbol.clone(), bar.start.exchange_date()), bar))
        .collect();
    let mut coverage = Coverage::new();
    for (symbol, date) in unique.keys() {
        cover(&mut coverage, Some(symbol), *date);
    }
    let source_name = wire_name(&source);
    write_batch(
        connection,
        source,
        Dataset::Bars1d,
        coverage,
        |transaction, batch_id| {
            stage(
                transaction,
                "bar_1d",
                &unique,
                |appender, ((symbol, date), bar)| {
                    appender.append_row(params![
                        symbol.as_str(),
                        date,
                        Dec(bar.open),
                        Dec(bar.high),
                        Dec(bar.low),
                        Dec(bar.close),
                        bar.volume,
                        source_name,
                        batch_id,
                    ])
                },
            )?;
            replace_staged(transaction, "bar_1d", batch_id)
        },
    )
}

pub(crate) fn quiet_sessions(
    connection: &mut Connection,
    source: DataSource,
    sessions: Vec<(Symbol, NaiveDate)>,
) -> Result<WriteSummary> {
    let source_name = wire_name(&source);
    write_batch(
        connection,
        source,
        Dataset::Bars1d,
        Coverage::new(),
        |transaction, batch_id| {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO main.quiet_session VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
            )?;
            let mut rows = 0;
            for (symbol, date) in &sessions {
                rows += statement.execute(params![symbol.as_str(), date, source_name, batch_id])?;
            }
            Ok(rows as u64)
        },
    )
}

pub(crate) fn ended_quiet_sessions(
    connection: &mut Connection,
    source: DataSource,
    ticker: Symbol,
    ended: Symbol,
    last: NaiveDate,
) -> Result<WriteSummary> {
    write_batch(
        connection,
        source,
        Dataset::Bars1d,
        Coverage::new(),
        |transaction, batch_id| {
            let rows = transaction.execute(
                "INSERT INTO main.quiet_session SELECT ?, date, source, ? FROM main.quiet_session \
                 WHERE symbol = ? AND date <= ? ON CONFLICT DO NOTHING",
                params![ended.as_str(), batch_id, ticker.as_str(), last],
            )?;
            Ok(rows as u64)
        },
    )
}

const SCATTERED_ROW_GROUPS_TOLERATED: i64 = 1;

pub(crate) fn scattered_daily_row_groups(connection: &Connection) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT count(DISTINCT row_group_id) FROM (SELECT row_group_id, \
         CAST(stats.min AS DATE) AS first, max(CAST(stats.max AS DATE)) OVER (ORDER BY \
         row_group_id, segment_id ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS seen \
         FROM pragma_storage_info('bar_1d') WHERE column_name = 'date' AND segment_type = 'DATE') \
         WHERE first < seen - INTERVAL 30 DAY",
        [],
        |row| row.get(0),
    )?)
}

pub(crate) fn order_daily_bars(connection: &mut Connection) -> Result<Option<u64>> {
    if scattered_daily_row_groups(connection)? <= SCATTERED_ROW_GROUPS_TOLERATED {
        return Ok(None);
    }
    let definition: String = connection.query_row(
        "SELECT sql FROM duckdb_tables() WHERE database_name = current_database() \
         AND schema_name = 'main' AND table_name = 'bar_1d'",
        [],
        |row| row.get(0),
    )?;
    let columns = definition
        .strip_prefix("CREATE TABLE bar_1d")
        .ok_or_else(|| {
            MarketError::Catalog(format!("unexpected bar_1d definition {definition}"))
        })?;
    let transaction = connection.transaction()?;
    transaction.execute_batch(&format!("CREATE TABLE main.ordered_bar_1d{columns}"))?;
    let rows = transaction.execute(
        "INSERT INTO main.ordered_bar_1d SELECT * FROM main.bar_1d ORDER BY date, symbol",
        [],
    )?;
    transaction.execute_batch(
        "DROP TABLE main.bar_1d; ALTER TABLE main.ordered_bar_1d RENAME TO bar_1d",
    )?;
    transaction.commit()?;
    Ok(Some(rows as u64))
}

pub(crate) fn shadowed_bars(
    connection: &mut Connection,
    source: DataSource,
    shadowed: Vec<(Symbol, NaiveDate, NaiveDate, Symbol)>,
) -> Result<WriteSummary> {
    write_batch(
        connection,
        source,
        Dataset::Bars1d,
        Coverage::new(),
        |transaction, batch_id| {
            transaction.execute("DELETE FROM shadow_span", [])?;
            let mut appender =
                transaction.appender_to_catalog_and_db("shadow_span", "temp", "main")?;
            for (ticker, lo, hi, listing) in &shadowed {
                appender.append_row(params![ticker.as_str(), lo, hi, listing.as_str()])?;
            }
            appender.flush()?;
            drop(appender);
            let withdrawn = transaction.execute(
                "INSERT INTO main.bar_1d_revision SELECT b.*, ? FROM main.bar_1d b \
                 JOIN shadow_span s ON b.symbol = s.ticker AND b.date BETWEEN s.lo AND s.hi \
                 WHERE EXISTS (SELECT 1 FROM main.bar_1d k WHERE k.symbol = s.listing \
                 AND k.date = b.date AND k.open = b.open AND k.high = b.high AND k.low = b.low \
                 AND k.close = b.close AND k.volume = b.volume)",
                params![batch_id],
            )?;
            transaction.execute(
                "DELETE FROM main.bar_1d b WHERE EXISTS (SELECT 1 FROM main.bar_1d_revision r \
                 WHERE r.superseded_by = ? AND r.symbol = b.symbol AND r.date = b.date)",
                params![batch_id],
            )?;
            transaction.execute("DELETE FROM shadow_span", [])?;
            Ok(withdrawn as u64)
        },
    )
}

pub(crate) fn minute_bars(
    connection: &mut Connection,
    source: DataSource,
    bars: Vec<Bar>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<(Symbol, Timestamp), Bar> = bars
        .into_iter()
        .map(|bar| ((bar.symbol.clone(), bar.start), bar))
        .collect();
    let mut spans: BTreeMap<(&Symbol, NaiveDate), (Timestamp, Timestamp)> = BTreeMap::new();
    for (symbol, start) in unique.keys() {
        spans
            .entry((symbol, start.exchange_date()))
            .and_modify(|(lo, hi)| {
                *lo = (*lo).min(*start);
                *hi = (*hi).max(*start);
            })
            .or_insert((*start, *start));
    }
    let mut coverage = Coverage::new();
    for (symbol, date) in spans.keys() {
        cover(&mut coverage, Some(symbol), *date);
    }
    let source_name = wire_name(&source);
    write_batch(
        connection,
        source,
        Dataset::Bars1m,
        coverage,
        |transaction, batch_id| {
            transaction.execute("DELETE FROM bar_span", [])?;
            let mut appender =
                transaction.appender_to_catalog_and_db("bar_span", "temp", "main")?;
            for ((symbol, _), (lo, hi)) in &spans {
                appender.append_row(params![symbol.as_str(), Ts(*lo), Ts(*hi)])?;
            }
            appender.flush()?;
            drop(appender);
            transaction.execute_batch(
                "DELETE FROM main.bar_1m USING bar_span \
                 WHERE bar_1m.symbol = bar_span.symbol \
                 AND bar_1m.start BETWEEN bar_span.lo AND bar_span.hi; \
                 DELETE FROM bar_span",
            )?;
            let mut appender = transaction.appender("bar_1m")?;
            for bar in unique.values() {
                appender.append_row(params![
                    bar.symbol.as_str(),
                    Ts(bar.start),
                    Dec(bar.open),
                    Dec(bar.high),
                    Dec(bar.low),
                    Dec(bar.close),
                    bar.volume,
                    feed_name(bar.feed),
                    source_name,
                    batch_id,
                ])?;
            }
            appender.flush()?;
            Ok(unique.len() as u64)
        },
    )
}

pub(crate) fn corporate_actions(
    connection: &mut Connection,
    source: DataSource,
    actions: Vec<CorporateAction>,
    window: &ActionWindow,
    market_wide: Option<&'static str>,
) -> Result<WriteSummary> {
    let ActionWindow {
        symbols: covered,
        from,
        through: to,
        covered_to,
    } = window;
    let (from, to) = (*from, *to);
    let unique: BTreeMap<(Symbol, NaiveDate, &'static str, String), CorporateAction> = actions
        .into_iter()
        .filter(|action| {
            let valid = action.kind.has_valid_rates();
            if !valid {
                tracing::warn!(symbol = %action.symbol, ex_date = %action.ex_date, kind = ?action.kind, "refused a corporate action without positive rates");
            }
            valid
        })
        .map(|action| {
            let columns = ActionColumns::of(&action.kind);
            (
                (
                    action.symbol.clone(),
                    action.ex_date,
                    columns.kind,
                    columns.other_symbol,
                ),
                action,
            )
        })
        .collect();
    let mut coverage = Coverage::new();
    if let Some(covered_to) = covered_to {
        if market_wide.is_some() {
            cover(&mut coverage, None, from);
            cover(&mut coverage, None, *covered_to);
        }
        for symbol in covered {
            cover(&mut coverage, Some(symbol), from);
            cover(&mut coverage, Some(symbol), *covered_to);
        }
    }
    let source_name = wire_name(&source);
    let scope = match market_wide {
        Some(scope) => scope.to_owned(),
        None => {
            "c.kind <> 'name_change' AND c.symbol IN (SELECT symbol FROM action_scope)".to_owned()
        }
    };
    write_batch(
        connection,
        source,
        Dataset::CorporateActions,
        coverage,
        |transaction, batch_id| {
            stage(
                transaction,
                "corporate_action",
                unique.values(),
                |appender, action| append_action(appender, action, &source_name, batch_id),
            )?;
            transaction.execute("DELETE FROM action_scope", [])?;
            let mut appender =
                transaction.appender_to_catalog_and_db("action_scope", "temp", "main")?;
            for symbol in covered {
                appender.append_row(params![symbol.as_str()])?;
            }
            appender.flush()?;
            drop(appender);
            let withdrawn = format!(
                "FROM main.corporate_action c WHERE c.source = ? \
                 AND c.ex_date BETWEEN ? AND ? AND {scope} \
                 AND NOT EXISTS (SELECT 1 FROM stage_corporate_action s \
                 WHERE s.symbol = c.symbol AND s.ex_date = c.ex_date AND s.kind = c.kind \
                 AND s.other_symbol = c.other_symbol)"
            );
            transaction.execute(
                &format!("INSERT INTO main.corporate_action_revision SELECT c.*, ? {withdrawn}"),
                params![batch_id, source_name, from, to],
            )?;
            transaction.execute(
                &format!("DELETE {withdrawn}"),
                params![source_name, from, to],
            )?;
            replace_staged(transaction, "corporate_action", batch_id)
        },
    )
}

fn append_action(
    appender: &mut Appender<'_>,
    action: &CorporateAction,
    source_name: &str,
    batch_id: i64,
) -> duckdb::Result<()> {
    let columns = ActionColumns::of(&action.kind);
    appender.append_row(params![
        action.symbol.as_str(),
        action.ex_date,
        columns.kind,
        columns.other_symbol,
        columns.old_rate.map(Dec),
        columns.new_rate.map(Dec),
        columns.rate.map(Dec),
        source_name,
        batch_id,
    ])
}

pub(crate) fn normalized_splits(connection: &mut Connection, source: DataSource) -> Result<u64> {
    let source_name = wire_name(&source);
    let mut statement = connection.prepare(
        "SELECT symbol, ex_date, old_rate, new_rate FROM corporate_action \
         WHERE kind = 'split' AND source = ? AND old_rate > 0 AND new_rate > 0",
    )?;
    let stored = statement
        .query_map(params![source_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, NaiveDate>(1)?,
                row.get::<_, Dec>(2)?.0,
                row.get::<_, Dec>(3)?.0,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    drop(statement);
    let changed: Vec<CorporateAction> = stored
        .into_iter()
        .filter_map(|(symbol, ex_date, old_rate, new_rate)| {
            let (old, new) = split_rates(new_rate / old_rate);
            (old != old_rate || new != new_rate).then(|| CorporateAction {
                symbol: Symbol::new(&symbol),
                ex_date,
                kind: CorporateActionKind::Split {
                    old_rate: old,
                    new_rate: new,
                },
            })
        })
        .collect();
    if changed.is_empty() {
        return Ok(0);
    }
    let summary = write_batch(
        connection,
        source,
        Dataset::CorporateActions,
        Coverage::new(),
        |transaction, batch_id| {
            stage(
                transaction,
                "corporate_action",
                &changed,
                |appender, action| append_action(appender, action, &source_name, batch_id),
            )?;
            replace_staged(transaction, "corporate_action", batch_id)
        },
    )?;
    Ok(summary.rows)
}

pub(crate) fn action_checks(
    connection: &mut Connection,
    source: DataSource,
    checks: Vec<ActionDisagreement>,
) -> Result<WriteSummary> {
    write_batch(
        connection,
        source,
        Dataset::CorporateActions,
        Coverage::new(),
        |transaction, batch_id| {
            stage(
                transaction,
                "corporate_action_check",
                &checks,
                |appender, check| {
                    appender.append_row(params![
                        check.symbol.as_str(),
                        check.ex_date,
                        wire_name(&check.action),
                        check.alpaca.map(Dec),
                        check.tiingo.map(Dec),
                        wire_name(&check.kept),
                        batch_id,
                    ])
                },
            )?;
            replace_staged(transaction, "corporate_action_check", batch_id)
        },
    )
}

pub(crate) fn fundamental_facts(
    connection: &mut Connection,
    source: DataSource,
    facts: Vec<FundamentalFact>,
) -> Result<WriteSummary> {
    let mut seen = BTreeSet::new();
    let unique: Vec<FundamentalFact> = facts
        .into_iter()
        .filter(|fact| {
            seen.insert((
                fact.symbol.clone(),
                fact.cik,
                fact.taxonomy.clone(),
                fact.concept.clone(),
                fact.unit.clone(),
                fact.period_start,
                fact.period_end,
                fact.accession.clone(),
            ))
        })
        .collect();
    let mut coverage = Coverage::new();
    for fact in &unique {
        cover(&mut coverage, Some(&fact.symbol), fact.filed);
    }
    write_batch(
        connection,
        source,
        Dataset::Fundamentals,
        coverage,
        |transaction, batch_id| {
            stage(
                transaction,
                "fundamental_fact",
                &unique,
                |appender, fact| {
                    appender.append_row(params![
                        fact.symbol.as_str(),
                        fact.cik,
                        fact.taxonomy,
                        fact.concept,
                        fact.unit,
                        fact.metric.as_ref().map(wire_name),
                        fact.period_type.as_ref().map(wire_name),
                        fact.period_start,
                        fact.period_end,
                        Dec(fact.value),
                        fact.fiscal_year,
                        fact.fiscal_period,
                        fact.form,
                        fact.accession,
                        fact.filed,
                        fact.frame,
                        Ts(fact.available_at),
                        batch_id,
                    ])
                },
            )?;
            let same_fact = "f.symbol = s.symbol AND f.cik = s.cik AND f.taxonomy = s.taxonomy \
                 AND f.concept = s.concept AND f.unit = s.unit \
                 AND f.period_start IS NOT DISTINCT FROM s.period_start \
                 AND f.period_end = s.period_end AND f.accession = s.accession";
            transaction.execute(
                &format!(
                    "DELETE FROM main.fundamental_fact f WHERE EXISTS (SELECT 1 \
                     FROM stage_fundamental_fact s WHERE f.symbol = s.symbol AND f.cik = s.cik \
                     AND f.accession = s.accession) \
                     AND NOT EXISTS (SELECT 1 FROM stage_fundamental_fact s WHERE {same_fact} \
                     AND f.value = s.value AND f.metric IS NOT DISTINCT FROM s.metric \
                     AND f.period_type IS NOT DISTINCT FROM s.period_type)"
                ),
                [],
            )?;
            let inserted = transaction.execute(
                &format!(
                    "INSERT INTO main.fundamental_fact SELECT s.* FROM stage_fundamental_fact s \
                     WHERE NOT EXISTS (SELECT 1 FROM main.fundamental_fact f WHERE {same_fact})"
                ),
                [],
            )?;
            transaction.execute("DELETE FROM stage_fundamental_fact", [])?;
            Ok(inserted as u64)
        },
    )
}

pub(crate) fn risk_free_rates(
    connection: &mut Connection,
    source: DataSource,
    rates: Vec<RiskFreeRate>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<NaiveDate, RiskFreeRate> =
        rates.into_iter().map(|rate| (rate.date, rate)).collect();
    let mut coverage = Coverage::new();
    for date in unique.keys() {
        cover(&mut coverage, None, *date);
    }
    let source_name = wire_name(&source);
    write_batch(
        connection,
        source,
        Dataset::Rates,
        coverage,
        |transaction, batch_id| {
            stage(
                transaction,
                "rate_daily",
                unique.values(),
                |appender, rate| {
                    appender.append_row(params![
                        rate.date,
                        Dec(rate.percent),
                        source_name,
                        batch_id
                    ])
                },
            )?;
            replace_staged(transaction, "rate_daily", batch_id)
        },
    )
}

pub(crate) fn macro_observations(
    connection: &mut Connection,
    source: DataSource,
    observations: Vec<MacroObservation>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<(String, NaiveDate, NaiveDate), Option<Dec>> = observations
        .into_iter()
        .map(|observation| {
            (
                (
                    wire_name(&observation.series),
                    observation.date,
                    observation.published,
                ),
                observation.value.map(Dec),
            )
        })
        .collect();
    let mut coverage = Coverage::new();
    for (_, date, _) in unique.keys() {
        cover(&mut coverage, None, *date);
    }
    let source_name = wire_name(&source);
    write_batch(
        connection,
        source,
        Dataset::Macro,
        coverage,
        |transaction, batch_id| {
            stage(
                transaction,
                "macro_observation",
                unique,
                |appender, ((series, date, published), value)| {
                    appender.append_row(params![
                        series,
                        date,
                        published,
                        value,
                        source_name,
                        batch_id
                    ])
                },
            )?;
            replace_staged(transaction, "macro_observation", batch_id)
        },
    )
}

pub(crate) fn fomc_meetings(
    connection: &mut Connection,
    source: DataSource,
    years: RangeInclusive<i32>,
    meetings: Vec<FomcMeeting>,
) -> Result<WriteSummary> {
    let unique: BTreeSet<FomcMeeting> = meetings.into_iter().collect();
    let mut coverage = Coverage::new();
    for meeting in &unique {
        cover(&mut coverage, None, meeting.date);
    }
    let source_name = wire_name(&source);
    let (first, last) = years.into_inner();
    write_batch(
        connection,
        source,
        Dataset::Fomc,
        coverage,
        |transaction, batch_id| {
            stage(transaction, "fomc_meeting", unique, |appender, meeting| {
                appender.append_row(params![
                    meeting.date,
                    meeting.scheduled,
                    meeting.statement_at.map(Ts),
                    source_name,
                    batch_id
                ])
            })?;
            let withdrawn = "FROM main.fomc_meeting m WHERE year(m.date) BETWEEN ? AND ? \
                 AND NOT EXISTS (SELECT 1 FROM stage_fomc_meeting s \
                 WHERE s.date = m.date AND s.scheduled = m.scheduled)";
            transaction.execute(
                &format!("INSERT INTO main.fomc_meeting_revision SELECT m.*, ? {withdrawn}"),
                params![batch_id, first, last],
            )?;
            let removed =
                transaction.execute(&format!("DELETE {withdrawn}"), params![first, last])?;
            Ok(removed as u64 + replace_staged(transaction, "fomc_meeting", batch_id)?)
        },
    )
}

pub(crate) fn earnings_insight(
    connection: &mut Connection,
    source: DataSource,
    report: EarningsInsight,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    cover(&mut coverage, None, report.issue_date);
    write_batch(
        connection,
        source,
        Dataset::Estimates,
        coverage,
        |transaction, batch_id| {
            let written = transaction.execute(
                "INSERT INTO earnings_insight VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT (report) DO NOTHING",
                params![
                    report.report,
                    report.issue_date,
                    report.published,
                    report.url,
                    report.retrieved_from,
                    report.digest,
                    report.text,
                    batch_id,
                ],
            )?;
            Ok(written as u64)
        },
    )
}

pub(crate) fn earnings_insight_facts(
    connection: &mut Connection,
    source: DataSource,
    facts: BTreeMap<String, Vec<EstimateFact>>,
) -> Result<WriteSummary> {
    write_batch(
        connection,
        source,
        Dataset::Estimates,
        Coverage::new(),
        |transaction, batch_id| {
            transaction.execute("DELETE FROM stage_earnings_insight_fact", [])?;
            let mut appender = transaction.appender_to_catalog_and_db(
                "stage_earnings_insight_fact",
                "temp",
                "main",
            )?;
            for (report, fact) in facts
                .iter()
                .flat_map(|(report, facts)| facts.iter().map(move |fact| (report, fact)))
            {
                appender.append_row(params![
                    report,
                    wire_name(&fact.metric),
                    wire_name(&fact.scope),
                    fact.period
                        .map(|period| period.to_string())
                        .unwrap_or_default(),
                    fact.as_of,
                    Dec(fact.value),
                    fact.metric.unit(),
                    batch_id,
                ])?;
            }
            appender.flush()?;
            drop(appender);
            transaction.execute("DELETE FROM report_scope", [])?;
            let mut appender =
                transaction.appender_to_catalog_and_db("report_scope", "temp", "main")?;
            for report in facts.keys() {
                appender.append_row(params![report])?;
            }
            appender.flush()?;
            drop(appender);
            let withdrawn = "FROM main.earnings_insight_fact f \
                 WHERE f.report IN (SELECT report FROM report_scope) \
                 AND NOT EXISTS (SELECT 1 FROM stage_earnings_insight_fact s \
                 WHERE s.report = f.report AND s.metric = f.metric AND s.scope = f.scope \
                 AND s.period = f.period)";
            transaction.execute(
                &format!(
                    "INSERT INTO main.earnings_insight_fact_revision SELECT f.*, ? {withdrawn}"
                ),
                params![batch_id],
            )?;
            let removed = transaction.execute(&format!("DELETE {withdrawn}"), [])?;
            Ok(removed as u64 + replace_staged(transaction, "earnings_insight_fact", batch_id)?)
        },
    )
}

pub(crate) fn companies(
    connection: &mut Connection,
    source: DataSource,
    companies: Vec<Company>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<Symbol, Company> = companies
        .into_iter()
        .map(|company| (company.symbol.clone(), company))
        .collect();
    write_batch(
        connection,
        source,
        Dataset::Companies,
        Coverage::new(),
        |transaction, batch_id| {
            stage(
                transaction,
                "company",
                unique.values(),
                |appender, company| {
                    appender.append_row(params![
                        company.symbol.as_str(),
                        company.cik,
                        company.sic,
                        company.sic_description,
                        batch_id,
                    ])
                },
            )?;
            replace_staged(transaction, "company", batch_id)
        },
    )
}

pub(crate) fn earnings_releases(
    connection: &mut Connection,
    source: DataSource,
    releases: Vec<EarningsRelease>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<(Symbol, String), EarningsRelease> = releases
        .into_iter()
        .map(|release| ((release.symbol.clone(), release.accession.clone()), release))
        .collect();
    let mut coverage = Coverage::new();
    for release in unique.values() {
        cover(&mut coverage, Some(&release.symbol), release.filed);
    }
    write_batch(
        connection,
        source,
        Dataset::EarningsReleases,
        coverage,
        |transaction, batch_id| {
            stage(
                transaction,
                "earnings_release",
                unique.values(),
                |appender, release| {
                    appender.append_row(params![
                        release.symbol.as_str(),
                        release.cik,
                        release.accession,
                        release.form,
                        release.filed,
                        release.report_date,
                        Ts(release.accepted_at),
                        batch_id,
                    ])
                },
            )?;
            replace_staged(transaction, "earnings_release", batch_id)
        },
    )
}

pub(crate) fn sec_filings(
    connection: &mut Connection,
    source: DataSource,
    filings: Vec<SecFiling>,
    day: NaiveDate,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    cover(&mut coverage, None, day);
    write_batch(
        connection,
        source,
        Dataset::SecFilings,
        coverage,
        |transaction, batch_id| {
            let mut statement = transaction
                .prepare_cached("INSERT OR IGNORE INTO main.sec_filing VALUES (?, ?, ?, ?, ?)")?;
            let mut rows = 0;
            for filing in &filings {
                rows += statement.execute(params![
                    filing.cik,
                    filing.accession,
                    filing.form,
                    filing.filed,
                    batch_id
                ])? as u64;
            }
            Ok(rows)
        },
    )
}

pub(crate) fn index_history(
    connection: &mut Connection,
    source: DataSource,
    history: IndexHistory,
    today: NaiveDate,
) -> Result<WriteSummary> {
    let index = wire_name(&history.index);
    let mut coverage = Coverage::new();
    let first = history
        .members
        .iter()
        .flat_map(|member| [member.start, member.end])
        .flatten()
        .min();
    if let Some(first) = first {
        cover(&mut coverage, None, first);
        cover(&mut coverage, None, today);
    }
    write_batch(
        connection,
        source,
        Dataset::IndexMembership,
        coverage,
        |transaction, batch_id| {
            for table in ["index_member", "index_history"] {
                transaction.execute(
                    &format!(
                        "INSERT INTO main.{table}_revision SELECT *, ? FROM main.{table} \
                         WHERE index_name = ?"
                    ),
                    params![batch_id, index],
                )?;
            }
            transaction.execute(
                "DELETE FROM main.index_member WHERE index_name = ?",
                params![index],
            )?;
            transaction.execute(
                "INSERT OR REPLACE INTO main.index_history VALUES (?, ?, ?, ?)",
                params![index, history.changes, history.reliable_from, batch_id],
            )?;
            let mut appender = transaction.appender("index_member")?;
            for member in &history.members {
                appender.append_row(params![
                    index,
                    member.symbol.as_str(),
                    member.start,
                    member.end,
                    batch_id,
                ])?;
            }
            appender.flush()?;
            Ok(history.members.len() as u64)
        },
    )
}

pub(crate) fn daily_watermark(connection: &Connection, through: Timestamp) -> Result<()> {
    connection.execute(
        "INSERT OR REPLACE INTO daily_watermark (through) VALUES (?)",
        params![Ts(through)],
    )?;
    Ok(())
}

pub(crate) fn borrow_availability(
    connection: &mut Connection,
    source: DataSource,
    records: Vec<BorrowAvailability>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<(Symbol, Timestamp), BorrowAvailability> = records
        .into_iter()
        .map(|record| ((record.symbol.clone(), record.at), record))
        .collect();
    let mut coverage = Coverage::new();
    for (_, at) in unique.keys() {
        cover(&mut coverage, None, at.exchange_date());
    }
    write_batch(
        connection,
        source,
        Dataset::Borrow,
        coverage,
        |transaction, batch_id| {
            let mut appender = transaction.appender("borrow_availability")?;
            for record in unique.values() {
                appender.append_row(params![
                    record.symbol.as_str(),
                    Ts(record.at),
                    record.con_id,
                    record.rebate_rate.map(Dec),
                    record.fee_rate.map(Dec),
                    record.available,
                    record.available_or_more,
                    batch_id,
                ])?;
            }
            appender.flush()?;
            Ok(unique.len() as u64)
        },
    )
}

pub(crate) fn short_interest(
    connection: &mut Connection,
    source: DataSource,
    settlement: NaiveDate,
    records: Vec<ShortInterestRecord>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    cover(&mut coverage, None, settlement);
    write_batch(
        connection,
        source,
        Dataset::ShortInterest,
        coverage,
        |transaction, batch_id| {
            let mut appender = transaction.appender("short_interest")?;
            for record in &records {
                appender.append_row(params![
                    record.symbol.as_str(),
                    record.settlement,
                    record.shares,
                    record.previous_shares,
                    record.average_daily_volume,
                    Dec(record.days_to_cover),
                    record.split,
                    record.revised,
                    record.published,
                    batch_id,
                ])?;
            }
            appender.flush()?;
            Ok(records.len() as u64)
        },
    )
}

pub(crate) fn short_volume(
    connection: &mut Connection,
    source: DataSource,
    date: NaiveDate,
    records: Vec<ShortVolumeRecord>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    cover(&mut coverage, None, date);
    write_batch(
        connection,
        source,
        Dataset::ShortVolume,
        coverage,
        |transaction, batch_id| {
            let mut appender = transaction.appender("short_volume")?;
            for record in &records {
                appender.append_row(params![
                    record.symbol.as_str(),
                    record.date,
                    Dec(record.short_volume),
                    Dec(record.short_exempt_volume),
                    Dec(record.total_volume),
                    batch_id,
                ])?;
            }
            appender.flush()?;
            Ok(records.len() as u64)
        },
    )
}

pub(crate) fn insider_filings(
    connection: &mut Connection,
    source: DataSource,
    filings: Vec<InsiderFiling>,
) -> Result<WriteSummary> {
    let unique: BTreeMap<(Symbol, String), InsiderFiling> = filings
        .into_iter()
        .map(|filing| ((filing.symbol.clone(), filing.accession.clone()), filing))
        .collect();
    let mut coverage = Coverage::new();
    for filing in unique.values() {
        cover(&mut coverage, Some(&filing.symbol), filing.filed);
    }
    write_batch(
        connection,
        source,
        Dataset::InsiderTrades,
        coverage,
        |transaction, batch_id| {
            let mut stored = transaction.prepare_cached(
                "SELECT count(*) > 0 FROM main.insider_filing WHERE symbol = ? AND accession = ?",
            )?;
            let mut filed = transaction.appender("insider_filing")?;
            let mut plans = transaction.appender("insider_plan")?;
            let mut owners = transaction.appender("insider_owner")?;
            let mut trades = transaction.appender("insider_transaction")?;
            let mut rows = 0;
            for filing in unique.values() {
                let symbol = filing.symbol.as_str();
                let known: bool =
                    stored.query_row(params![symbol, filing.accession], |row| row.get(0))?;
                if known {
                    continue;
                }
                filed.append_row(params![
                    symbol,
                    filing.issuer_cik,
                    filing.accession,
                    filing.form,
                    filing.filed,
                    Ts(filing.accepted_at),
                    batch_id,
                ])?;
                plans.append_row(params![
                    symbol,
                    filing.accession,
                    filing.plan_10b5_1,
                    batch_id
                ])?;
                for owner in &filing.owners {
                    owners.append_row(params![
                        symbol,
                        filing.accession,
                        owner.cik,
                        owner.name,
                        owner.director,
                        owner.officer,
                        owner.ten_percent_owner,
                        owner.other,
                        owner.officer_title,
                        batch_id,
                    ])?;
                }
                for (line, trade) in (0_u32..).zip(&filing.transactions) {
                    trades.append_row(params![
                        symbol,
                        filing.accession,
                        line,
                        trade.security,
                        trade.date,
                        trade.code,
                        trade.acquired,
                        trade.shares.map(Dec),
                        trade.price.map(Dec),
                        trade.shares_after.map(Dec),
                        trade.direct,
                        batch_id,
                    ])?;
                }
                rows += 1;
            }
            filed.flush()?;
            plans.flush()?;
            owners.flush()?;
            trades.flush()?;
            Ok(rows)
        },
    )
}

pub(crate) fn insider_plans(
    connection: &mut Connection,
    source: DataSource,
    plans: Vec<InsiderPlan>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    for plan in &plans {
        cover(&mut coverage, Some(&plan.symbol), plan.filed);
    }
    write_batch(
        connection,
        source,
        Dataset::InsiderTrades,
        coverage,
        |transaction, batch_id| {
            let mut statement = transaction
                .prepare_cached("INSERT OR IGNORE INTO main.insider_plan VALUES (?, ?, ?, ?)")?;
            let mut rows = 0;
            for plan in &plans {
                rows += statement.execute(params![
                    plan.symbol.as_str(),
                    plan.accession,
                    plan.plan_10b5_1,
                    batch_id,
                ])? as u64;
            }
            Ok(rows)
        },
    )
}

pub(crate) fn current_reports(
    connection: &mut Connection,
    source: DataSource,
    reports: Vec<Filing>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    for report in &reports {
        cover(&mut coverage, Some(&report.symbol), report.filed);
    }
    write_batch(
        connection,
        source,
        Dataset::CurrentReports,
        coverage,
        |transaction, batch_id| {
            let mut statement = transaction.prepare_cached(
                "INSERT OR IGNORE INTO main.current_report VALUES (?, ?, ?, ?, ?, ?, \
                 list_filter(string_split(?, ','), lambda item: item <> ''), ?, ?)",
            )?;
            let mut rows = 0;
            for report in &reports {
                rows += statement.execute(params![
                    report.symbol.as_str(),
                    report.cik,
                    report.accession,
                    report.form,
                    report.filed,
                    report.report_date,
                    report.items.join(","),
                    Ts(report.accepted_at),
                    batch_id,
                ])? as u64;
            }
            Ok(rows)
        },
    )
}

pub(crate) fn periodic_reports(
    connection: &mut Connection,
    source: DataSource,
    reports: Vec<Filing>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    for report in &reports {
        cover(&mut coverage, Some(&report.symbol), report.filed);
    }
    write_batch(
        connection,
        source,
        Dataset::PeriodicReports,
        coverage,
        |transaction, batch_id| {
            let mut statement = transaction.prepare_cached(
                "INSERT OR IGNORE INTO main.periodic_report VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            let mut rows = 0;
            for report in &reports {
                rows += statement.execute(params![
                    report.symbol.as_str(),
                    report.cik,
                    report.accession,
                    report.form,
                    report.filed,
                    report.report_date,
                    Ts(report.accepted_at),
                    batch_id,
                ])? as u64;
            }
            Ok(rows)
        },
    )
}

pub(crate) fn document_sections(
    connection: &mut Connection,
    source: DataSource,
    documents: Vec<DocumentSections>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    for document in &documents {
        cover(&mut coverage, None, document.published_at.exchange_date());
    }
    write_batch(
        connection,
        source,
        Dataset::FilingSections,
        coverage,
        |transaction, batch_id| {
            let mut statement = transaction.prepare_cached(
                "INSERT OR IGNORE INTO main.section VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            let mut rows = 0;
            for document in &documents {
                let rows_of: Vec<(Option<&Section>, Option<&str>)> = match &document.outcome {
                    Ok(sections) => sections
                        .iter()
                        .map(|section| (Some(section), None))
                        .collect(),
                    Err(reason) => vec![(None, Some(reason.as_str()))],
                };
                for (ordinal, (section, failure)) in rows_of.into_iter().enumerate() {
                    rows += statement.execute(params![
                        document.doc_id,
                        document.content_hash,
                        document.splitter_version,
                        u32::try_from(ordinal).unwrap_or(u32::MAX),
                        section.map(|section| section.key.as_str()),
                        section.map(|section| section.start),
                        section.map(|section| section.end),
                        failure,
                        batch_id,
                    ])? as u64;
                }
            }
            Ok(rows)
        },
    )
}

pub(crate) fn press_releases(
    connection: &mut Connection,
    source: DataSource,
    releases: Vec<PressExhibit>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    for release in &releases {
        cover(&mut coverage, None, release.accepted_at.exchange_date());
    }
    write_batch(
        connection,
        source,
        Dataset::PressReleases,
        coverage,
        |transaction, batch_id| {
            let mut statement = transaction.prepare_cached(
                "INSERT OR IGNORE INTO main.press_release VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            let mut rows = 0;
            for release in &releases {
                let exhibit = release.exhibit.as_ref();
                rows += statement.execute(params![
                    release.accession,
                    release.cik,
                    Ts(release.accepted_at),
                    exhibit.map(|exhibit| exhibit.document.as_str()),
                    exhibit.map(|exhibit| exhibit.kind.as_str()),
                    exhibit.map(|exhibit| exhibit.text.as_str()),
                    batch_id,
                ])? as u64;
            }
            Ok(rows)
        },
    )
}

pub(crate) fn filing_texts(
    connection: &mut Connection,
    volumes: &TextVolumes,
    source: DataSource,
    filings: Vec<FilingText>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    for filing in &filings {
        cover(&mut coverage, None, filing.accepted_at.exchange_date());
    }
    let source_name = wire_name(&source);
    let mut months: BTreeMap<String, Vec<&FilingText>> = BTreeMap::new();
    for filing in filings.iter().filter(|filing| filing.outcome.is_ok()) {
        months
            .entry(
                filing
                    .accepted_at
                    .exchange_date()
                    .format("%Y/%m")
                    .to_string(),
            )
            .or_default()
            .push(filing);
    }
    write_batch(
        connection,
        source,
        Dataset::FilingDocuments,
        coverage,
        |transaction, batch_id| {
            for (month, group) in &months {
                write_text_file(transaction, volumes, &source_name, month, group, batch_id)?;
            }
            let mut statement = transaction.prepare_cached(
                "INSERT OR IGNORE INTO main.filing_text VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            let mut rows = 0;
            for filing in &filings {
                let (documents, failure) = match &filing.outcome {
                    Ok(documents) => (documents.len(), None),
                    Err(reason) => (0, Some(reason.as_str())),
                };
                rows += statement.execute(params![
                    filing.accession,
                    filing.cik,
                    filing.form,
                    Ts(filing.accepted_at),
                    Ts(filing.retrieved_at),
                    u32::try_from(documents).unwrap_or(u32::MAX),
                    failure,
                    batch_id,
                ])? as u64;
            }
            Ok(rows)
        },
    )
}

fn write_text_file(
    transaction: &Transaction<'_>,
    volumes: &TextVolumes,
    source: &str,
    month: &str,
    filings: &[&FilingText],
    batch_id: i64,
) -> Result<()> {
    let documents: Vec<(&FilingText, &FilingDocument, String)> = filings
        .iter()
        .flat_map(|filing| {
            filing.outcome.iter().flatten().map(move |document| {
                let id = format!("{source}:{}:{}", filing.accession, document.sequence);
                (*filing, document, id)
            })
        })
        .collect();
    if documents.is_empty() {
        return Ok(());
    }
    transaction.execute("DELETE FROM stage_text_blob", [])?;
    let mut appender = transaction.appender_to_catalog_and_db("stage_text_blob", "temp", "main")?;
    let mut hashes = Vec::with_capacity(documents.len());
    for (_, document, id) in &documents {
        let hash = blake3::hash(&document.raw).to_hex().to_string();
        appender.append_row(params![id, hash, document.text, document.raw])?;
        hashes.push(hash);
    }
    appender.flush()?;
    drop(appender);
    let volume = volumes.choose()?;
    let name = format!("{batch_id}-{}.parquet", month.replace('/', "-"));
    let written = TextVolumes::scratch(volume, &name)?;
    let quoted = written.to_string_lossy().replace('\'', "''");
    transaction.execute(
        &format!(
            "COPY (SELECT doc_id, content_hash, text, raw FROM stage_text_blob ORDER BY doc_id) \
             TO '{quoted}' (FORMAT parquet, COMPRESSION zstd)"
        ),
        [],
    )?;
    let relative = format!("{source}/{month}/{batch_id}.parquet");
    let bytes = TextVolumes::place(volume, &written, &relative)?;
    let file_id: i64 =
        transaction.query_row("SELECT nextval('text_file_id')", [], |row| row.get(0))?;
    transaction.execute(
        "INSERT INTO main.text_file VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![
            file_id,
            volume.id,
            relative,
            source,
            u32::try_from(documents.len()).unwrap_or(u32::MAX),
            bytes,
            batch_id
        ],
    )?;
    let mut statement = transaction.prepare_cached(
        "INSERT OR IGNORE INTO main.text_document VALUES \
         (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    for ((filing, document, id), hash) in documents.iter().zip(&hashes) {
        statement.execute(params![
            id,
            hash,
            source,
            document.kind,
            filing.accession,
            document.sequence,
            document.document,
            document.description,
            filing.cik,
            Ts(filing.accepted_at),
            Ts(filing.retrieved_at),
            document.raw.len() as u64,
            document
                .text
                .as_ref()
                .map(|text| text.chars().count() as u64),
            file_id,
            batch_id,
        ])?;
    }
    Ok(())
}

pub(crate) fn text_volumes(connection: &Connection, volumes: &TextVolumes) -> Result<()> {
    let now = Ts(Timestamp::now());
    for volume in volumes.volumes() {
        connection.execute(
            "INSERT INTO main.text_volume VALUES (?, ?, ?) ON CONFLICT (id) \
             DO UPDATE SET path = excluded.path, seen_at = excluded.seen_at",
            params![volume.id, volume.root.to_string_lossy(), now],
        )?;
    }
    Ok(())
}

pub(crate) fn news_articles(
    connection: &mut Connection,
    source: DataSource,
    articles: Vec<NewsArticle>,
    window: Option<(NaiveDate, NaiveDate)>,
) -> Result<WriteSummary> {
    let mut coverage = Coverage::new();
    match window {
        Some((first, last)) => {
            cover(&mut coverage, None, first);
            cover(&mut coverage, None, last);
        }
        None => {
            for article in &articles {
                cover(&mut coverage, None, article.received_at.exchange_date());
            }
        }
    }
    write_batch(
        connection,
        source,
        Dataset::News,
        coverage,
        |transaction, batch_id| {
            let mut statement = transaction.prepare_cached(
                "INSERT OR IGNORE INTO main.news_article VALUES (?, ?, ?, \
                 list_filter(string_split(?, ','), lambda symbol: symbol <> ''), ?, ?, ?, ?, ?, ?)",
            )?;
            let mut rows = 0;
            for article in &articles {
                let symbols = article
                    .symbols
                    .iter()
                    .map(Symbol::as_str)
                    .collect::<Vec<_>>()
                    .join(",");
                rows += statement.execute(params![
                    article.id,
                    Ts(article.updated_at),
                    Ts(article.created_at),
                    symbols,
                    article.headline,
                    article.summary,
                    article.source,
                    Ts(article.received_at),
                    article.backfilled,
                    batch_id,
                ])? as u64;
            }
            Ok(rows)
        },
    )
}

#[cfg(test)]
mod tests {
    use buizz_core::market::{Adjustment, AdjustmentFactor, AssetClass, Feed};
    use chrono::NaiveTime;
    use rust_decimal::Decimal;

    use super::*;

    fn instrument(name: &str) -> Instrument {
        Instrument {
            symbol: Symbol::new("FB"),
            name: name.to_owned(),
            exchange: "NASDAQ".to_owned(),
            asset_class: AssetClass::Stock,
            active: true,
            shortable: true,
            first_session: None,
            last_session: None,
        }
    }

    fn session(day: u32) -> Session {
        let date = NaiveDate::from_ymd_opt(2024, 7, day).unwrap();
        let at = |hour| {
            Timestamp::at_exchange_time(date, NaiveTime::from_hms_opt(hour, 0, 0).unwrap()).unwrap()
        };
        Session {
            date,
            open: at(9),
            close: at(16),
        }
    }

    fn texts(connection: &Connection, sql: &str) -> Vec<String> {
        let mut statement = connection.prepare(sql).unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<duckdb::Result<_>>()
            .unwrap()
    }

    #[test]
    fn a_replaced_value_keeps_the_version_it_superseded() {
        let mut connection = Connection::open_in_memory().unwrap();
        crate::migrate::migrate(&mut connection).unwrap();
        prepare(&connection).unwrap();
        let source = DataSource::Alpaca;
        instruments(&mut connection, source, vec![instrument("Facebook")]).unwrap();
        let renamed =
            instruments(&mut connection, source, vec![instrument("Meta Platforms")]).unwrap();
        let unchanged =
            instruments(&mut connection, source, vec![instrument("Meta Platforms")]).unwrap();
        assert_eq!((renamed.rows, unchanged.rows), (1, 0));
        assert_eq!(
            texts(&connection, "SELECT name FROM instrument"),
            ["Meta Platforms"]
        );
        assert_eq!(
            texts(
                &connection,
                "SELECT name || ' until ' || superseded_by FROM instrument_revision"
            ),
            [format!("Facebook until {}", renamed.batch_id)]
        );

        let range = (session(1).date, session(5).date);
        let all = vec![session(1), session(2), session(5)];
        sessions(&mut connection, source, Some(range), all).unwrap();
        let closed = vec![session(1), session(5)];
        sessions(&mut connection, source, Some(range), closed).unwrap();
        assert_eq!(
            texts(
                &connection,
                "SELECT CAST(date AS VARCHAR) FROM calendar ORDER BY date"
            ),
            ["2024-07-01", "2024-07-05"]
        );
        assert_eq!(
            texts(
                &connection,
                "SELECT CAST(date AS VARCHAR) FROM calendar_revision"
            ),
            ["2024-07-02"]
        );
    }

    #[test]
    fn a_split_without_positive_rates_is_refused() {
        let mut connection = Connection::open_in_memory().unwrap();
        crate::migrate::migrate(&mut connection).unwrap();
        prepare(&connection).unwrap();
        let date = session(1).date;
        let split = |old_rate: i64| CorporateAction {
            symbol: Symbol::new("FB"),
            ex_date: date,
            kind: CorporateActionKind::Split {
                old_rate: Decimal::from(old_rate),
                new_rate: Decimal::from(2),
            },
        };
        let window = ActionWindow {
            symbols: vec![Symbol::new("FB")],
            from: date,
            through: date,
            covered_to: Some(date),
        };
        let written = corporate_actions(
            &mut connection,
            DataSource::Alpaca,
            vec![split(0)],
            &window,
            None,
        )
        .unwrap();
        assert_eq!(written.rows, 0);
        assert!(
            AdjustmentFactor::for_shares(&split(0), Adjustment::Split).is_none(),
            "a zero rate must not divide"
        );
    }

    #[test]
    fn daily_bars_appended_out_of_date_order_are_put_back_in_date_order() {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = Connection::open_in_memory().unwrap();
        let path = directory.path().join("market.duckdb");
        connection
            .execute_batch(&format!(
                "ATTACH '{}' AS market (ROW_GROUP_SIZE 2048); USE market",
                path.display()
            ))
            .unwrap();
        crate::migrate::migrate(&mut connection).unwrap();
        prepare(&connection).unwrap();
        let reader = connection.try_clone().unwrap();
        reader.execute_batch("USE market").unwrap();
        let rows_read = |reader: &Connection| -> Vec<String> {
            let mut statement = reader
                .prepare_cached(
                    "SELECT symbol || ' ' || CAST(date AS VARCHAR) || ' ' || CAST(close AS VARCHAR) \
                     FROM bar_1d ORDER BY symbol, date",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<duckdb::Result<_>>()
                .unwrap()
        };
        let bars = |first: NaiveDate, close: i64| -> Vec<Bar> {
            first
                .iter_days()
                .take(300)
                .flat_map(|date| {
                    (0..20).map(move |index| Bar {
                        symbol: Symbol::new(&format!("S{index:02}")),
                        start: Timestamp::at_exchange_time(
                            date,
                            NaiveTime::from_hms_opt(9, 30, 0).unwrap(),
                        )
                        .unwrap(),
                        open: Decimal::from(close),
                        high: Decimal::from(close),
                        low: Decimal::from(close),
                        close: Decimal::from(close),
                        volume: 100,
                        feed: Feed::Sip,
                    })
                })
                .collect()
        };
        let source = DataSource::Alpaca;
        daily_bars(
            &mut connection,
            source,
            bars(NaiveDate::from_ymd_opt(2016, 1, 4).unwrap(), 2),
        )
        .unwrap();
        let recent = rows_read(&reader);
        daily_bars(
            &mut connection,
            source,
            bars(NaiveDate::from_ymd_opt(2000, 1, 3).unwrap(), 1),
        )
        .unwrap();
        let all = rows_read(&reader);
        assert_eq!((recent.len(), all.len()), (6_000, 12_000));
        assert!(scattered_daily_row_groups(&connection).unwrap() > SCATTERED_ROW_GROUPS_TOLERATED);

        assert_eq!(order_daily_bars(&mut connection).unwrap(), Some(12_000));
        assert_eq!(scattered_daily_row_groups(&connection).unwrap(), 0);
        assert_eq!(order_daily_bars(&mut connection).unwrap(), None);
        assert_eq!(rows_read(&reader), all);

        let revised = daily_bars(
            &mut connection,
            source,
            bars(NaiveDate::from_ymd_opt(2016, 1, 4).unwrap(), 3),
        )
        .unwrap();
        assert_eq!(revised.rows, 6_000);
        assert_eq!(rows_read(&reader).len(), 12_000);
    }
}
