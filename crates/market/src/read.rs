use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{
    Adjustment, AdjustmentFactor, AssetClass, Bar, Calendar, Company, CorporateAction,
    CorporateActionKind, Coverage, DataSource, Dataset, EarningsRelease, Feed, FundamentalMetric,
    FundamentalPoint, HISTORY_FLOORS, IndexHistory, IndexMember, Instrument, InstrumentProfile,
    MacroObservation, MacroPoint, MacroSeries, RiskFreeRate, SIP_DELAY, Session, StockIndex,
    Symbol, Timeframe,
};
use buizz_core::ops::{CoverageGap, DataWarning, UNEXPLAINED_MOVE_THRESHOLD};
use buizz_core::protocol::BarRow;
use buizz_core::time::Timestamp;
use buizz_core::visibility::{
    bar_visible_at, corporate_action_visible_at, published_before, published_visible_at,
};
use chrono::NaiveDate;
use duckdb::{Connection, ToSql, params, params_from_iter};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::codec::{
    Dec, Params, Sym, Ts, Wire, action_kind, placeholders, symbol_params, wire_name,
};
use crate::{HistoryRequest, Result};

const MINUTES_PER_SESSION: usize = 390;

fn sip_delay_micros() -> i64 {
    i64::try_from(SIP_DELAY.as_micros()).expect("the SIP delay fits in microseconds")
}

pub(crate) fn calendar(connection: &Connection) -> Result<Calendar> {
    let mut statement =
        connection.prepare_cached("SELECT date, open, close FROM calendar ORDER BY date")?;
    let sessions = statement
        .query_map([], |row| {
            Ok(Session {
                date: row.get(0)?,
                open: row.get::<_, Ts>(1)?.0,
                close: row.get::<_, Ts>(2)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(Calendar::new(sessions))
}

pub(crate) fn instruments(connection: &Connection) -> Result<Vec<Instrument>> {
    let mut statement = connection.prepare_cached(
        "SELECT symbol, name, exchange, asset_class, active, shortable, first_session, \
         last_session FROM instrument ORDER BY symbol",
    )?;
    let instruments = statement
        .query_map([], |row| {
            Ok(Instrument {
                symbol: row.get::<_, Sym>(0)?.0,
                name: row.get(1)?,
                exchange: row.get(2)?,
                asset_class: row.get::<_, Wire<_>>(3)?.0,
                active: row.get(4)?,
                shortable: row.get(5)?,
                first_session: row.get(6)?,
                last_session: row.get(7)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(instruments)
}

pub(crate) struct BarQuery<'a> {
    pub timeframe: Timeframe,
    pub symbols: &'a [Symbol],
    pub from: Option<Timestamp>,
    pub to: Timestamp,
    pub to_inclusive: bool,
    pub latest: Option<usize>,
}

pub(crate) fn bars(connection: &Connection, query: &BarQuery<'_>) -> Result<Vec<Bar>> {
    if query.symbols.is_empty() {
        return Ok(Vec::new());
    }
    let (source, start, feed) = match query.timeframe {
        Timeframe::Minute => ("bar_1m b", "b.start", "b.feed"),
        Timeframe::Day => (
            "bar_1d b JOIN calendar c ON c.date = b.date",
            "c.open",
            "'sip'",
        ),
    };
    let upper = if query.to_inclusive { "<=" } else { "<" };
    let mut sql = format!(
        "SELECT b.symbol, {start}, b.open, b.high, b.low, b.close, b.volume, {feed} \
         FROM {source} WHERE b.symbol IN ({}) AND {start} {upper} ?",
        placeholders(query.symbols.len())
    );
    let mut params = symbol_params(query.symbols);
    params.push(Box::new(Ts(query.to)));
    if let Some(from) = query.from {
        sql.push_str(&format!(" AND {start} >= ?"));
        params.push(Box::new(Ts(from)));
    }
    if query.timeframe == Timeframe::Day {
        sql.push_str(" AND b.date <= ?");
        params.push(Box::new(query.to.exchange_date()));
        if let Some(from) = query.from {
            sql.push_str(" AND b.date >= ?");
            params.push(Box::new(from.exchange_date()));
        }
    }
    if let Some(latest) = query.latest {
        sql.push_str(&format!(
            " QUALIFY row_number() OVER (PARTITION BY b.symbol ORDER BY {start} DESC) <= ?"
        ));
        params.push(Box::new(latest as u64));
    }
    sql.push_str(&format!(" ORDER BY {start}, b.symbol"));
    let mut statement = connection.prepare_cached(&sql)?;
    let bars = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(Bar {
                symbol: row.get::<_, Sym>(0)?.0,
                start: row.get::<_, Ts>(1)?.0,
                open: row.get::<_, Dec>(2)?.0,
                high: row.get::<_, Dec>(3)?.0,
                low: row.get::<_, Dec>(4)?.0,
                close: row.get::<_, Dec>(5)?.0,
                volume: row.get(6)?,
                feed: row.get::<_, Wire<Feed>>(7)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(bars)
}

fn unique(symbols: &[Symbol]) -> Vec<Symbol> {
    symbols
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn is_visible(calendar: &Calendar, timeframe: Timeframe, bar: &Bar, as_of: Timestamp) -> bool {
    calendar
        .containing(bar.start)
        .is_some_and(|session| bar_visible_at(timeframe, bar.start, session) <= as_of)
}

fn visible_bars(
    connection: &Connection,
    calendar: &Calendar,
    timeframe: Timeframe,
    symbols: Vec<Symbol>,
    count: usize,
    as_of: Timestamp,
) -> Result<BTreeMap<Symbol, Vec<Bar>>> {
    let opened = calendar
        .sessions()
        .partition_point(|session| session.open <= as_of);
    let mut lookback = match timeframe {
        Timeframe::Day => count + 1,
        Timeframe::Minute => (count + 1).div_ceil(MINUTES_PER_SESSION) + 1,
    };
    let mut pending = symbols;
    let mut visible = BTreeMap::new();
    while !pending.is_empty() {
        let from = opened
            .checked_sub(lookback)
            .map(|index| calendar.sessions()[index].open);
        let fetched = bars(
            connection,
            &BarQuery {
                timeframe,
                symbols: &pending,
                from,
                to: as_of,
                to_inclusive: true,
                latest: Some(count + 1),
            },
        )?;
        let mut found: BTreeMap<Symbol, Vec<Bar>> = BTreeMap::new();
        for bar in fetched {
            if is_visible(calendar, timeframe, &bar, as_of) {
                found.entry(bar.symbol.clone()).or_default().push(bar);
            }
        }
        pending.retain(|symbol| {
            let enough = found.get(symbol).map_or(0, Vec::len) >= count;
            if enough || from.is_none() {
                if let Some(mut bars) = found.remove(symbol) {
                    bars.drain(..bars.len().saturating_sub(count));
                    visible.insert(symbol.clone(), bars);
                }
                false
            } else {
                true
            }
        });
        lookback *= 2;
    }
    Ok(visible)
}

fn to_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(f64::NAN)
}

pub(crate) fn history(
    connection: &Connection,
    calendar: &Calendar,
    request: &HistoryRequest,
) -> Result<Vec<BarRow>> {
    let count = request.bars as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let visible = visible_bars(
        connection,
        calendar,
        request.timeframe,
        unique(&request.symbols),
        count,
        request.as_of,
    )?;
    let earliest = visible
        .values()
        .filter_map(|bars| bars.first())
        .map(|bar| bar.start.exchange_date())
        .min();
    let Some(earliest) = earliest else {
        return Ok(Vec::new());
    };
    let symbols: Vec<Symbol> = visible.keys().cloned().collect();
    let factors = adjustment_factors(
        connection,
        calendar,
        &symbols,
        request.adjust,
        earliest,
        request.as_of,
    )?;
    let mut rows: Vec<BarRow> = visible
        .into_values()
        .flatten()
        .map(|bar| {
            let symbol_factors = factors.get(&bar.symbol).map_or(&[][..], Vec::as_slice);
            let (price, volume) =
                AdjustmentFactor::cumulative(bar.start.exchange_date(), symbol_factors);
            BarRow {
                start: bar.start,
                open: to_f64(bar.open * price),
                high: to_f64(bar.high * price),
                low: to_f64(bar.low * price),
                close: to_f64(bar.close * price),
                volume: to_f64(Decimal::from(bar.volume) * volume),
                symbol: bar.symbol,
            }
        })
        .collect();
    rows.sort_by(|left, right| (left.start, &left.symbol).cmp(&(right.start, &right.symbol)));
    Ok(rows)
}

fn adjustment_factors(
    connection: &Connection,
    calendar: &Calendar,
    symbols: &[Symbol],
    adjustment: Adjustment,
    earliest_bar: NaiveDate,
    as_of: Timestamp,
) -> Result<BTreeMap<Symbol, Vec<AdjustmentFactor>>> {
    let Some(after) = earliest_bar.succ_opt() else {
        return Ok(BTreeMap::new());
    };
    if adjustment == Adjustment::Raw || after > as_of.exchange_date() {
        return Ok(BTreeMap::new());
    }
    let actions: Vec<CorporateAction> =
        corporate_actions(connection, symbols, after, as_of.exchange_date())?
            .into_iter()
            .filter(|action| {
                corporate_action_visible_at(action.ex_date, calendar)
                    .is_some_and(|visible_at| visible_at <= as_of)
            })
            .collect();
    let closes = if adjustment == Adjustment::All {
        previous_closes(connection, calendar, &actions)?
    } else {
        BTreeMap::new()
    };
    let mut factors: BTreeMap<Symbol, Vec<AdjustmentFactor>> = BTreeMap::new();
    for action in &actions {
        let previous_close = closes
            .get(&(action.symbol.clone(), action.ex_date))
            .copied();
        if let Some(factor) = AdjustmentFactor::for_action(action, adjustment, previous_close) {
            factors
                .entry(action.symbol.clone())
                .or_default()
                .push(factor);
        }
    }
    Ok(factors)
}

fn previous_closes(
    connection: &Connection,
    calendar: &Calendar,
    actions: &[CorporateAction],
) -> Result<BTreeMap<(Symbol, NaiveDate), Decimal>> {
    let wanted: BTreeMap<(Symbol, NaiveDate), NaiveDate> = actions
        .iter()
        .filter(|action| matches!(action.kind, CorporateActionKind::CashDividend { .. }))
        .filter_map(|action| {
            calendar
                .previous_before(action.ex_date)
                .map(|session| ((action.symbol.clone(), session.date), action.ex_date))
        })
        .collect();
    if wanted.is_empty() {
        return Ok(BTreeMap::new());
    }
    let sql = format!(
        "SELECT symbol, date, close FROM bar_1d WHERE {}",
        vec!["(symbol = ? AND date = ?)"; wanted.len()].join(" OR ")
    );
    let params: Params = wanted
        .keys()
        .flat_map(|(symbol, date)| {
            [
                Box::new(symbol.as_str().to_owned()) as Box<dyn ToSql>,
                Box::new(*date),
            ]
        })
        .collect();
    let mut statement = connection.prepare_cached(&sql)?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((
                row.get::<_, Sym>(0)?.0,
                row.get::<_, NaiveDate>(1)?,
                row.get::<_, Dec>(2)?.0,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(symbol, date, close)| {
            let ex_date = wanted.get(&(symbol.clone(), date))?;
            Some(((symbol, *ex_date), close))
        })
        .collect())
}

pub(crate) fn corporate_actions(
    connection: &Connection,
    symbols: &[Symbol],
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<CorporateAction>> {
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT symbol, ex_date, kind, old_rate, new_rate, rate FROM corporate_action \
         WHERE symbol IN ({}) AND ex_date BETWEEN ? AND ? ORDER BY symbol, ex_date, kind",
        placeholders(symbols.len())
    );
    let mut params = symbol_params(symbols);
    params.push(Box::new(from));
    params.push(Box::new(to));
    let mut statement = connection.prepare_cached(&sql)?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((
                row.get::<_, Sym>(0)?.0,
                row.get::<_, NaiveDate>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<Dec>>(3)?.map(|rate| rate.0),
                row.get::<_, Option<Dec>>(4)?.map(|rate| rate.0),
                row.get::<_, Option<Dec>>(5)?.map(|rate| rate.0),
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(symbol, ex_date, kind, old_rate, new_rate, rate)| {
            action_kind(&kind, old_rate, new_rate, rate).map(|kind| CorporateAction {
                symbol,
                ex_date,
                kind,
            })
        })
        .collect())
}

pub(crate) fn fundamentals(
    connection: &Connection,
    symbols: &[Symbol],
    metrics: &[FundamentalMetric],
    as_of: Timestamp,
) -> Result<Vec<FundamentalPoint>> {
    let symbols = unique(symbols);
    let metrics: Vec<String> = metrics
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(crate::codec::wire_name)
        .collect();
    if symbols.is_empty() || metrics.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT symbol, metric, period_type, period_start, period_end, value, unit, form, \
         filed, available_at FROM fundamental_fact WHERE symbol IN ({}) AND metric IN ({}) \
         AND period_type IS NOT NULL AND available_at <= ? \
         QUALIFY row_number() OVER (PARTITION BY symbol, metric, period_end, period_type, \
         period_start ORDER BY available_at DESC, filed DESC, accession DESC) = 1",
        placeholders(symbols.len()),
        placeholders(metrics.len())
    );
    let mut params = symbol_params(&symbols);
    params.extend(
        metrics
            .into_iter()
            .map(|metric| Box::new(metric) as Box<dyn ToSql>),
    );
    params.push(Box::new(Ts(as_of)));
    let mut statement = connection.prepare_cached(&sql)?;
    let mut points = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(FundamentalPoint {
                symbol: row.get::<_, Sym>(0)?.0,
                metric: row.get::<_, Wire<_>>(1)?.0,
                period_type: row.get::<_, Wire<_>>(2)?.0,
                period_start: row.get(3)?,
                period_end: row.get(4)?,
                value: row.get::<_, Dec>(5)?.0,
                unit: row.get(6)?,
                form: row.get(7)?,
                filed: row.get(8)?,
                available_at: row.get::<_, Ts>(9)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    points.sort_by(|a, b| {
        (
            &a.symbol,
            a.metric,
            a.period_end,
            a.period_type,
            a.period_start,
        )
            .cmp(&(
                &b.symbol,
                b.metric,
                b.period_end,
                b.period_type,
                b.period_start,
            ))
    });
    Ok(points)
}

fn sessions_before(calendar: &Calendar, before: NaiveDate, count: u32) -> &[Session] {
    let sessions = calendar.sessions();
    let end = sessions.partition_point(|session| session.date < before);
    &sessions[end.saturating_sub(count as usize)..end]
}

pub(crate) fn average_daily_volume(
    connection: &Connection,
    calendar: &Calendar,
    symbols: &[Symbol],
    before: NaiveDate,
    sessions: u32,
) -> Result<BTreeMap<Symbol, u64>> {
    let symbols = unique(symbols);
    let Some(first) = sessions_before(calendar, before, sessions).first() else {
        return Ok(BTreeMap::new());
    };
    let as_of = calendar
        .first_on_or_after(before)
        .map_or(Timestamp::MAX, |session| session.open);
    let daily = bars(
        connection,
        &BarQuery {
            timeframe: Timeframe::Day,
            symbols: &symbols,
            from: Some(first.open),
            to: as_of,
            to_inclusive: false,
            latest: None,
        },
    )?;
    let factors = adjustment_factors(
        connection,
        calendar,
        &symbols,
        Adjustment::Split,
        first.date,
        as_of,
    )?;
    let mut totals: BTreeMap<Symbol, (Decimal, u32)> = BTreeMap::new();
    for bar in daily {
        let symbol_factors = factors.get(&bar.symbol).map_or(&[][..], Vec::as_slice);
        let (_, volume) = AdjustmentFactor::cumulative(bar.start.exchange_date(), symbol_factors);
        let total = totals.entry(bar.symbol).or_default();
        total.0 += Decimal::from(bar.volume) * volume;
        total.1 += 1;
    }
    Ok(totals
        .into_iter()
        .filter_map(|(symbol, (sum, days))| {
            (sum / Decimal::from(days))
                .round()
                .to_u64()
                .map(|average| (symbol, average))
        })
        .collect())
}

pub(crate) fn top_dollar_volume(
    connection: &Connection,
    calendar: &Calendar,
    count: u32,
    asset_class: Option<AssetClass>,
    before: NaiveDate,
    sessions: u32,
) -> Result<Vec<Symbol>> {
    let Some(first) = sessions_before(calendar, before, sessions).first() else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare_cached(
        "SELECT b.symbol FROM bar_1d b LEFT JOIN instrument i ON i.symbol = b.symbol \
         WHERE b.date >= ? AND b.date < ? AND (? IS NULL OR i.asset_class = ?) \
         AND (i.last_session IS NULL OR i.last_session >= ?) \
         GROUP BY b.symbol ORDER BY avg(CAST(b.close AS DOUBLE) * b.volume) DESC, b.symbol \
         LIMIT ?",
    )?;
    let class = asset_class.as_ref().map(crate::codec::wire_name);
    let symbols = statement
        .query_map(
            params![first.date, before, class, class, before, u64::from(count)],
            |row| Ok(row.get::<_, Sym>(0)?.0),
        )?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(symbols)
}

pub(crate) fn index_universe(
    connection: &Connection,
    calendar: &Calendar,
    index: StockIndex,
    before: NaiveDate,
    sessions: u32,
) -> Result<Vec<Symbol>> {
    let Some(first) = sessions_before(calendar, before, sessions).first() else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare_cached(
        "SELECT DISTINCT m.symbol FROM index_member m \
         WHERE m.index_name = ? AND (m.start_date IS NULL OR m.start_date <= ?) \
         AND (m.end_date IS NULL OR ? < m.end_date) \
         AND EXISTS (SELECT 1 FROM bar_1d b WHERE b.symbol = m.symbol \
         AND b.date >= ? AND b.date < ?) ORDER BY m.symbol",
    )?;
    let symbols = statement
        .query_map(
            params![
                crate::codec::wire_name(&index),
                before,
                before,
                first.date,
                before
            ],
            |row| Ok(row.get::<_, Sym>(0)?.0),
        )?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(symbols)
}

pub(crate) fn index_history(
    connection: &Connection,
    index: StockIndex,
) -> Result<Option<IndexHistory>> {
    let name = crate::codec::wire_name(&index);
    let mut statement = connection
        .prepare_cached("SELECT changes, reliable_from FROM index_history WHERE index_name = ?")?;
    let mut rows = statement.query(params![name])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let changes: u32 = row.get(0)?;
    let reliable_from: Option<NaiveDate> = row.get(1)?;
    let mut statement = connection.prepare_cached(
        "SELECT symbol, start_date, end_date FROM index_member WHERE index_name = ? \
         ORDER BY symbol, start_date NULLS FIRST",
    )?;
    let members = statement
        .query_map(params![name], |row| {
            Ok(IndexMember {
                index,
                symbol: row.get::<_, Sym>(0)?.0,
                start: row.get(1)?,
                end: row.get(2)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(Some(IndexHistory {
        index,
        members,
        changes,
        reliable_from,
    }))
}

pub(crate) fn companies(connection: &Connection) -> Result<Vec<Company>> {
    let mut statement = connection
        .prepare_cached("SELECT symbol, cik, sic, sic_description FROM company ORDER BY symbol")?;
    let companies = statement
        .query_map([], |row| {
            Ok(Company {
                symbol: row.get::<_, Sym>(0)?.0,
                cik: row.get(1)?,
                sic: row.get(2)?,
                sic_description: row.get(3)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(companies)
}

pub(crate) fn instrument_profiles(
    connection: &Connection,
    symbols: &[Symbol],
) -> Result<Vec<InstrumentProfile>> {
    let symbols = unique(symbols);
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT i.symbol, i.name, i.exchange, i.asset_class, c.sic, c.sic_description \
         FROM instrument i LEFT JOIN company c ON c.symbol = i.symbol \
         WHERE i.symbol IN ({}) ORDER BY i.symbol",
        placeholders(symbols.len())
    );
    let params = symbol_params(&symbols);
    let mut statement = connection.prepare_cached(&sql)?;
    let profiles = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(InstrumentProfile {
                symbol: row.get::<_, Sym>(0)?.0,
                name: row.get(1)?,
                exchange: row.get(2)?,
                asset_class: row.get::<_, Wire<_>>(3)?.0,
                sic: row.get(4)?,
                sic_description: row.get(5)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(profiles)
}

pub(crate) fn earnings_releases(
    connection: &Connection,
    symbols: &[Symbol],
    as_of: Timestamp,
) -> Result<Vec<EarningsRelease>> {
    let symbols = unique(symbols);
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT symbol, cik, accession, form, filed, report_date, accepted_at \
         FROM earnings_release e WHERE symbol IN ({}) AND accepted_at <= ? \
         AND NOT (form LIKE '%/A' AND EXISTS (SELECT 1 FROM earnings_release o \
         WHERE o.symbol = e.symbol AND o.report_date = e.report_date \
         AND o.accepted_at < e.accepted_at)) \
         ORDER BY symbol, accepted_at, accession",
        placeholders(symbols.len())
    );
    let mut params = symbol_params(&symbols);
    params.push(Box::new(Ts(as_of)));
    let mut statement = connection.prepare_cached(&sql)?;
    let releases = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(EarningsRelease {
                symbol: row.get::<_, Sym>(0)?.0,
                cik: row.get(1)?,
                accession: row.get(2)?,
                form: row.get(3)?,
                filed: row.get(4)?,
                report_date: row.get(5)?,
                accepted_at: row.get::<_, Ts>(6)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(releases)
}

pub(crate) fn earnings_accessions(
    connection: &Connection,
    symbol: &Symbol,
) -> Result<BTreeSet<String>> {
    let mut statement =
        connection.prepare_cached("SELECT accession FROM earnings_release WHERE symbol = ?")?;
    let accessions = statement
        .query_map([symbol.as_str()], |row| row.get(0))?
        .collect::<duckdb::Result<_>>()?;
    Ok(accessions)
}

pub(crate) fn latest_sec_filings(
    connection: &Connection,
    forms: &[&str],
) -> Result<BTreeMap<u32, NaiveDate>> {
    if forms.is_empty() {
        return Ok(BTreeMap::new());
    }
    let sql = format!(
        "SELECT cik, max(filed) FROM sec_filing WHERE form IN ({}) GROUP BY cik",
        placeholders(forms.len())
    );
    let mut statement = connection.prepare_cached(&sql)?;
    let latest = statement
        .query_map(params_from_iter(forms.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok(latest)
}

pub(crate) fn daily_watermark(connection: &Connection) -> Result<Option<Timestamp>> {
    let mut statement = connection.prepare_cached("SELECT max(through) FROM daily_watermark")?;
    let through: Option<Ts> = statement.query_row([], |row| row.get(0))?;
    Ok(through.map(|at| at.0))
}

pub(crate) fn latest_borrow(connection: &Connection) -> Result<Option<Timestamp>> {
    let mut statement =
        connection.prepare_cached("SELECT max(recorded_at) FROM borrow_availability")?;
    let latest: Option<Ts> = statement.query_row([], |row| row.get(0))?;
    Ok(latest.map(|at| at.0))
}

pub(crate) fn risk_free_rates(
    connection: &Connection,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<RiskFreeRate>> {
    let mut statement = connection.prepare_cached(
        "SELECT date, percent FROM rate_daily WHERE date BETWEEN ? AND ? ORDER BY date",
    )?;
    let rates = statement
        .query_map(params![from, to], |row| {
            Ok(RiskFreeRate {
                date: row.get(0)?,
                percent: row.get::<_, Dec>(1)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(rates)
}

pub(crate) fn macro_observations(
    connection: &Connection,
    series: MacroSeries,
) -> Result<Vec<MacroObservation>> {
    let mut statement = connection.prepare_cached(
        "SELECT date, published, value FROM macro_observation WHERE series = ? \
         ORDER BY date, published",
    )?;
    let observations = statement
        .query_map(params![wire_name(&series)], |row| {
            Ok(MacroObservation {
                series,
                date: row.get(0)?,
                published: row.get(1)?,
                value: row.get::<_, Option<Dec>>(2)?.map(|value| value.0),
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(observations)
}

pub(crate) fn macro_points(
    connection: &Connection,
    calendar: &Calendar,
    series: &[MacroSeries],
    from: NaiveDate,
    to: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<MacroPoint>> {
    let Some(cutoff) = published_before(as_of, calendar) else {
        return Ok(Vec::new());
    };
    let mut known: Vec<(MacroSeries, NaiveDate, NaiveDate, Decimal)> = Vec::new();
    let wanted: BTreeSet<MacroSeries> = series.iter().copied().collect();
    if wanted.contains(&MacroSeries::Treasury3m) {
        known.extend(
            risk_free_rates(connection, from, to)?
                .into_iter()
                .filter(|rate| rate.date < cutoff)
                .map(|rate| (MacroSeries::Treasury3m, rate.date, rate.date, rate.percent)),
        );
    }
    let mut statement = connection.prepare_cached(
        "SELECT date, published, value FROM (SELECT date, published, value, \
         row_number() OVER (PARTITION BY date ORDER BY published DESC) AS vintage \
         FROM macro_observation WHERE series = ? AND date BETWEEN ? AND ? AND published < ?) \
         WHERE vintage = 1 AND value IS NOT NULL ORDER BY date",
    )?;
    for id in wanted.iter().filter(|id| id.fred_id().is_some()) {
        let rows = statement
            .query_map(params![wire_name(id), from, to, cutoff], |row| {
                Ok((*id, row.get(0)?, row.get(1)?, row.get::<_, Dec>(2)?.0))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        known.extend(rows);
    }
    Ok(known
        .into_iter()
        .filter_map(|(series, date, published, value)| {
            Some(MacroPoint {
                series,
                date,
                value,
                published,
                available_at: published_visible_at(published, calendar)?,
            })
        })
        .collect())
}

pub(crate) fn coverage(connection: &Connection) -> Result<Vec<Coverage>> {
    let mut statement = connection.prepare_cached(
        "SELECT dataset, symbol, source, first, last, updated_at FROM coverage \
         ORDER BY dataset, symbol, source",
    )?;
    let coverage = statement
        .query_map([], |row| {
            let symbol: String = row.get(1)?;
            Ok(Coverage {
                dataset: row.get::<_, Wire<Dataset>>(0)?.0,
                symbol: (!symbol.is_empty()).then(|| Symbol::new(&symbol)),
                source: row.get::<_, Wire<DataSource>>(2)?.0,
                first: row.get(3)?,
                last: row.get(4)?,
                updated_at: row.get::<_, Ts>(5)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(coverage)
}

type Listing = (Option<NaiveDate>, Option<NaiveDate>);

fn listing_bounds(
    connection: &Connection,
    calendar: &Calendar,
    symbols: &[Symbol],
) -> Result<BTreeMap<Symbol, Listing>> {
    let floors: Vec<NaiveDate> = HISTORY_FLOORS
        .iter()
        .filter_map(|floor| {
            calendar
                .first_on_or_after(*floor)
                .map(|session| session.date)
        })
        .collect();
    let sql = format!(
        "WITH wanted (symbol) AS (VALUES {}), \
         first_bar AS (SELECT w.symbol, (SELECT min(d.date) FROM bar_1d d \
         WHERE d.symbol = w.symbol) AS first FROM wanted w) \
         SELECT f.symbol, coalesce(i.first_session, \
         CASE WHEN f.first IN ({}) THEN NULL ELSE f.first END), i.last_session \
         FROM first_bar f LEFT JOIN instrument i ON i.symbol = f.symbol",
        vec!["(?)"; symbols.len()].join(", "),
        placeholders(floors.len().max(1))
    );
    let mut params = symbol_params(symbols);
    params.extend(
        floors
            .iter()
            .map(|floor| Box::new(*floor) as Box<dyn ToSql>),
    );
    if floors.is_empty() {
        params.push(Box::new(NaiveDate::MIN));
    }
    let mut statement = connection.prepare_cached(&sql)?;
    let bounds = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((row.get::<_, Sym>(0)?.0, (row.get(1)?, row.get(2)?)))
        })?
        .collect::<duckdb::Result<BTreeMap<_, _>>>()?;
    Ok(bounds)
}

pub(crate) fn coverage_gaps(
    connection: &Connection,
    calendar: &Calendar,
    symbols: &[Symbol],
    timeframe: Timeframe,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<CoverageGap>> {
    let symbols = unique(symbols);
    let sessions = calendar.between(from, to);
    let (Some(first), Some(last)) = (sessions.first(), sessions.last()) else {
        return Ok(Vec::new());
    };
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let mut params = symbol_params(&symbols);
    let sql = match timeframe {
        Timeframe::Day => {
            params.push(Box::new(first.date));
            params.push(Box::new(last.date));
            format!(
                "SELECT DISTINCT symbol, date FROM bar_1d \
                 WHERE symbol IN ({}) AND date BETWEEN ? AND ?",
                placeholders(symbols.len())
            )
        }
        Timeframe::Minute => {
            params.push(Box::new(Ts(first.open)));
            params.push(Box::new(Ts(last.close)));
            params.push(Box::new(first.date));
            params.push(Box::new(last.date));
            params.push(Box::new(sip_delay_micros()));
            format!(
                "SELECT DISTINCT b.symbol, c.date FROM bar_1m b \
                 JOIN calendar c ON b.start >= c.open AND b.start < c.close \
                 JOIN ingest_batch i ON i.id = b.batch_id \
                 WHERE b.symbol IN ({}) AND b.start >= ? AND b.start < ? \
                 AND c.date BETWEEN ? AND ? AND b.feed = 'sip' \
                 AND i.created_at >= c.close + to_microseconds(?)",
                placeholders(symbols.len())
            )
        }
    };
    let mut statement = connection.prepare_cached(&sql)?;
    let present = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((row.get::<_, Sym>(0)?.0, row.get::<_, NaiveDate>(1)?))
        })?
        .collect::<duckdb::Result<BTreeSet<_>>>()?;
    let bounds = listing_bounds(connection, calendar, &symbols)?;
    let mut gaps = Vec::new();
    for symbol in symbols {
        let (listed, delisted) = bounds.get(&symbol).copied().unwrap_or_default();
        let mut open_gap: Option<(NaiveDate, NaiveDate)> = None;
        let listed_sessions = sessions.iter().filter(|session| {
            listed.is_none_or(|listed| listed <= session.date)
                && delisted.is_none_or(|delisted| session.date <= delisted)
        });
        for session in listed_sessions {
            if present.contains(&(symbol.clone(), session.date)) {
                if let Some((gap_from, gap_to)) = open_gap.take() {
                    gaps.push(CoverageGap {
                        symbol: symbol.clone(),
                        timeframe,
                        from: gap_from,
                        to: gap_to,
                    });
                }
            } else {
                open_gap = Some(
                    open_gap.map_or((session.date, session.date), |(gap_from, _)| {
                        (gap_from, session.date)
                    }),
                );
            }
        }
        if let Some((gap_from, gap_to)) = open_gap {
            gaps.push(CoverageGap {
                symbol,
                timeframe,
                from: gap_from,
                to: gap_to,
            });
        }
    }
    Ok(gaps)
}

pub(crate) fn unexplained_moves(
    connection: &Connection,
    calendar: &Calendar,
    symbols: &[Symbol],
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<DataWarning>> {
    let symbols = unique(symbols);
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let start = calendar
        .previous_before(from)
        .map_or(from, |session| session.date);
    let sql = format!(
        "SELECT symbol, date, open, close FROM bar_1d WHERE symbol IN ({}) \
         AND date BETWEEN ? AND ? ORDER BY symbol, date",
        placeholders(symbols.len())
    );
    let mut params = symbol_params(&symbols);
    params.push(Box::new(start));
    params.push(Box::new(to));
    let mut statement = connection.prepare_cached(&sql)?;
    let daily = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((
                row.get::<_, Sym>(0)?.0,
                row.get::<_, NaiveDate>(1)?,
                row.get::<_, Dec>(2)?.0,
                row.get::<_, Dec>(3)?.0,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let explained: BTreeSet<(Symbol, NaiveDate)> =
        corporate_actions(connection, &symbols, from, to)?
            .into_iter()
            .map(|action| (action.symbol, action.ex_date))
            .collect();
    Ok(daily
        .windows(2)
        .filter_map(|pair| {
            let [
                (previous_symbol, _, _, previous_close),
                (symbol, date, open, _),
            ] = pair
            else {
                return None;
            };
            if previous_symbol != symbol || *date < from || previous_close.is_zero() {
                return None;
            }
            let change = to_f64(*open / *previous_close - Decimal::ONE);
            (change.abs() > UNEXPLAINED_MOVE_THRESHOLD
                && !explained.contains(&(symbol.clone(), *date)))
            .then(|| DataWarning::UnexplainedMove {
                symbol: symbol.clone(),
                date: *date,
                change,
            })
        })
        .collect())
}
