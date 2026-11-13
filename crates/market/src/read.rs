use std::collections::{BTreeMap, BTreeSet};

use buizz_core::listing::Listings;
use buizz_core::market::{
    Adjustment, AdjustmentFactor, AssetClass, Bar, Borrow, Calendar, Company, CorporateAction,
    CorporateActionKind, Coverage, DataSource, Dataset, EarningsInsight, EarningsRelease,
    EstimateFact, EstimateMetric, EstimateScope, Filing, FomcAnnouncement, FomcMeeting,
    FundamentalMetric, FundamentalPoint, HISTORY_FLOORS, HISTORY_START, IndexHistory, IndexMember,
    InsiderOwner, InsiderPlan, InsiderTrade, InsiderTransaction, Instrument, InstrumentProfile,
    Liquidity, MacroObservation, MacroPoint, MacroSeries, NewsArticle, PressCandidate,
    PressRelease, RiskFreeRate, SIP_DELAY, Session, ShortInterest, ShortInterestRecord,
    ShortVolume, StockIndex, Symbol, Timeframe,
};
use buizz_core::ops::{ActionDisagreement, CoverageGap, DataWarning, UNEXPLAINED_MOVE_THRESHOLD};
use buizz_core::protocol::BarRow;
use buizz_core::time::Timestamp;
use buizz_core::visibility::{
    bar_visible_at, corporate_action_visible_at, filing_visible_at, fomc_visible_at,
    published_before, published_visible_at,
};
use chrono::{NaiveDate, NaiveTime};
use duckdb::types::Type;
use duckdb::{Connection, ToSql, params, params_from_iter};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::codec::{
    ActionColumns, Dec, FeedColumn, IN_SYMBOLS, Params, Sym, Ts, Wire, placeholders, symbol_list,
    text_list, wire_name,
};
use crate::{ChartBars, ChartRequest, HistoryRequest, Result};

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

pub(crate) fn listings(connection: &Connection) -> Result<Listings> {
    let renames = select_actions(connection, "kind = 'name_change'", Params::new())?;
    let successions = select_actions(
        connection,
        "kind = 'stock_merger' AND NOT EXISTS (SELECT 1 FROM bar_1d b \
         WHERE b.symbol = other_symbol AND b.date < ex_date - INTERVAL 30 DAY)",
        Params::new(),
    )?;
    let mut statement =
        connection.prepare_cached("SELECT symbol FROM instrument WHERE contains(symbol, '@')")?;
    let keys = statement
        .query_map([], |row| Ok(row.get::<_, Sym>(0)?.0))?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(Listings::new(&renames, &successions, &keys))
}

pub(crate) fn listed_on(calendar: &Calendar, at: Timestamp) -> NaiveDate {
    let opened = calendar
        .sessions()
        .partition_point(|session| session.open <= at);
    opened.checked_sub(1).map_or_else(
        || at.exchange_date(),
        |index| calendar.sessions()[index].date,
    )
}

pub(crate) type Resolved = BTreeMap<Symbol, Symbol>;

pub(crate) fn resolve(listings: &Listings, symbols: &[Symbol], on: NaiveDate) -> Resolved {
    symbols
        .iter()
        .filter_map(|ticker| Some((ticker.clone(), listings.listing(ticker, on)?)))
        .collect()
}

pub(crate) fn keys(resolved: &Resolved) -> Vec<Symbol> {
    unique(&resolved.values().cloned().collect::<Vec<_>>())
}

pub(crate) fn relabel<T: Clone>(
    resolved: &Resolved,
    rows: Vec<T>,
    symbol: impl Fn(&mut T) -> &mut Symbol,
) -> Vec<T> {
    let mut tickers: BTreeMap<&Symbol, Vec<&Symbol>> = BTreeMap::new();
    for (ticker, key) in resolved {
        tickers.entry(key).or_default().push(ticker);
    }
    let mut relabeled: Vec<(Symbol, T)> = Vec::new();
    for mut row in rows {
        let Some(asked) = tickers.get(symbol(&mut row)) else {
            continue;
        };
        for ticker in asked {
            let mut copy = row.clone();
            *symbol(&mut copy) = (*ticker).clone();
            relabeled.push(((*ticker).clone(), copy));
        }
    }
    relabeled.sort_by(|a, b| a.0.cmp(&b.0));
    relabeled.into_iter().map(|(_, row)| row).collect()
}

pub(crate) struct Span {
    ticker: Symbol,
    from: NaiveDate,
    to: NaiveDate,
    asked: Symbol,
}

pub(crate) fn spans(
    listings: &Listings,
    symbols: &[Symbol],
    on: NaiveDate,
    since: NaiveDate,
    through: NaiveDate,
) -> Vec<Span> {
    let mut spans = Vec::new();
    if since > through {
        return spans;
    }
    for (asked, key) in resolve(listings, symbols, on) {
        let base = key
            .ended_listing()
            .map_or_else(|| key.clone(), |(ticker, _)| ticker);
        for ticker in listings.former(&[base]) {
            for (from, to, listing) in listings.segments(&ticker, since, through) {
                if listing == key {
                    spans.push(Span {
                        ticker: ticker.clone(),
                        from,
                        to,
                        asked: asked.clone(),
                    });
                }
            }
        }
    }
    spans
}

fn span_tickers(spans: &[Span]) -> Vec<Symbol> {
    unique(
        &spans
            .iter()
            .map(|span| span.ticker.clone())
            .collect::<Vec<_>>(),
    )
}

fn within_spans<T: Clone>(
    spans: &[Span],
    rows: Vec<T>,
    date: impl Fn(&T) -> NaiveDate,
    symbol: impl Fn(&mut T) -> &mut Symbol,
) -> Vec<T> {
    let mut kept: Vec<(Symbol, T)> = Vec::new();
    for mut row in rows {
        let day = date(&row);
        let ticker = symbol(&mut row).clone();
        for span in spans
            .iter()
            .filter(|span| span.ticker == ticker && (span.from..=span.to).contains(&day))
        {
            let mut copy = row.clone();
            *symbol(&mut copy) = span.asked.clone();
            kept.push((span.asked.clone(), copy));
        }
    }
    kept.sort_by(|a, b| a.0.cmp(&b.0));
    kept.into_iter().map(|(_, row)| row).collect()
}

pub(crate) fn delisted_before(
    connection: &Connection,
    symbols: &[Symbol],
    date: NaiveDate,
) -> Result<BTreeSet<Symbol>> {
    if symbols.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut statement = connection.prepare_cached(&format!(
        "SELECT symbol FROM instrument WHERE last_session < ? AND symbol {IN_SYMBOLS}"
    ))?;
    let params: Params = vec![Box::new(date), symbol_list(symbols)];
    let delisted = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(row.get::<_, Sym>(0)?.0)
        })?
        .collect::<duckdb::Result<BTreeSet<_>>>()?;
    Ok(delisted)
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
         FROM {source} WHERE b.symbol {IN_SYMBOLS} AND {start} {upper} ?"
    );
    let mut params = vec![symbol_list(query.symbols)];
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
                feed: row.get::<_, FeedColumn>(7)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(bars)
}

pub(crate) fn traded_bars(
    connection: &Connection,
    listings: &Listings,
    query: &BarQuery<'_>,
) -> Result<Vec<Bar>> {
    let first = query.from.unwrap_or(query.to).exchange_date();
    let last = query.to.exchange_date();
    let mut spans: BTreeMap<Symbol, Vec<(&Symbol, NaiveDate, NaiveDate)>> = BTreeMap::new();
    for ticker in query.symbols {
        for (from, to, key) in listings.segments(ticker, first, last) {
            spans.entry(key).or_default().push((ticker, from, to));
        }
    }
    let keys: Vec<Symbol> = spans.keys().cloned().collect();
    let stored = bars(
        connection,
        &BarQuery {
            symbols: &keys,
            ..*query
        },
    )?;
    let mut traded = Vec::new();
    for bar in stored {
        let date = bar.start.exchange_date();
        for (ticker, from, to) in spans.get(&bar.symbol).into_iter().flatten() {
            if (*from..=*to).contains(&date) {
                traded.push(Bar {
                    symbol: (*ticker).clone(),
                    ..bar.clone()
                });
            }
        }
    }
    traded.sort_by(|a, b| (a.start, &a.symbol).cmp(&(b.start, &b.symbol)));
    Ok(traded)
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
    listed_since: &BTreeMap<Symbol, Timestamp>,
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
            let listed = listed_since
                .get(&bar.symbol)
                .is_none_or(|since| bar.start >= *since);
            if listed && is_visible(calendar, timeframe, &bar, as_of) {
                found.entry(bar.symbol.clone()).or_default().push(bar);
            }
        }
        pending.retain(|symbol| {
            let enough = found.get(symbol).map_or(0, Vec::len) >= count;
            let exhausted = from
                .is_none_or(|from| listed_since.get(symbol).is_some_and(|since| from <= *since));
            if enough || exhausted {
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

fn listings_started(
    actions: &Actions,
    calendar: &Calendar,
    listings: &Listings,
    keys: &[Symbol],
    as_of: Timestamp,
) -> BTreeMap<Symbol, Timestamp> {
    let mut started = BTreeMap::new();
    for action in listing_actions(
        actions,
        calendar,
        keys,
        HISTORY_START,
        as_of.exchange_date(),
    ) {
        let taken_over = matches!(action.kind, CorporateActionKind::NameChange { .. })
            && listings.renamed_into(&action.symbol, action.ex_date);
        if let Some(since) = corporate_action_visible_at(action.ex_date, calendar)
            .filter(|visible_at| action.kind.ends_listing() && !taken_over && *visible_at <= as_of)
        {
            started.insert(action.symbol, since);
        }
    }
    started
}

fn listing_actions(
    actions: &Actions,
    calendar: &Calendar,
    keys: &[Symbol],
    from: NaiveDate,
    to: NaiveDate,
) -> Vec<CorporateAction> {
    let mut ended: BTreeMap<Symbol, Vec<(NaiveDate, &Symbol)>> = BTreeMap::new();
    for key in keys {
        if let Some((ticker, last)) = key.ended_listing() {
            let until = calendar.next_after(last).map_or(last, |next| next.date);
            ended.entry(ticker).or_default().push((until, key));
        }
    }
    let asked: BTreeSet<&Symbol> = keys.iter().collect();
    let stored: Vec<Symbol> = keys.iter().chain(ended.keys()).cloned().collect();
    let mut listed = Vec::new();
    for action in actions.between(&stored, from, to) {
        let owner = ended.get(&action.symbol).and_then(|ends| {
            ends.iter()
                .find(|(until, _)| action.ex_date <= *until)
                .map(|(_, key)| (*key).clone())
        });
        if let Some(key) = owner {
            listed.push(CorporateAction {
                symbol: key,
                ..action.clone()
            });
        }
        if asked.contains(&action.symbol) {
            listed.push(action);
        }
    }
    listed.sort_by(|a, b| (&a.symbol, a.ex_date).cmp(&(&b.symbol, b.ex_date)));
    listed
}

pub(crate) fn traded_actions(
    actions: &Actions,
    calendar: &Calendar,
    listings: &Listings,
    symbols: &[Symbol],
    from: NaiveDate,
    to: NaiveDate,
) -> Vec<CorporateAction> {
    let symbols = unique(symbols);
    let renamed = |action: &CorporateAction| matches!(&action.kind, CorporateActionKind::NameChange { new_symbol } if *new_symbol != action.symbol);
    let resolved = resolve(listings, &symbols, from.pred_opt().unwrap_or(from));
    let keyed: Vec<CorporateAction> =
        listing_actions(actions, calendar, &keys(&resolved), from, to)
            .into_iter()
            .filter(|action| !matches!(action.kind, CorporateActionKind::NameChange { .. }))
            .collect();
    let mut traded = relabel(&resolved, keyed, |action| &mut action.symbol);
    traded.extend(
        actions
            .between(&symbols, from, to)
            .into_iter()
            .filter(renamed),
    );
    traded.sort_by(|a, b| {
        (&a.symbol, a.ex_date, renamed(a)).cmp(&(&b.symbol, b.ex_date, renamed(b)))
    });
    traded
}

fn to_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(f64::NAN)
}

fn adjusted_row(bar: Bar, factors: &[AdjustmentFactor]) -> BarRow {
    let (price, volume) = AdjustmentFactor::cumulative(bar.start.exchange_date(), factors);
    BarRow {
        start: bar.start,
        open: to_f64(bar.open * price),
        high: to_f64(bar.high * price),
        low: to_f64(bar.low * price),
        close: to_f64(bar.close * price),
        volume: to_f64(Decimal::from(bar.volume) * volume),
        symbol: bar.symbol,
    }
}

pub(crate) fn chart(
    connection: &Connection,
    calendar: &Calendar,
    listings: &Listings,
    actions: &Actions,
    request: &ChartRequest,
) -> Result<ChartBars> {
    let Some(key) = listings.listing(&request.symbol, request.listed_on) else {
        return Ok(ChartBars {
            bars: Vec::new(),
            older: false,
            splits: Vec::new(),
        });
    };
    let keys = [key.clone()];
    let (to, to_inclusive) = match request.before.filter(|before| *before <= request.as_of) {
        Some(before) => (before, false),
        None => (request.as_of, true),
    };
    let fetched = bars(
        connection,
        &BarQuery {
            timeframe: request.timeframe,
            symbols: &keys,
            from: Some(
                request
                    .after
                    .map_or(request.from, |after| after.max(request.from)),
            ),
            to,
            to_inclusive,
            latest: Some(request.bars + 2),
        },
    )?;
    let mut visible: Vec<Bar> = fetched
        .into_iter()
        .filter(|bar| request.after.is_none_or(|after| bar.start > after))
        .filter(|bar| is_visible(calendar, request.timeframe, bar, request.as_of))
        .collect();
    let older = visible.len() > request.bars;
    visible.drain(..visible.len().saturating_sub(request.bars));
    let splits = adjustment_factors(
        connection,
        calendar,
        actions,
        &keys,
        Adjustment::Split,
        request.from.exchange_date(),
        request.as_of,
    )?
    .remove(&key)
    .unwrap_or_default();
    Ok(ChartBars {
        bars: visible
            .into_iter()
            .map(|bar| {
                let bar = Bar {
                    symbol: request.symbol.clone(),
                    ..bar
                };
                adjusted_row(bar, &splits)
            })
            .collect(),
        older,
        splits,
    })
}

pub(crate) fn history(
    connection: &Connection,
    calendar: &Calendar,
    listings: &Listings,
    actions: &Actions,
    request: &HistoryRequest,
) -> Result<Vec<BarRow>> {
    let count = request.bars as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let on = listed_on(calendar, request.as_of);
    let resolved = resolve(listings, &unique(&request.symbols), on);
    let keys = keys(&resolved);
    let listed_since = listings_started(actions, calendar, listings, &keys, request.as_of);
    let visible = visible_bars(
        connection,
        calendar,
        &listed_since,
        request.timeframe,
        keys,
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
    let priced: Vec<Symbol> = visible.keys().cloned().collect();
    let factors = adjustment_factors(
        connection,
        calendar,
        actions,
        &priced,
        request.adjust,
        earliest,
        request.as_of,
    )?;
    let mut rows: Vec<BarRow> = Vec::new();
    for (ticker, key) in &resolved {
        let key_factors = factors.get(key).map_or(&[][..], Vec::as_slice);
        for bar in visible.get(key).into_iter().flatten() {
            let bar = Bar {
                symbol: ticker.clone(),
                ..bar.clone()
            };
            rows.push(adjusted_row(bar, key_factors));
        }
    }
    rows.sort_by(|left, right| (left.start, &left.symbol).cmp(&(right.start, &right.symbol)));
    Ok(rows)
}

pub(crate) fn split_adjusted_closes(
    connection: &Connection,
    calendar: &Calendar,
    listings: &Listings,
    actions: &Actions,
    symbols: &[Symbol],
    as_of: Timestamp,
) -> Result<BTreeMap<Symbol, Decimal>> {
    let resolved = resolve(listings, &unique(symbols), listed_on(calendar, as_of));
    let keys = keys(&resolved);
    let listed_since = listings_started(actions, calendar, listings, &keys, as_of);
    let visible = visible_bars(
        connection,
        calendar,
        &listed_since,
        Timeframe::Day,
        keys,
        1,
        as_of,
    )?;
    let Some(earliest) = visible
        .values()
        .flatten()
        .map(|bar| bar.start.exchange_date())
        .min()
    else {
        return Ok(BTreeMap::new());
    };
    let priced: Vec<Symbol> = visible.keys().cloned().collect();
    let factors = adjustment_factors(
        connection,
        calendar,
        actions,
        &priced,
        Adjustment::Split,
        earliest,
        as_of,
    )?;
    let mut closes = BTreeMap::new();
    for (ticker, key) in &resolved {
        let key_factors = factors.get(key).map_or(&[][..], Vec::as_slice);
        if let Some(bar) = visible.get(key).and_then(|bars| bars.last()) {
            let (price, _) = AdjustmentFactor::cumulative(bar.start.exchange_date(), key_factors);
            closes.insert(ticker.clone(), bar.close * price);
        }
    }
    Ok(closes)
}

fn adjustment_factors(
    connection: &Connection,
    calendar: &Calendar,
    actions: &Actions,
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
        listing_actions(actions, calendar, symbols, after, as_of.exchange_date())
            .into_iter()
            .filter(|action| {
                corporate_action_visible_at(action.ex_date, calendar)
                    .is_some_and(|visible_at| visible_at <= as_of)
            })
            .collect();
    let mut factors: BTreeMap<Symbol, Vec<AdjustmentFactor>> = BTreeMap::new();
    for action in &actions {
        if let Some(factor) = AdjustmentFactor::for_shares(action, adjustment) {
            factors
                .entry(action.symbol.clone())
                .or_default()
                .push(factor);
        }
    }
    if adjustment == Adjustment::All {
        for (symbol, factor) in distribution_factors(connection, calendar, &actions, as_of)? {
            factors.entry(symbol).or_default().push(factor);
        }
    }
    Ok(factors)
}

fn distribution_factors(
    connection: &Connection,
    calendar: &Calendar,
    actions: &[CorporateAction],
    as_of: Timestamp,
) -> Result<Vec<(Symbol, AdjustmentFactor)>> {
    let ex_session = |ex_date: NaiveDate| {
        corporate_action_visible_at(ex_date, calendar)
            .and_then(|open| calendar.session(open.exchange_date()))
    };
    let distributions: Vec<(&CorporateAction, NaiveDate)> = actions
        .iter()
        .filter(|action| {
            matches!(
                action.kind,
                CorporateActionKind::CashDividend { .. } | CorporateActionKind::SpinOff { .. }
            ) && action.kind.has_valid_rates()
        })
        .filter_map(|action| {
            calendar
                .previous_before(action.ex_date)
                .map(|previous| (action, previous.date))
        })
        .collect();
    let mut wanted: BTreeSet<(Symbol, NaiveDate)> = BTreeSet::new();
    for (action, previous) in &distributions {
        wanted.insert((action.symbol.clone(), *previous));
        if let CorporateActionKind::SpinOff { new_symbol, .. } = &action.kind
            && let Some(session) = ex_session(action.ex_date)
            && session.close <= as_of
        {
            wanted.insert((new_symbol.clone(), session.date));
        }
    }
    let prices = daily_prices(connection, &wanted)?;
    let mut values: BTreeMap<(Symbol, NaiveDate), (Decimal, NaiveDate)> = BTreeMap::new();
    for (action, previous) in &distributions {
        let value = match &action.kind {
            CorporateActionKind::CashDividend { rate } => Some(*rate),
            CorporateActionKind::SpinOff {
                new_symbol,
                old_rate,
                new_rate,
            } => ex_session(action.ex_date)
                .and_then(|session| prices.get(&(new_symbol.clone(), session.date)))
                .map(|(open, _)| *open * *new_rate / *old_rate),
            _ => None,
        };
        if let Some(value) = value {
            values
                .entry((action.symbol.clone(), action.ex_date))
                .or_insert((Decimal::ZERO, *previous))
                .0 += value;
        }
    }
    Ok(values
        .into_iter()
        .filter_map(|((symbol, ex_date), (value, previous))| {
            let (_, close) = prices.get(&(symbol.clone(), previous))?;
            AdjustmentFactor::for_distribution(ex_date, value, *close)
                .map(|factor| (symbol, factor))
        })
        .collect())
}

fn daily_prices(
    connection: &Connection,
    wanted: &BTreeSet<(Symbol, NaiveDate)>,
) -> Result<BTreeMap<(Symbol, NaiveDate), (Decimal, Decimal)>> {
    if wanted.is_empty() {
        return Ok(BTreeMap::new());
    }
    let sql = format!(
        "SELECT b.symbol, b.date, b.open, b.close FROM bar_1d b \
         JOIN (VALUES {}) AS wanted(symbol, date) \
         ON b.symbol = wanted.symbol AND b.date = wanted.date",
        vec!["(?, ?)"; wanted.len()].join(", ")
    );
    let params: Params = wanted
        .iter()
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
                (row.get::<_, Sym>(0)?.0, row.get::<_, NaiveDate>(1)?),
                (row.get::<_, Dec>(2)?.0, row.get::<_, Dec>(3)?.0),
            ))
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok(rows)
}

pub(crate) struct Actions(BTreeMap<Symbol, Vec<CorporateAction>>);

impl Actions {
    pub(crate) fn between(
        &self,
        symbols: &[Symbol],
        from: NaiveDate,
        to: NaiveDate,
    ) -> Vec<CorporateAction> {
        unique(symbols)
            .iter()
            .filter_map(|symbol| self.0.get(symbol))
            .flat_map(|actions| {
                actions
                    .iter()
                    .filter(|action| (from..=to).contains(&action.ex_date))
                    .cloned()
            })
            .collect()
    }
}

pub(crate) fn actions(connection: &Connection) -> Result<Actions> {
    let mut grouped: BTreeMap<Symbol, Vec<CorporateAction>> = BTreeMap::new();
    for action in select_actions(connection, "true", Params::new())? {
        grouped
            .entry(action.symbol.clone())
            .or_default()
            .push(action);
    }
    Ok(Actions(grouped))
}

pub(crate) fn action_disagreements(
    connection: &Connection,
    symbols: Option<&[Symbol]>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<DataWarning>> {
    let mut params: Params = vec![Box::new(from), Box::new(to)];
    let mut sql = "SELECT symbol, ex_date, action, alpaca, tiingo, kept \
                   FROM corporate_action_check WHERE ex_date BETWEEN ? AND ?"
        .to_owned();
    if let Some(symbols) = symbols {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }
        sql.push_str(&format!(" AND symbol {IN_SYMBOLS}"));
        params.push(symbol_list(symbols));
    }
    sql.push_str(" ORDER BY symbol, ex_date, action");
    let mut statement = connection.prepare_cached(&sql)?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(DataWarning::ActionDisagreement(ActionDisagreement {
                symbol: row.get::<_, Sym>(0)?.0,
                ex_date: row.get(1)?,
                action: row.get::<_, Wire<_>>(2)?.0,
                alpaca: row.get::<_, Option<Dec>>(3)?.map(|value| value.0),
                tiingo: row.get::<_, Option<Dec>>(4)?.map(|value| value.0),
                kept: row.get::<_, Wire<_>>(5)?.0,
            }))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(crate) fn actions_reaching_other_symbols(
    connection: &Connection,
) -> Result<Vec<CorporateAction>> {
    select_actions(connection, "other_symbol <> ''", Params::new())
}

fn select_actions(
    connection: &Connection,
    condition: &str,
    params: Params,
) -> Result<Vec<CorporateAction>> {
    let sql = format!(
        "SELECT symbol, ex_date, kind, other_symbol, old_rate, new_rate, rate \
         FROM corporate_action WHERE {condition} ORDER BY symbol, ex_date, kind, other_symbol"
    );
    let mut statement = connection.prepare_cached(&sql)?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            let kind = ActionColumns::parse(
                &row.get::<_, String>(2)?,
                &row.get::<_, String>(3)?,
                row.get::<_, Option<Dec>>(4)?.map(|rate| rate.0),
                row.get::<_, Option<Dec>>(5)?.map(|rate| rate.0),
                row.get::<_, Option<Dec>>(6)?.map(|rate| rate.0),
            );
            let (symbol, ex_date) = (row.get::<_, Sym>(0)?.0, row.get(1)?);
            Ok(kind.map(|kind| CorporateAction {
                symbol,
                ex_date,
                kind,
            }))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(rows.into_iter().flatten().collect())
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
         filed, available_at FROM fundamental_fact WHERE symbol {IN_SYMBOLS} AND metric IN ({}) \
         AND period_type IS NOT NULL AND available_at <= ? \
         QUALIFY row_number() OVER (PARTITION BY symbol, metric, period_end, period_type, \
         period_start ORDER BY available_at DESC, filed DESC, accession DESC) = 1",
        placeholders(metrics.len())
    );
    let mut params = vec![symbol_list(&symbols)];
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

pub(crate) fn liquidity(
    connection: &Connection,
    calendar: &Calendar,
    listings: &Listings,
    actions: &Actions,
    symbols: &[Symbol],
    before: NaiveDate,
    sessions: u32,
) -> Result<BTreeMap<Symbol, Liquidity>> {
    let resolved = resolve(listings, &unique(symbols), before);
    let keys = keys(&resolved);
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
            symbols: &keys,
            from: Some(first.open),
            to: as_of,
            to_inclusive: false,
            latest: None,
        },
    )?;
    let factors = adjustment_factors(
        connection,
        calendar,
        actions,
        &keys,
        Adjustment::Split,
        first.date,
        as_of,
    )?;
    let mut windows: BTreeMap<Symbol, Vec<Bar>> = BTreeMap::new();
    for bar in daily {
        windows.entry(bar.symbol.clone()).or_default().push(bar);
    }
    for window in windows.values_mut() {
        window.sort_by_key(|bar| bar.start);
    }
    Ok(resolved
        .into_iter()
        .filter_map(|(ticker, key)| {
            let symbol_factors = factors.get(&key).map_or(&[][..], Vec::as_slice);
            Some((
                ticker,
                Liquidity::estimate(windows.get(&key)?, symbol_factors)?,
            ))
        })
        .collect())
}

pub(crate) type Listed = (Symbol, Symbol);

pub(crate) fn top_dollar_volume(
    connection: &Connection,
    calendar: &Calendar,
    listings: &Listings,
    count: u32,
    asset_class: Option<AssetClass>,
    before: NaiveDate,
    sessions: u32,
) -> Result<Vec<Listed>> {
    let Some(first) = sessions_before(calendar, before, sessions).first() else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare_cached(
        "SELECT b.symbol FROM bar_1d b LEFT JOIN instrument i ON i.symbol = b.symbol \
         WHERE b.date >= ? AND b.date < ? AND (? IS NULL OR i.asset_class = ?) \
         AND (i.last_session IS NULL OR i.last_session >= ?) \
         GROUP BY b.symbol ORDER BY avg(CAST(b.close AS DOUBLE) * b.volume) DESC, b.symbol",
    )?;
    let class = asset_class.as_ref().map(crate::codec::wire_name);
    let keys = statement
        .query_map(params![first.date, before, class, class, before], |row| {
            Ok(row.get::<_, Sym>(0)?.0)
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(keys
        .into_iter()
        .filter_map(|key| Some((listings.ticker_of(&key, before)?, key)))
        .take(count as usize)
        .collect())
}

pub(crate) fn index_universe(
    connection: &Connection,
    calendar: &Calendar,
    listings: &Listings,
    index: StockIndex,
    before: NaiveDate,
    sessions: u32,
) -> Result<Vec<Listed>> {
    let Some(first) = sessions_before(calendar, before, sessions).first() else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare_cached(
        "SELECT symbol, start_date, end_date FROM index_member WHERE index_name = ? \
         AND (start_date IS NULL OR start_date <= ?) AND (end_date IS NULL OR ? < end_date)",
    )?;
    let members = statement
        .query_map(
            params![crate::codec::wire_name(&index), before, before],
            |row| {
                Ok((
                    row.get::<_, Sym>(0)?.0,
                    row.get::<_, Option<NaiveDate>>(1)?,
                    row.get::<_, Option<NaiveDate>>(2)?,
                ))
            },
        )?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let listed: BTreeSet<Listed> = members
        .into_iter()
        .filter_map(|(symbol, start, end)| {
            let ticker = listings.ticker(&symbol, before, start, end);
            let key = listings.listing(&ticker, before)?;
            Some((ticker, key))
        })
        .collect();
    let keys = unique(
        &listed
            .iter()
            .map(|(_, key)| key.clone())
            .collect::<Vec<_>>(),
    );
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT DISTINCT symbol FROM bar_1d WHERE symbol {IN_SYMBOLS} AND date >= ? AND date < ?"
    );
    let params: Params = vec![symbol_list(&keys), Box::new(first.date), Box::new(before)];
    let mut statement = connection.prepare_cached(&sql)?;
    let priced = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(row.get::<_, Sym>(0)?.0)
        })?
        .collect::<duckdb::Result<BTreeSet<_>>>()?;
    Ok(listed
        .into_iter()
        .filter(|(_, key)| priced.contains(key))
        .collect())
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
         WHERE i.symbol {IN_SYMBOLS} ORDER BY i.symbol"
    );
    let params = [symbol_list(&symbols)];
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
         FROM earnings_release e WHERE symbol {IN_SYMBOLS} AND accepted_at <= ? \
         AND NOT (form LIKE '%/A' AND EXISTS (SELECT 1 FROM earnings_release o \
         WHERE o.symbol = e.symbol AND o.report_date = e.report_date \
         AND o.accepted_at < e.accepted_at)) \
         ORDER BY symbol, accepted_at, accession"
    );
    let params: Params = vec![symbol_list(&symbols), Box::new(Ts(as_of))];
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

pub(crate) fn accessions(
    connection: &Connection,
    table: &str,
    symbol: &Symbol,
) -> Result<BTreeSet<String>> {
    let mut statement =
        connection.prepare_cached(&format!("SELECT accession FROM {table} WHERE symbol = ?"))?;
    let accessions = statement
        .query_map([symbol.as_str()], |row| row.get(0))?
        .collect::<duckdb::Result<_>>()?;
    Ok(accessions)
}

pub(crate) fn filings(
    connection: &Connection,
    symbols: &[Symbol],
    since: NaiveDate,
    items: Option<&[String]>,
    as_of: Timestamp,
) -> Result<Vec<Filing>> {
    let symbols = unique(symbols);
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT symbol, cik, accession, form, filed, report_date, array_to_string(items, ','), \
         accepted_at FROM current_report WHERE symbol {IN_SYMBOLS} AND filed >= ? \
         AND accepted_at <= ? AND (? OR list_has_any(items, ?::VARCHAR[])) \
         ORDER BY symbol, accepted_at, accession"
    );
    let params: Params = vec![
        symbol_list(&symbols),
        Box::new(since),
        Box::new(Ts(as_of)),
        Box::new(items.is_none()),
        text_list(items.unwrap_or_default().iter().map(String::as_str)),
    ];
    let mut statement = connection.prepare_cached(&sql)?;
    let filings = statement
        .query_map(params_from_iter(params.iter()), |row| {
            let items: String = row.get(6)?;
            let accepted_at = row.get::<_, Ts>(7)?.0;
            Ok(Filing {
                symbol: row.get::<_, Sym>(0)?.0,
                cik: row.get(1)?,
                accession: row.get(2)?,
                form: row.get(3)?,
                filed: row.get(4)?,
                report_date: row.get(5)?,
                items: items
                    .split(',')
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect(),
                accepted_at,
                available_at: accepted_at,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(filings)
}

pub(crate) fn press_releases(
    connection: &Connection,
    symbols: &[Symbol],
    since: NaiveDate,
    accessions: Option<&[String]>,
    as_of: Timestamp,
) -> Result<Vec<PressRelease>> {
    let symbols = unique(symbols);
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT e.symbol, p.accession, p.accepted_at, p.exhibit, p.text FROM press_release p \
         JOIN earnings_release e ON e.accession = p.accession \
         WHERE e.symbol {IN_SYMBOLS} AND e.filed >= ? AND p.accepted_at <= ? \
         AND p.text IS NOT NULL AND (? OR p.accession IN (SELECT unnest(?::VARCHAR[]))) \
         ORDER BY e.symbol, p.accepted_at, p.accession"
    );
    let params: Params = vec![
        symbol_list(&symbols),
        Box::new(since),
        Box::new(Ts(as_of)),
        Box::new(accessions.is_none()),
        text_list(accessions.unwrap_or_default().iter().map(String::as_str)),
    ];
    let mut statement = connection.prepare_cached(&sql)?;
    let releases = statement
        .query_map(params_from_iter(params.iter()), |row| {
            let accepted_at = row.get::<_, Ts>(2)?.0;
            Ok(PressRelease {
                symbol: row.get::<_, Sym>(0)?.0,
                accession: row.get(1)?,
                accepted_at,
                available_at: accepted_at,
                exhibit: row.get(3)?,
                text: row.get(4)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(releases)
}

pub(crate) fn press_candidates(connection: &Connection) -> Result<Vec<PressCandidate>> {
    let mut statement = connection.prepare_cached(
        "SELECT e.symbol, e.cik, e.accession, e.accepted_at FROM earnings_release e \
         WHERE NOT EXISTS (SELECT 1 FROM press_release p WHERE p.accession = e.accession) \
         ORDER BY e.accepted_at DESC, e.accession, e.symbol",
    )?;
    let candidates = statement
        .query_map([], |row| {
            Ok(PressCandidate {
                symbol: row.get::<_, Sym>(0)?.0,
                cik: row.get(1)?,
                accession: row.get(2)?,
                accepted_at: row.get::<_, Ts>(3)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(candidates)
}

pub(crate) fn unread_plans(connection: &Connection, from: Timestamp) -> Result<Vec<InsiderPlan>> {
    let mut statement = connection.prepare_cached(
        "SELECT f.symbol, f.accession, f.filed FROM insider_filing f WHERE f.accepted_at >= ? \
         AND EXISTS (SELECT 1 FROM insider_owner o \
         WHERE o.symbol = f.symbol AND o.accession = f.accession) \
         AND NOT EXISTS (SELECT 1 FROM insider_plan p \
         WHERE p.symbol = f.symbol AND p.accession = f.accession) \
         ORDER BY f.symbol, f.filed, f.accession",
    )?;
    let unread = statement
        .query_map([Ts(from)], |row| {
            Ok(InsiderPlan {
                symbol: row.get::<_, Sym>(0)?.0,
                accession: row.get(1)?,
                filed: row.get(2)?,
                plan_10b5_1: None,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(unread)
}

pub(crate) fn insider_trades(
    connection: &Connection,
    calendar: &Calendar,
    symbols: &[Symbol],
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<InsiderTrade>> {
    let symbols = unique(symbols);
    let sessions = calendar.sessions();
    let opened = sessions.partition_point(|session| session.open <= as_of);
    let Some(last_open) = opened.checked_sub(1).map(|index| sessions[index].open) else {
        return Ok(Vec::new());
    };
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let visible = format!("f.symbol {IN_SYMBOLS} AND f.filed >= ? AND f.accepted_at < ?");
    let params = || -> Params {
        vec![
            symbol_list(&symbols),
            Box::new(since),
            Box::new(Ts(last_open)),
        ]
    };
    let mut statement = connection.prepare(&format!(
        "SELECT o.symbol, o.accession, o.owner_cik, o.name, o.director, o.officer, \
         o.ten_percent_owner, o.other, o.officer_title FROM insider_owner o \
         JOIN insider_filing f ON f.symbol = o.symbol AND f.accession = o.accession \
         WHERE {visible} ORDER BY o.symbol, o.accession, o.owner_cik"
    ))?;
    let mut owners: BTreeMap<(String, String), Vec<InsiderOwner>> = BTreeMap::new();
    let rows = statement.query_map(params_from_iter(params().iter()), |row| {
        Ok((
            (row.get(0)?, row.get(1)?),
            InsiderOwner {
                cik: row.get(2)?,
                name: row.get(3)?,
                director: row.get(4)?,
                officer: row.get(5)?,
                ten_percent_owner: row.get(6)?,
                other: row.get(7)?,
                officer_title: row.get(8)?,
            },
        ))
    })?;
    for row in rows {
        let (filing, owner) = row?;
        owners.entry(filing).or_default().push(owner);
    }
    let mut statement = connection.prepare(&format!(
        "SELECT f.symbol, f.accession, f.form, f.filed, f.accepted_at, t.security, t.date, \
         t.code, t.acquired, t.shares, t.price, t.shares_after, t.direct, p.plan_10b5_1 \
         FROM insider_transaction t \
         JOIN insider_filing f ON f.symbol = t.symbol AND f.accession = t.accession \
         LEFT JOIN insider_plan p ON p.symbol = f.symbol AND p.accession = f.accession \
         WHERE {visible} ORDER BY f.symbol, f.accepted_at, f.accession, t.line"
    ))?;
    let trades = statement
        .query_map(params_from_iter(params().iter()), |row| {
            let symbol: String = row.get(0)?;
            let accession: String = row.get(1)?;
            let accepted_at = row.get::<_, Ts>(4)?.0;
            let decimal = |index| -> duckdb::Result<Option<Decimal>> {
                Ok(row.get::<_, Option<Dec>>(index)?.map(|amount| amount.0))
            };
            Ok(InsiderTrade {
                owners: owners
                    .get(&(symbol.clone(), accession.clone()))
                    .cloned()
                    .unwrap_or_default(),
                symbol: Symbol::new(&symbol),
                accession,
                form: row.get(2)?,
                filed: row.get(3)?,
                accepted_at,
                available_at: filing_visible_at(accepted_at, calendar).unwrap_or(last_open),
                plan_10b5_1: row.get(13)?,
                transaction: InsiderTransaction {
                    security: row.get(5)?,
                    date: row.get(6)?,
                    code: row.get(7)?,
                    acquired: row.get(8)?,
                    shares: decimal(9)?,
                    price: decimal(10)?,
                    shares_after: decimal(11)?,
                    direct: row.get(12)?,
                },
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(trades)
}

pub(crate) fn news(
    connection: &Connection,
    symbols: &[Symbol],
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<NewsArticle>> {
    let symbols = unique(symbols);
    let Some(from) = Timestamp::at_exchange_time(since, NaiveTime::MIN) else {
        return Ok(Vec::new());
    };
    if symbols.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare_cached(
        "SELECT id, array_to_string(symbols, ','), headline, summary, source, created_at, \
         updated_at, received_at, backfilled FROM news_article \
         WHERE list_has_any(symbols, ?::VARCHAR[]) AND created_at >= ? \
         AND CASE WHEN backfilled THEN updated_at ELSE received_at END <= ? \
         ORDER BY created_at, id, updated_at",
    )?;
    let params: Params = vec![
        symbol_list(&symbols),
        Box::new(Ts(from)),
        Box::new(Ts(as_of)),
    ];
    let articles = statement
        .query_map(params_from_iter(params.iter()), |row| {
            let symbols: String = row.get(1)?;
            Ok(NewsArticle {
                id: row.get(0)?,
                symbols: symbols
                    .split(',')
                    .filter(|symbol| !symbol.is_empty())
                    .map(Symbol::new)
                    .collect(),
                headline: row.get(2)?,
                summary: row.get(3)?,
                source: row.get(4)?,
                created_at: row.get::<_, Ts>(5)?.0,
                updated_at: row.get::<_, Ts>(6)?.0,
                received_at: row.get::<_, Ts>(7)?.0,
                backfilled: row.get(8)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(articles)
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

pub(crate) fn short_interest(
    connection: &Connection,
    calendar: &Calendar,
    spans: &[Span],
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<ShortInterest>> {
    let Some(cutoff) = published_before(as_of, calendar) else {
        return Ok(Vec::new());
    };
    let tickers = span_tickers(spans);
    if tickers.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare_cached(&format!(
        "SELECT symbol, settlement, shares, previous_shares, average_daily_volume, \
         days_to_cover, split, revised, published FROM (SELECT *, row_number() OVER \
         (PARTITION BY symbol, settlement ORDER BY published DESC, batch_id DESC) AS vintage \
         FROM short_interest WHERE symbol {IN_SYMBOLS} AND settlement >= ? AND published < ?) \
         WHERE vintage = 1 ORDER BY settlement, symbol"
    ))?;
    let params: Params = vec![symbol_list(&tickers), Box::new(since), Box::new(cutoff)];
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            let published: NaiveDate = row.get(8)?;
            Ok(ShortInterest {
                symbol: row.get::<_, Sym>(0)?.0,
                settlement: row.get(1)?,
                shares: row.get(2)?,
                previous_shares: row.get(3)?,
                average_daily_volume: row.get(4)?,
                days_to_cover: row.get::<_, Dec>(5)?.0.normalize(),
                split: row.get(6)?,
                revised: row.get(7)?,
                published,
                available_at: published_visible_at(published, calendar).unwrap_or(as_of),
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(within_spans(
        spans,
        rows,
        |row| row.settlement,
        |row| &mut row.symbol,
    ))
}

pub(crate) fn short_interest_records(
    connection: &Connection,
    settlement: NaiveDate,
) -> Result<Vec<ShortInterestRecord>> {
    let mut statement = connection.prepare_cached(
        "SELECT symbol, settlement, shares, previous_shares, average_daily_volume, \
         days_to_cover, split, revised, published FROM (SELECT *, row_number() OVER \
         (PARTITION BY symbol ORDER BY published DESC, batch_id DESC) AS vintage \
         FROM short_interest WHERE settlement = ?) WHERE vintage = 1 ORDER BY symbol",
    )?;
    let records = statement
        .query_map(params![settlement], |row| {
            Ok(ShortInterestRecord {
                symbol: row.get::<_, Sym>(0)?.0,
                settlement: row.get(1)?,
                shares: row.get(2)?,
                previous_shares: row.get(3)?,
                average_daily_volume: row.get(4)?,
                days_to_cover: row.get::<_, Dec>(5)?.0.normalize(),
                split: row.get(6)?,
                revised: row.get(7)?,
                published: row.get(8)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(records)
}

pub(crate) fn short_interest_settlements(connection: &Connection) -> Result<BTreeSet<NaiveDate>> {
    let mut statement =
        connection.prepare_cached("SELECT DISTINCT settlement FROM short_interest")?;
    let settlements = statement
        .query_map([], |row| row.get(0))?
        .collect::<duckdb::Result<BTreeSet<_>>>()?;
    Ok(settlements)
}

pub(crate) fn short_volume(
    connection: &Connection,
    calendar: &Calendar,
    spans: &[Span],
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<ShortVolume>> {
    let Some(cutoff) = published_before(as_of, calendar) else {
        return Ok(Vec::new());
    };
    let tickers = span_tickers(spans);
    if tickers.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare_cached(&format!(
        "SELECT symbol, date, short_volume, short_exempt_volume, total_volume FROM short_volume \
         WHERE symbol {IN_SYMBOLS} AND date >= ? AND date < ? ORDER BY date, symbol"
    ))?;
    let params: Params = vec![symbol_list(&tickers), Box::new(since), Box::new(cutoff)];
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            let date: NaiveDate = row.get(1)?;
            Ok(ShortVolume {
                symbol: row.get::<_, Sym>(0)?.0,
                date,
                short_volume: row.get::<_, Dec>(2)?.0.normalize(),
                short_exempt_volume: row.get::<_, Dec>(3)?.0.normalize(),
                total_volume: row.get::<_, Dec>(4)?.0.normalize(),
                available_at: published_visible_at(date, calendar).unwrap_or(as_of),
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(within_spans(
        spans,
        rows,
        |row| row.date,
        |row| &mut row.symbol,
    ))
}

pub(crate) fn borrow(
    connection: &Connection,
    spans: &[Span],
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<Borrow>> {
    let tickers = span_tickers(spans);
    let Some(from) = Timestamp::at_exchange_time(since, NaiveTime::MIN) else {
        return Ok(Vec::new());
    };
    if tickers.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare_cached(&format!(
        "SELECT a.symbol, a.recorded_at, a.rebate_rate, a.fee_rate, a.available, \
         a.available_or_more, b.created_at FROM borrow_availability a \
         JOIN ingest_batch b ON b.id = a.batch_id \
         WHERE a.symbol {IN_SYMBOLS} AND a.recorded_at >= ? AND b.created_at <= ? \
         ORDER BY a.recorded_at, a.symbol"
    ))?;
    let params: Params = vec![
        symbol_list(&tickers),
        Box::new(Ts(from)),
        Box::new(Ts(as_of)),
    ];
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(Borrow {
                symbol: row.get::<_, Sym>(0)?.0,
                at: row.get::<_, Ts>(1)?.0,
                rebate_rate: row.get::<_, Option<Dec>>(2)?.map(|rate| rate.0.normalize()),
                fee_rate: row.get::<_, Option<Dec>>(3)?.map(|rate| rate.0.normalize()),
                available: row.get(4)?,
                available_or_more: row.get(5)?,
                available_at: row.get::<_, Ts>(6)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(within_spans(
        spans,
        rows,
        |row| row.at.exchange_date(),
        |row| &mut row.symbol,
    ))
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

pub(crate) fn earnings_insight_reports(connection: &Connection) -> Result<BTreeSet<String>> {
    let mut statement = connection.prepare_cached("SELECT report FROM earnings_insight")?;
    let reports = statement
        .query_map([], |row| row.get(0))?
        .collect::<duckdb::Result<_>>()?;
    Ok(reports)
}

pub(crate) fn earnings_insights(connection: &Connection) -> Result<Vec<EarningsInsight>> {
    let mut statement = connection.prepare_cached(
        "SELECT report, issue_date, published, url, retrieved_from, digest, text \
         FROM earnings_insight ORDER BY issue_date, report",
    )?;
    let reports = statement
        .query_map([], |row| {
            Ok(EarningsInsight {
                report: row.get(0)?,
                issue_date: row.get(1)?,
                published: row.get(2)?,
                url: row.get(3)?,
                retrieved_from: row.get(4)?,
                digest: row.get(5)?,
                text: row.get(6)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(reports)
}

pub(crate) fn earnings_insight_facts(
    connection: &Connection,
) -> Result<BTreeMap<String, Vec<EstimateFact>>> {
    let mut statement = connection.prepare_cached(
        "SELECT report, metric, scope, period, as_of, value FROM earnings_insight_fact \
         ORDER BY report, metric, scope, period",
    )?;
    let rows = statement
        .query_map([], |row| {
            let period: String = row.get(3)?;
            let period = match period.as_str() {
                "" => None,
                text => Some(text.parse().map_err(|error: String| {
                    duckdb::Error::FromSqlConversionFailure(3, Type::Text, error.into())
                })?),
            };
            Ok((
                row.get::<_, String>(0)?,
                EstimateFact {
                    metric: row.get::<_, Wire<EstimateMetric>>(1)?.0,
                    scope: row.get::<_, Wire<EstimateScope>>(2)?.0,
                    period,
                    as_of: row.get(4)?,
                    value: row.get::<_, Dec>(5)?.0.normalize(),
                },
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let mut facts: BTreeMap<String, Vec<EstimateFact>> = BTreeMap::new();
    for (report, fact) in rows {
        facts.entry(report).or_default().push(fact);
    }
    Ok(facts)
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
    for id in wanted.iter().filter(|id| **id != MacroSeries::Treasury3m) {
        let rows = statement
            .query_map(params![wire_name(id), from, to, cutoff], |row| {
                Ok((*id, row.get(0)?, row.get(1)?, row.get::<_, Dec>(2)?.0))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        known.extend(rows);
    }
    let mut statement = connection.prepare_cached(
        "SELECT date, published, value FROM (SELECT r.issue_date AS date, r.published, f.value, \
         row_number() OVER (PARTITION BY r.issue_date ORDER BY r.published DESC, r.report DESC) \
         AS vintage FROM earnings_insight_fact f JOIN earnings_insight r USING (report) \
         WHERE f.metric = ? AND f.scope = ? AND f.period = '' \
         AND r.issue_date BETWEEN ? AND ? AND r.published < ?) WHERE vintage = 1 ORDER BY date",
    )?;
    for (id, (metric, scope)) in wanted.iter().filter_map(|id| Some((id, id.estimate()?))) {
        let rows = statement
            .query_map(
                params![wire_name(&metric), wire_name(&scope), from, to, cutoff],
                |row| {
                    Ok((
                        *id,
                        row.get(0)?,
                        row.get(1)?,
                        row.get::<_, Dec>(2)?.0.normalize(),
                    ))
                },
            )?
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

pub(crate) fn fomc_meetings(connection: &Connection) -> Result<Vec<FomcMeeting>> {
    let mut statement = connection.prepare_cached(
        "SELECT date, scheduled, statement_at FROM fomc_meeting ORDER BY date, scheduled",
    )?;
    let meetings = statement
        .query_map([], |row| {
            Ok(FomcMeeting {
                date: row.get(0)?,
                scheduled: row.get(1)?,
                statement_at: row.get::<_, Option<Ts>>(2)?.map(|at| at.0),
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(meetings)
}

pub(crate) fn fomc_announcements(
    connection: &Connection,
    calendar: &Calendar,
    from: NaiveDate,
    to: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<FomcAnnouncement>> {
    Ok(fomc_meetings(connection)?
        .into_iter()
        .filter(|meeting| (from..=to).contains(&meeting.date))
        .filter_map(|meeting| {
            let available_at = fomc_visible_at(&meeting, calendar)?;
            (available_at <= as_of).then_some(FomcAnnouncement {
                date: meeting.date,
                scheduled: meeting.scheduled,
                statement_at: meeting.statement_at,
                available_at,
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
        "WITH wanted (symbol) AS (SELECT unnest(?::VARCHAR[])), \
         first_bar AS (SELECT w.symbol, (SELECT min(d.date) FROM bar_1d d \
         WHERE d.symbol = w.symbol) AS first FROM wanted w) \
         SELECT f.symbol, coalesce(i.first_session, \
         CASE WHEN f.first IN ({}) THEN NULL ELSE f.first END), i.last_session \
         FROM first_bar f LEFT JOIN instrument i ON i.symbol = f.symbol",
        placeholders(floors.len().max(1))
    );
    let mut params = vec![symbol_list(symbols)];
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
    listings: &Listings,
    symbols: &[Symbol],
    timeframe: Timeframe,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<CoverageGap>> {
    let sessions = calendar.between(from, to);
    let (Some(first), Some(last)) = (sessions.first(), sessions.last()) else {
        return Ok(Vec::new());
    };
    let spans: Vec<(Symbol, NaiveDate, NaiveDate, Symbol)> = unique(symbols)
        .into_iter()
        .flat_map(|ticker| {
            listings
                .segments(&ticker, first.date, last.date)
                .into_iter()
                .map(move |(lo, hi, key)| (ticker.clone(), lo, hi, key))
        })
        .collect();
    let keys = unique(&spans.iter().map(|span| span.3.clone()).collect::<Vec<_>>());
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut params = vec![symbol_list(&keys)];
    let sql = match timeframe {
        Timeframe::Day => {
            params.push(Box::new(first.date));
            params.push(Box::new(last.date));
            params.push(symbol_list(&keys));
            params.push(Box::new(first.date));
            params.push(Box::new(last.date));
            format!(
                "SELECT symbol, date FROM bar_1d WHERE symbol {IN_SYMBOLS} AND date BETWEEN ? AND ? \
                 UNION SELECT symbol, date FROM quiet_session \
                 WHERE symbol {IN_SYMBOLS} AND date BETWEEN ? AND ?"
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
                 WHERE b.symbol {IN_SYMBOLS} AND b.start >= ? AND b.start < ? \
                 AND c.date BETWEEN ? AND ? AND b.feed = 'sip' \
                 AND i.created_at >= c.close + to_microseconds(?)"
            )
        }
    };
    let mut statement = connection.prepare_cached(&sql)?;
    let present = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((row.get::<_, Sym>(0)?.0, row.get::<_, NaiveDate>(1)?))
        })?
        .collect::<duckdb::Result<BTreeSet<_>>>()?;
    let bounds = listing_bounds(connection, calendar, &keys)?;
    let mut traded_again = connection.prepare_cached(
        "SELECT min(date) FROM (SELECT date FROM bar_1d WHERE symbol = ? AND date >= ? \
         UNION ALL SELECT date FROM quiet_session WHERE symbol = ? AND date >= ?)",
    )?;
    let mut gaps = Vec::new();
    for (ticker, lo, hi, key) in spans {
        let lo = if key == ticker && listings.renamed_away(&ticker, lo) {
            let next: Option<NaiveDate> = traded_again
                .query_row(params![key.as_str(), lo, key.as_str(), lo], |row| {
                    row.get(0)
                })?;
            match next {
                Some(next) if next <= hi => next,
                _ => continue,
            }
        } else {
            lo
        };
        let (listed, delisted) = bounds.get(&key).copied().unwrap_or_default();
        let mut open_gap: Option<(NaiveDate, NaiveDate)> = None;
        let listed_sessions = sessions.iter().filter(|session| {
            (lo..=hi).contains(&session.date)
                && listed.is_none_or(|listed| listed <= session.date)
                && delisted.is_none_or(|delisted| session.date <= delisted)
        });
        for session in listed_sessions {
            if present.contains(&(key.clone(), session.date)) {
                if let Some((gap_from, gap_to)) = open_gap.take() {
                    gaps.push(CoverageGap {
                        symbol: ticker.clone(),
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
                symbol: ticker,
                timeframe,
                from: gap_from,
                to: gap_to,
            });
        }
    }
    Ok(gaps)
}

pub(crate) fn long_silences(
    connection: &Connection,
    sessions: i64,
) -> Result<Vec<(Symbol, NaiveDate)>> {
    let mut statement = connection.prepare_cached(
        "WITH session AS (SELECT date, row_number() OVER (ORDER BY date) AS ordinal FROM calendar), \
         quiet AS (SELECT q.symbol, q.date, s.ordinal - row_number() OVER \
           (PARTITION BY q.symbol ORDER BY q.date) AS island \
           FROM quiet_session q JOIN session s USING (date) WHERE NOT contains(q.symbol, '@') \
           AND NOT EXISTS (SELECT 1 FROM bar_1d b WHERE b.symbol = q.symbol AND b.date = q.date) \
           AND NOT EXISTS (SELECT 1 FROM corporate_action a \
             WHERE a.kind = 'name_change' AND a.other_symbol = q.symbol)), \
         run AS (SELECT symbol, min(date) AS first, max(date) AS last FROM quiet \
           GROUP BY symbol, island HAVING count(*) > ?), \
         ended AS (SELECT r.symbol, r.last, max(b.date) AS traded FROM run r \
           JOIN bar_1d b ON b.symbol = r.symbol AND b.date < r.first GROUP BY r.symbol, r.last) \
         SELECT DISTINCT e.symbol, e.traded FROM ended e \
         WHERE EXISTS (SELECT 1 FROM bar_1d b WHERE b.symbol = e.symbol AND b.date > e.last) \
         AND NOT EXISTS (SELECT 1 FROM instrument i \
           WHERE i.symbol = e.symbol || '@' || strftime(e.traded, '%Y-%m-%d')) \
         ORDER BY e.symbol, e.traded",
    )?;
    let ended = statement
        .query_map(params![sessions], |row| {
            Ok((row.get::<_, Sym>(0)?.0, row.get::<_, NaiveDate>(1)?))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(ended)
}

type MemberSpan = (Symbol, NaiveDate, NaiveDate, Option<Symbol>);

fn member_spans(
    connection: &Connection,
    calendar: &Calendar,
    listings: &Listings,
    index: StockIndex,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<MemberSpan>> {
    let Some(history) = index_history(connection, index)? else {
        return Ok(Vec::new());
    };
    let mut spans: Vec<MemberSpan> = Vec::new();
    for member in history.members {
        let last = member
            .end
            .and_then(|end| end.pred_opt())
            .unwrap_or(to)
            .min(to);
        let first = member.start.unwrap_or(from).max(from);
        let mut tickers: Vec<(NaiveDate, Symbol)> = Vec::new();
        for session in calendar.between(first, last) {
            let ticker = listings.ticker(&member.symbol, session.date, member.start, member.end);
            if tickers.last().is_none_or(|(_, current)| *current != ticker) {
                tickers.push((session.date, ticker));
            }
        }
        let mut member_spans = Vec::new();
        for (index, (start, ticker)) in tickers.iter().enumerate() {
            let end = tickers
                .get(index + 1)
                .and_then(|(next, _)| next.pred_opt())
                .unwrap_or(last);
            let mut covered = *start;
            for (lo, hi, key) in listings.segments(ticker, *start, end) {
                if covered < lo {
                    member_spans.push((ticker.clone(), covered, lo.pred_opt().unwrap_or(lo), None));
                }
                member_spans.push((ticker.clone(), lo, hi, Some(key)));
                covered = hi.succ_opt().unwrap_or(hi);
            }
            if covered <= end {
                member_spans.push((ticker.clone(), covered, end, None));
            }
        }
        let traded_until = member_spans
            .iter()
            .filter_map(|(_, _, _, key)| Some(key.as_ref()?.ended_listing()?.1))
            .min()
            .unwrap_or(NaiveDate::MAX);
        spans.extend(
            member_spans
                .into_iter()
                .filter(|(_, lo, _, _)| *lo <= traded_until)
                .map(|(ticker, lo, hi, key)| (ticker, lo, hi.min(traded_until), key)),
        );
    }
    Ok(spans)
}

pub(crate) fn unpriced_members(
    connection: &Connection,
    calendar: &Calendar,
    listings: &Listings,
    index: StockIndex,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<CoverageGap>> {
    let spans = member_spans(connection, calendar, listings, index, from, to)?;
    if spans.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "WITH span (id, ticker, lo, hi, listing) AS (VALUES {}), \
         session AS (SELECT date, row_number() OVER (ORDER BY date) AS ordinal FROM calendar), \
         missing AS (SELECT s.id, s.ticker, c.date, \
           c.ordinal - row_number() OVER (PARTITION BY s.id ORDER BY c.date) AS island \
           FROM span s JOIN session c ON c.date BETWEEN s.lo AND s.hi \
           LEFT JOIN instrument i ON i.symbol = s.listing \
           WHERE (i.last_session IS NULL OR c.date <= i.last_session) \
           AND NOT EXISTS (SELECT 1 FROM bar_1d b WHERE b.symbol = s.listing AND b.date = c.date) \
           AND NOT EXISTS (SELECT 1 FROM quiet_session q \
             WHERE q.symbol IN (s.listing, s.ticker) AND q.date = c.date)) \
         SELECT ticker, min(date) AS first, max(date) FROM missing \
         GROUP BY id, ticker, island ORDER BY ticker, first",
        vec!["(?, ?, CAST(? AS DATE), CAST(? AS DATE), ?)"; spans.len()].join(", ")
    );
    let params: Params = spans
        .iter()
        .zip(0_u64..)
        .flat_map(|((ticker, lo, hi, key), id)| {
            [
                Box::new(id) as Box<dyn ToSql>,
                Box::new(ticker.as_str().to_owned()),
                Box::new(*lo),
                Box::new(*hi),
                Box::new(
                    key.as_ref()
                        .map_or_else(String::new, |key| key.as_str().to_owned()),
                ),
            ]
        })
        .collect();
    let mut statement = connection.prepare_cached(&sql)?;
    let gaps = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(CoverageGap {
                symbol: row.get::<_, Sym>(0)?.0,
                timeframe: Timeframe::Day,
                from: row.get(1)?,
                to: row.get(2)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(gaps)
}

pub(crate) fn member_warnings(
    connection: &Connection,
    calendar: &Calendar,
    actions: &Actions,
    listings: &Listings,
    index: StockIndex,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<DataWarning>> {
    let spans: Vec<(Symbol, NaiveDate, NaiveDate, Symbol)> =
        member_spans(connection, calendar, listings, index, from, to)?
            .into_iter()
            .filter_map(|(ticker, lo, hi, key)| Some((ticker, lo, hi, key?)))
            .collect();
    let keys: Vec<Symbol> = spans.iter().map(|(_, _, _, key)| key.clone()).collect();
    let mut warnings = unexplained_moves(connection, calendar, actions, &keys, from, to)?;
    warnings.extend(action_disagreements(connection, Some(&keys), from, to)?);
    Ok(warnings
        .into_iter()
        .filter_map(|warning| {
            let (key, date) = match &warning {
                DataWarning::UnexplainedMove { symbol, date, .. } => (symbol, *date),
                DataWarning::ActionDisagreement(disagreement) => {
                    (&disagreement.symbol, disagreement.ex_date)
                }
                _ => return None,
            };
            let (ticker, _, _, _) = spans
                .iter()
                .find(|(_, lo, hi, listing)| listing == key && (*lo..=*hi).contains(&date))?;
            let ticker = ticker.clone();
            Some(match warning {
                DataWarning::UnexplainedMove { date, change, .. } => DataWarning::UnexplainedMove {
                    symbol: ticker,
                    date,
                    change,
                },
                DataWarning::ActionDisagreement(disagreement) => {
                    DataWarning::ActionDisagreement(ActionDisagreement {
                        symbol: ticker,
                        ..disagreement
                    })
                }
                other => other,
            })
        })
        .collect())
}

pub(crate) fn unexplained_moves(
    connection: &Connection,
    calendar: &Calendar,
    actions: &Actions,
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
        "SELECT symbol, date, open, close FROM bar_1d WHERE symbol {IN_SYMBOLS} \
         AND date BETWEEN ? AND ? ORDER BY symbol, date"
    );
    let params: Params = vec![symbol_list(&symbols), Box::new(start), Box::new(to)];
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
    let explained: BTreeSet<(Symbol, NaiveDate)> = actions
        .between(&symbols, from, to)
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
