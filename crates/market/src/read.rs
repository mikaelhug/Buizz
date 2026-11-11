use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{
    Adjustment, AdjustmentFactor, Bar, Calendar, CorporateAction, CorporateActionKind, Coverage,
    DataSource, Dataset, Feed, FundamentalMetric, FundamentalPoint, Instrument, PeriodType,
    RiskFreeRate, Session, Symbol, Timeframe,
};
use buizz_core::ops::{CoverageGap, DataWarning, UNEXPLAINED_MOVE_THRESHOLD};
use buizz_core::protocol::BarRow;
use buizz_core::time::Timestamp;
use buizz_core::visibility::{bar_visible_at, corporate_action_visible_at, fundamental_visible_at};
use chrono::NaiveDate;
use duckdb::{Connection, ToSql, params, params_from_iter};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::codec::{Dec, Params, Sym, Ts, Wire, action_kind, placeholders, symbol_params};
use crate::{HistoryRequest, Result};

const MINUTES_PER_SESSION: usize = 390;

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

type PeriodKey = (
    Symbol,
    FundamentalMetric,
    NaiveDate,
    PeriodType,
    Option<NaiveDate>,
);

pub(crate) fn fundamentals(
    connection: &Connection,
    calendar: &Calendar,
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
        "SELECT symbol, metric, period_type, period_start, period_end, value, form, accession, \
         filed FROM fundamental_fact WHERE symbol IN ({}) AND metric IN ({}) \
         AND period_type IS NOT NULL AND filed < ?",
        placeholders(symbols.len()),
        placeholders(metrics.len())
    );
    let mut params = symbol_params(&symbols);
    params.extend(
        metrics
            .into_iter()
            .map(|metric| Box::new(metric) as Box<dyn ToSql>),
    );
    params.push(Box::new(as_of.exchange_date()));
    let mut statement = connection.prepare_cached(&sql)?;
    let facts = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((
                FundamentalPoint {
                    symbol: row.get::<_, Sym>(0)?.0,
                    metric: row.get::<_, Wire<_>>(1)?.0,
                    period_type: row.get::<_, Wire<_>>(2)?.0,
                    period_start: row.get(3)?,
                    period_end: row.get(4)?,
                    value: row.get::<_, Dec>(5)?.0,
                    form: row.get(6)?,
                    filed: row.get(8)?,
                    available_at: Timestamp::MAX,
                },
                row.get::<_, String>(7)?,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let mut latest: BTreeMap<PeriodKey, (FundamentalPoint, String)> = BTreeMap::new();
    for (mut point, accession) in facts {
        let Some(visible_at) = fundamental_visible_at(point.filed, calendar) else {
            continue;
        };
        if visible_at > as_of {
            continue;
        }
        point.available_at = visible_at;
        let key = (
            point.symbol.clone(),
            point.metric,
            point.period_end,
            point.period_type,
            point.period_start,
        );
        let newer = latest.get(&key).is_none_or(|(current, current_accession)| {
            (point.available_at, point.filed, &accession)
                > (current.available_at, current.filed, current_accession)
        });
        if newer {
            latest.insert(key, (point, accession));
        }
    }
    Ok(latest.into_values().map(|(point, _)| point).collect())
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
    before: NaiveDate,
    sessions: u32,
) -> Result<Vec<Symbol>> {
    let Some(first) = sessions_before(calendar, before, sessions).first() else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare_cached(
        "SELECT symbol FROM bar_1d WHERE date >= ? AND date < ? GROUP BY symbol \
         ORDER BY sum(CAST(close AS DOUBLE) * volume) DESC, symbol LIMIT ?",
    )?;
    let symbols = statement
        .query_map(params![first.date, before, u64::from(count)], |row| {
            Ok(row.get::<_, Sym>(0)?.0)
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(symbols)
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
    symbols: &[Symbol],
) -> Result<BTreeMap<Symbol, Listing>> {
    let sql = format!(
        "SELECT symbol, first_session, last_session FROM instrument WHERE symbol IN ({})",
        placeholders(symbols.len())
    );
    let params = symbol_params(symbols);
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
            format!(
                "SELECT DISTINCT b.symbol, c.date FROM bar_1m b \
                 JOIN calendar c ON b.start >= c.open AND b.start < c.close \
                 WHERE b.symbol IN ({}) AND b.start >= ? AND b.start < ? \
                 AND c.date BETWEEN ? AND ?",
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
    let bounds = listing_bounds(connection, &symbols)?;
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
