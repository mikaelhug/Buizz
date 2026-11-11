use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{
    Bar, BorrowAvailability, Company, CorporateAction, DataSource, Dataset, EarningsRelease,
    FundamentalFact, IndexHistory, Instrument, MacroObservation, RiskFreeRate, Session, Symbol,
};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use duckdb::{Appender, Connection, Transaction, params};

use crate::codec::{Dec, Ts, action_columns, feed_name, wire_name};
use crate::{Result, WriteSummary};

const STAGED: [&str; 9] = [
    "instrument",
    "calendar",
    "bar_1d",
    "corporate_action",
    "fundamental_fact",
    "rate_daily",
    "macro_observation",
    "company",
    "earnings_release",
];

pub(crate) fn prepare(connection: &Connection) -> Result<()> {
    for table in STAGED {
        connection.execute_batch(&format!(
            "CREATE TEMP TABLE stage_{table} AS SELECT * FROM main.{table} LIMIT 0"
        ))?;
    }
    connection
        .execute_batch("CREATE TEMP TABLE bar_span (symbol VARCHAR, lo TIMESTAMP, hi TIMESTAMP)")?;
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
    let mut upsert = transaction.prepare(
        "INSERT INTO coverage VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (dataset, symbol, source) \
         DO UPDATE SET first = least(coverage.first, excluded.first), \
         last = greatest(coverage.last, excluded.last), updated_at = excluded.updated_at",
    )?;
    for (symbol, (first, last)) in coverage {
        let symbol = symbol.as_ref().map_or("", Symbol::as_str);
        upsert.execute(params![dataset, symbol, source, first, last, now])?;
    }
    drop(upsert);
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

const KEYS: [(&str, &[&str]); 8] = [
    ("instrument", &["symbol"]),
    ("calendar", &["date"]),
    ("bar_1d", &["symbol", "date"]),
    ("corporate_action", &["symbol", "ex_date", "kind"]),
    ("rate_daily", &["date"]),
    ("macro_observation", &["series", "date", "published"]),
    ("company", &["symbol"]),
    ("earnings_release", &["symbol", "accession"]),
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
    covered: &[Symbol],
    (from, to): (NaiveDate, NaiveDate),
) -> Result<WriteSummary> {
    let unique: BTreeMap<(Symbol, NaiveDate, &'static str), CorporateAction> = actions
        .into_iter()
        .map(|action| {
            let kind = action_columns(&action.kind).kind;
            ((action.symbol.clone(), action.ex_date, kind), action)
        })
        .collect();
    let mut coverage = Coverage::new();
    for symbol in covered {
        cover(&mut coverage, Some(symbol), from);
        cover(&mut coverage, Some(symbol), to);
    }
    let source_name = wire_name(&source);
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
                |appender, action| {
                    let columns = action_columns(&action.kind);
                    appender.append_row(params![
                        action.symbol.as_str(),
                        action.ex_date,
                        columns.kind,
                        columns.old_rate,
                        columns.new_rate,
                        columns.rate,
                        source_name,
                        batch_id,
                    ])
                },
            )?;
            replace_staged(transaction, "corporate_action", batch_id)
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
            let inserted = transaction.execute(
                "INSERT INTO main.fundamental_fact SELECT s.* FROM stage_fundamental_fact s \
                 WHERE NOT EXISTS (SELECT 1 FROM main.fundamental_fact f \
                 WHERE f.symbol = s.symbol AND f.cik = s.cik AND f.taxonomy = s.taxonomy \
                 AND f.concept = s.concept AND f.unit = s.unit \
                 AND f.period_start IS NOT DISTINCT FROM s.period_start \
                 AND f.period_end = s.period_end AND f.accession = s.accession)",
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

#[cfg(test)]
mod tests {
    use buizz_core::market::AssetClass;
    use chrono::NaiveTime;

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
}
