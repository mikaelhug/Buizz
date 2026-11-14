use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use buizz_core::api::{TextDocumentView, TextFileView};
use buizz_core::listing::Listings;
use buizz_core::market::{
    Adjustment, AdjustmentFactor, AssetClass, Bar, Borrow, Calendar, Company, CorporateAction,
    CorporateActionKind, Coverage, DataSource, Dataset, DelistingTally, EarningsInsight,
    EarningsRelease, EstimateFact, EstimateMetric, EstimateScope, EtfHolding, EtfShares,
    FailToDeliver, Filing, FomcAnnouncement, FomcMeeting, FundamentalMetric, FundamentalPoint,
    HISTORY_FLOORS, HISTORY_START, IndexHistory, IndexMember, InsiderOwner, InsiderPlan,
    InsiderReport, InsiderTrade, InsiderTransaction, Instrument, InstrumentProfile, Liquidity,
    MacroObservation, MacroPoint, MacroSeries, NewsArticle, NordicBar, NordicListing, NordicSplit,
    OptionQuote, PageViews, PendingDelisting, PressCandidate, PressRelease, RiskFreeRate,
    SIP_DELAY, SecFiling, Section, Session, ShortInterest, ShortInterestRecord, ShortVolume,
    StockIndex, StoredNordicBar, Symbol, TextCandidate, Timeframe, TradingHalt, TradingHaltRecord,
};
use buizz_core::ops::{ActionDisagreement, CoverageGap, DataWarning, UNEXPLAINED_MOVE_THRESHOLD};
use buizz_core::protocol::BarRow;
use buizz_core::time::Timestamp;
use buizz_core::visibility::{
    bar_visible_at, corporate_action_visible_at, filing_visible_at, fomc_visible_at,
    page_views_visible_at, published_before, published_visible_at,
};
use chrono::{NaiveDate, NaiveTime};
use duckdb::types::{Type, Value};
use duckdb::{Connection, ToSql, params, params_from_iter};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::codec::{
    ActionColumns, Dec, FeedColumn, IN_SYMBOLS, Params, Sym, Ts, Wire, placeholders, symbol_list,
    text_list, wire_name,
};
use crate::recall::Store;
use crate::{AccessionProgress, ChartBars, ChartRequest, EtfDates, HistoryRequest, Result};

const MINUTES_PER_SESSION: usize = 390;
const EX_DATE_REACH_DAYS: i32 = 7;

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

pub(crate) fn is_visible(
    calendar: &Calendar,
    timeframe: Timeframe,
    bar: &Bar,
    as_of: Timestamp,
) -> bool {
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
        CorporateActionKind::ends_listing,
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
    keep: fn(&CorporateActionKind) -> bool,
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
    for action in actions.between(&stored, from, to, keep) {
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
        listing_actions(actions, calendar, &keys(&resolved), from, to, |_| true)
            .into_iter()
            .filter(|action| !matches!(action.kind, CorporateActionKind::NameChange { .. }))
            .collect();
    let mut traded = relabel(&resolved, keyed, |action| &mut action.symbol);
    traded.extend(
        actions
            .between(&symbols, from, to, |_| true)
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
        |wanted| daily_prices(connection, wanted),
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
    mut recall: Option<&mut Store>,
) -> Result<Vec<BarRow>> {
    let count = request.bars as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let on = listed_on(calendar, request.as_of);
    let resolved = resolve(listings, &unique(&request.symbols), on);
    let keys = keys(&resolved);
    let listed_since = listings_started(actions, calendar, listings, &keys, request.as_of);
    let visible = match recall.as_deref_mut() {
        Some(recall) if request.timeframe == Timeframe::Day => recall.visible(
            connection,
            calendar,
            &listed_since,
            keys,
            count,
            request.as_of,
        )?,
        _ => visible_bars(
            connection,
            calendar,
            &listed_since,
            request.timeframe,
            keys,
            count,
            request.as_of,
        )?,
    };
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
        |wanted| match recall {
            Some(recall) => recall.prices(connection, wanted),
            None => daily_prices(connection, wanted),
        },
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
        |wanted| daily_prices(connection, wanted),
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

type Wanted = BTreeSet<(Symbol, NaiveDate)>;
type DailyPrices = BTreeMap<(Symbol, NaiveDate), (Decimal, Decimal)>;

fn adjustment_factors(
    prices: impl FnOnce(&Wanted) -> Result<DailyPrices>,
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
    let actions: Vec<CorporateAction> = listing_actions(
        actions,
        calendar,
        symbols,
        after,
        as_of.exchange_date(),
        |_| true,
    )
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
        for (symbol, factor) in distribution_factors(prices, calendar, &actions, as_of)? {
            factors.entry(symbol).or_default().push(factor);
        }
    }
    Ok(factors)
}

fn distribution_factors(
    prices: impl FnOnce(&Wanted) -> Result<DailyPrices>,
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
    let prices = prices(&wanted)?;
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

pub(crate) fn daily_prices(
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
        keep: fn(&CorporateActionKind) -> bool,
    ) -> Vec<CorporateAction> {
        unique(symbols)
            .iter()
            .filter_map(|symbol| self.0.get(symbol))
            .flat_map(|actions| {
                actions
                    .iter()
                    .filter(|action| (from..=to).contains(&action.ex_date) && keep(&action.kind))
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

const SHARE_JUMP: u32 = 100;
const EPS_CEILING: u32 = 100_000;
const EPS_CHECKED: u32 = 1_000;
const EPS_RATIO: u32 = 100;

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
        "WITH visible AS (SELECT symbol, metric, period_type, period_start, period_end, value, \
           unit, form, filed, available_at, accession, \
           CASE WHEN regexp_full_match(split_part(unit, '/', 1), '[A-Z]{{3}}') \
             THEN split_part(unit, '/', 1) END AS currency \
           FROM fundamental_fact WHERE symbol {IN_SYMBOLS} AND period_type IS NOT NULL \
           AND period_end <= filed \
           AND available_at <= ?), \
         reporting AS (SELECT symbol, arg_max(currency, available_at) AS currency FROM visible \
           WHERE currency IS NOT NULL GROUP BY symbol), \
         latest AS (SELECT v.* FROM visible v LEFT JOIN reporting r USING (symbol) \
           WHERE (v.metric IN ({}) OR v.metric IN ('net_income', 'shares_outstanding')) \
           AND (v.currency IS NULL OR v.currency = r.currency) \
           QUALIFY row_number() OVER (PARTITION BY v.symbol, v.metric, v.period_end, \
           v.period_type, v.period_start ORDER BY v.available_at DESC, v.filed DESC, \
           v.accession DESC) = 1), \
         shares AS (SELECT symbol, period_type, period_start, period_end, value, \
           lag(value) OVER w AS before, lead(value) OVER w AS after FROM latest \
           WHERE metric = 'shares_outstanding' \
           WINDOW w AS (PARTITION BY symbol ORDER BY period_end, period_start NULLS FIRST)) \
         SELECT l.symbol, l.metric, l.period_type, l.period_start, l.period_end, l.value, l.unit, \
         l.form, l.filed, l.available_at FROM latest l \
         LEFT JOIN shares s ON l.metric = 'shares_outstanding' AND s.symbol = l.symbol \
           AND s.period_type = l.period_type AND s.period_end = l.period_end \
           AND s.period_start IS NOT DISTINCT FROM l.period_start \
         LEFT JOIN latest n ON l.metric = 'eps_diluted' AND n.metric = 'net_income' \
           AND n.symbol = l.symbol AND n.period_type = l.period_type \
           AND n.period_end = l.period_end AND n.period_start IS NOT DISTINCT FROM l.period_start \
         LEFT JOIN latest w ON l.metric = 'eps_diluted' AND w.metric = 'shares_outstanding' \
           AND w.symbol = l.symbol AND w.period_end = l.period_end \
           AND w.period_start IS NOT DISTINCT FROM l.period_start AND w.value > 0 \
         WHERE l.metric IN ({}) \
         AND NOT coalesce(s.value > 0 AND s.before > 0 AND s.after > 0 \
           AND greatest(s.value / s.before, s.before / s.value) > {SHARE_JUMP} \
           AND greatest(s.value / s.after, s.after / s.value) > {SHARE_JUMP}, false) \
         AND NOT (l.metric = 'eps_diluted' AND (abs(l.value) >= {EPS_CEILING} \
           OR coalesce(abs(l.value) >= {EPS_CHECKED} \
             AND abs(l.value) > {EPS_RATIO} * abs(n.value / w.value), false)))",
        placeholders(metrics.len()),
        placeholders(metrics.len())
    );
    let mut params = vec![symbol_list(&symbols), Box::new(Ts(as_of)) as Box<dyn ToSql>];
    for _ in 0..2 {
        params.extend(
            metrics
                .iter()
                .map(|metric| Box::new(metric.clone()) as Box<dyn ToSql>),
        );
    }
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
    sort_points(&mut points);
    Ok(points)
}

pub(crate) fn sort_points(points: &mut [FundamentalPoint]) {
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
}

pub(crate) fn fact_arrivals(
    connection: &Connection,
    symbols: &[Symbol],
) -> Result<BTreeMap<Symbol, Vec<Timestamp>>> {
    let mut arrivals: BTreeMap<Symbol, Vec<Timestamp>> = BTreeMap::new();
    if symbols.is_empty() {
        return Ok(arrivals);
    }
    let sql = format!(
        "SELECT DISTINCT symbol, available_at FROM fundamental_fact WHERE symbol {IN_SYMBOLS} \
         ORDER BY symbol, available_at"
    );
    let params: Params = vec![symbol_list(symbols)];
    let mut statement = connection.prepare_cached(&sql)?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((row.get::<_, Sym>(0)?.0, row.get::<_, Ts>(1)?.0))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    for (symbol, at) in rows {
        arrivals.entry(symbol).or_default().push(at);
    }
    Ok(arrivals)
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
        |wanted| daily_prices(connection, wanted),
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
        "SELECT symbol, start_date, end_date, article FROM index_member WHERE index_name = ? \
         ORDER BY symbol, start_date NULLS FIRST",
    )?;
    let members = statement
        .query_map(params![name], |row| {
            Ok(IndexMember {
                index,
                symbol: row.get::<_, Sym>(0)?.0,
                start: row.get(1)?,
                end: row.get(2)?,
                article: row.get(3)?,
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

pub(crate) fn symbol_accessions(
    connection: &Connection,
    table: &str,
) -> Result<BTreeSet<(Symbol, String)>> {
    let mut statement =
        connection.prepare_cached(&format!("SELECT symbol, accession FROM {table}"))?;
    let accessions = statement
        .query_map([], |row| Ok((row.get::<_, Sym>(0)?.0, row.get(1)?)))?
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

pub(crate) fn text_candidates(connection: &Connection) -> Result<Vec<TextCandidate>> {
    let mut statement = connection.prepare_cached(
        "SELECT c.cik, c.accession, any_value(c.form), any_value(c.accepted_at) \
         FROM (SELECT cik, accession, form, accepted_at FROM current_report \
         UNION ALL SELECT cik, accession, form, accepted_at FROM periodic_report) c \
         WHERE NOT EXISTS (SELECT 1 FROM filing_text f WHERE f.accession = c.accession) \
         GROUP BY c.cik, c.accession ORDER BY any_value(c.accepted_at) DESC, c.accession",
    )?;
    let candidates = statement
        .query_map([], |row| {
            Ok(TextCandidate {
                cik: row.get(0)?,
                accession: row.get(1)?,
                form: row.get(2)?,
                accepted_at: row.get::<_, Ts>(3)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(candidates)
}

pub(crate) struct VolumeTotals {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) documents: u64,
}

pub(crate) fn text_volume_totals(connection: &Connection) -> Result<Vec<VolumeTotals>> {
    let mut statement = connection.prepare_cached(
        "SELECT v.id, v.path, count(f.id), coalesce(sum(f.bytes), 0)::UBIGINT, \
         coalesce(sum(f.documents), 0)::UBIGINT \
         FROM text_volume v LEFT JOIN text_file f ON f.volume = v.id \
         GROUP BY v.id, v.path ORDER BY v.path",
    )?;
    let totals = statement
        .query_map([], |row| {
            Ok(VolumeTotals {
                id: row.get(0)?,
                path: row.get(1)?,
                files: row.get(2)?,
                bytes: row.get(3)?,
                documents: row.get(4)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(totals)
}

pub(crate) fn text_files(
    connection: &Connection,
    volume: &str,
    prefix: &str,
) -> Result<Vec<TextFileView>> {
    let mut statement = connection.prepare_cached(
        "SELECT f.id, f.path, f.source, f.documents, f.bytes, b.created_at \
         FROM text_file f LEFT JOIN ingest_batch b ON b.id = f.batch_id \
         WHERE f.volume = ? AND starts_with(f.path, ?) ORDER BY f.path",
    )?;
    let files = statement
        .query_map(params![volume, prefix], |row| {
            Ok(TextFileView {
                id: row.get(0)?,
                path: row.get(1)?,
                source: row.get(2)?,
                documents: row.get(3)?,
                bytes: row.get(4)?,
                written_at: row.get::<_, Option<Ts>>(5)?.map(|stamp| stamp.0),
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(files)
}

pub(crate) fn text_file_documents(
    connection: &Connection,
    file: i64,
) -> Result<Vec<TextDocumentView>> {
    let mut statement = connection.prepare_cached(
        "SELECT doc_id, kind, document, description, accession, cik, published_at, raw_bytes, \
         text_chars FROM text_document WHERE file_id = ? ORDER BY doc_id",
    )?;
    let documents = statement
        .query_map([file], |row| {
            Ok(TextDocumentView {
                doc_id: row.get(0)?,
                kind: row.get(1)?,
                document: row.get(2)?,
                description: row.get(3)?,
                accession: row.get(4)?,
                cik: row.get(5)?,
                published_at: row.get::<_, Ts>(6)?.0,
                raw_bytes: row.get(7)?,
                text_chars: row.get(8)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(documents)
}

pub(crate) fn text_location(
    connection: &Connection,
    doc_id: &str,
) -> Result<Option<(String, String, String)>> {
    let mut statement = connection.prepare_cached(
        "SELECT f.volume, f.path, d.content_hash FROM text_document d \
         JOIN text_file f ON f.id = d.file_id WHERE d.doc_id = ? \
         ORDER BY d.batch_id DESC LIMIT 1",
    )?;
    let mut rows =
        statement.query_map([doc_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    Ok(rows.next().transpose()?)
}

pub(crate) fn parquet_text(
    connection: &Connection,
    file: &Path,
    doc_id: &str,
    content_hash: &str,
) -> Result<Option<String>> {
    let quoted = file.to_string_lossy().replace('\'', "''");
    let mut statement = connection.prepare(&format!(
        "SELECT text FROM read_parquet('{quoted}') WHERE doc_id = ? AND content_hash = ?"
    ))?;
    let mut rows = statement.query_map(params![doc_id, content_hash], |row| {
        row.get::<_, Option<String>>(0)
    })?;
    Ok(rows.next().transpose()?.flatten())
}

pub(crate) fn filing_text_progress(connection: &Connection) -> Result<AccessionProgress> {
    Ok(connection.query_row(
        "SELECT count(*), count(*) FILTER (WHERE f.accession IS NOT NULL AND f.failure IS NULL), \
         count(*) FILTER (WHERE f.failure IS NOT NULL) \
         FROM (SELECT accession FROM current_report UNION SELECT accession FROM periodic_report) a \
         LEFT JOIN filing_text f ON f.accession = a.accession",
        [],
        |row| {
            Ok(AccessionProgress {
                expected: row.get(0)?,
                done: row.get(1)?,
                failed: row.get(2)?,
            })
        },
    )?)
}

pub(crate) fn press_release_progress(connection: &Connection) -> Result<AccessionProgress> {
    Ok(connection.query_row(
        "SELECT count(DISTINCT e.accession), count(DISTINCT p.accession), 0 \
         FROM earnings_release e LEFT JOIN press_release p ON p.accession = e.accession",
        [],
        |row| {
            Ok(AccessionProgress {
                expected: row.get(0)?,
                done: row.get(1)?,
                failed: row.get(2)?,
            })
        },
    )?)
}

const SPLIT_SCOPE: &str = "(d.kind IN ('10-K', '10-K/A', '10-Q', '10-Q/A') \
     OR (d.kind IN ('8-K', '8-K/A') AND EXISTS (SELECT 1 FROM current_report c \
     WHERE c.accession = d.accession AND len(c.items) > 0)))";

pub(crate) struct UnsplitLocation {
    pub(crate) doc_id: String,
    pub(crate) content_hash: String,
    pub(crate) kind: String,
    pub(crate) items: Vec<String>,
    pub(crate) published_at: Timestamp,
    pub(crate) volume: String,
    pub(crate) path: String,
}

pub(crate) fn unsplit_documents(
    connection: &Connection,
    splitter_version: u32,
    limit: usize,
) -> Result<Vec<UnsplitLocation>> {
    let mut statement = connection.prepare_cached(&format!(
        "WITH unsplit AS (SELECT d.doc_id, d.content_hash, d.kind, d.accession, d.published_at, \
         f.volume, f.path FROM text_document d JOIN text_file f ON f.id = d.file_id \
         WHERE {SPLIT_SCOPE} AND NOT EXISTS (SELECT 1 FROM section s \
         WHERE s.doc_id = d.doc_id AND s.content_hash = d.content_hash \
         AND s.splitter_version = ?) \
         ORDER BY d.published_at DESC, d.doc_id LIMIT ?) \
         SELECT u.doc_id, u.content_hash, u.kind, \
         coalesce((SELECT array_to_string(any_value(c.items), ',') FROM current_report c \
         WHERE c.accession = u.accession), ''), u.published_at, u.volume, u.path \
         FROM unsplit u ORDER BY u.published_at DESC, u.doc_id"
    ))?;
    let documents = statement
        .query_map(params![splitter_version, limit as u64], |row| {
            Ok(UnsplitLocation {
                doc_id: row.get(0)?,
                content_hash: row.get(1)?,
                kind: row.get(2)?,
                items: row
                    .get::<_, String>(3)?
                    .split(',')
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect(),
                published_at: row.get::<_, Ts>(4)?.0,
                volume: row.get(5)?,
                path: row.get(6)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(documents)
}

pub(crate) fn parquet_texts(
    connection: &Connection,
    file: &Path,
    doc_ids: &[&str],
) -> Result<BTreeMap<(String, String), Option<String>>> {
    let quoted = file.to_string_lossy().replace('\'', "''");
    let mut statement = connection.prepare(&format!(
        "SELECT doc_id, content_hash, text FROM read_parquet('{quoted}') \
         WHERE doc_id {IN_SYMBOLS}"
    ))?;
    let texts = statement
        .query_map([text_list(doc_ids.iter().copied())], |row| {
            Ok(((row.get(0)?, row.get(1)?), row.get(2)?))
        })?
        .collect::<duckdb::Result<BTreeMap<_, _>>>()?;
    Ok(texts)
}

pub(crate) fn split_progress(
    connection: &Connection,
    splitter_version: u32,
) -> Result<AccessionProgress> {
    Ok(connection.query_row(
        &format!(
            "SELECT count(*), count(*) FILTER (WHERE s.split), \
             count(*) FILTER (WHERE s.unsplittable AND NOT s.split) FROM text_document d \
             LEFT JOIN (SELECT doc_id, content_hash, bool_or(failure IS NULL) AS split, \
             bool_or(failure IS NOT NULL) AS unsplittable FROM section WHERE splitter_version = ? \
             GROUP BY doc_id, content_hash) s \
             ON s.doc_id = d.doc_id AND s.content_hash = d.content_hash WHERE {SPLIT_SCOPE}"
        ),
        [splitter_version],
        |row| {
            Ok(AccessionProgress {
                expected: row.get(0)?,
                done: row.get(1)?,
                failed: row.get(2)?,
            })
        },
    )?)
}

pub(crate) fn document_sections(
    connection: &Connection,
    doc_id: &str,
    content_hash: &str,
) -> Result<(Vec<Section>, Option<String>)> {
    let mut statement = connection.prepare_cached(
        "SELECT key, start_char, end_char, failure FROM section \
         WHERE doc_id = ? AND content_hash = ? AND splitter_version = \
         (SELECT max(splitter_version) FROM section WHERE doc_id = ? AND content_hash = ?) \
         ORDER BY ordinal",
    )?;
    let mut sections = Vec::new();
    let mut failure = None;
    let rows = statement.query_map(params![doc_id, content_hash, doc_id, content_hash], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<u64>>(1)?,
            row.get::<_, Option<u64>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        match row? {
            (Some(key), Some(start), Some(end), None) => sections.push(Section { key, start, end }),
            (_, _, _, reason) => failure = reason.or(failure),
        }
    }
    Ok((sections, failure))
}

pub(crate) fn insider_quarters(connection: &Connection) -> Result<BTreeSet<NaiveDate>> {
    let mut statement = connection.prepare_cached("SELECT quarter FROM insider_quarter")?;
    let quarters = statement
        .query_map([], |row| row.get(0))?
        .collect::<duckdb::Result<_>>()?;
    Ok(quarters)
}

pub(crate) fn insider_reports(
    connection: &Connection,
    accessions: &[String],
) -> Result<BTreeMap<String, InsiderReport>> {
    let listed = || text_list(accessions.iter().map(String::as_str));
    let mut statement = connection.prepare_cached(
        "SELECT accession, issuer_cik, plan_10b5_1 FROM insider_report \
         WHERE accession IN (SELECT unnest(?::VARCHAR[]))",
    )?;
    let mut reports: BTreeMap<String, InsiderReport> = statement
        .query_map([listed()], |row| {
            let accession: String = row.get(0)?;
            Ok((
                accession.clone(),
                InsiderReport {
                    accession,
                    issuer_cik: row.get(1)?,
                    plan_10b5_1: row.get(2)?,
                    owners: Vec::new(),
                    transactions: Vec::new(),
                },
            ))
        })?
        .collect::<duckdb::Result<_>>()?;
    let mut statement = connection.prepare_cached(
        "SELECT accession, owner_cik, name, director, officer, ten_percent_owner, other, \
         officer_title FROM insider_report_owner \
         WHERE accession IN (SELECT unnest(?::VARCHAR[])) ORDER BY accession, owner_cik",
    )?;
    let owners = statement.query_map([listed()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            InsiderOwner {
                cik: row.get(1)?,
                name: row.get(2)?,
                director: row.get(3)?,
                officer: row.get(4)?,
                ten_percent_owner: row.get(5)?,
                other: row.get(6)?,
                officer_title: row.get(7)?,
            },
        ))
    })?;
    for owner in owners {
        let (accession, owner) = owner?;
        if let Some(report) = reports.get_mut(&accession) {
            report.owners.push(owner);
        }
    }
    let mut statement = connection.prepare_cached(
        "SELECT accession, security, date, code, acquired, shares, price, shares_after, direct \
         FROM insider_report_transaction \
         WHERE accession IN (SELECT unnest(?::VARCHAR[])) ORDER BY accession, line",
    )?;
    let trades = statement.query_map([listed()], |row| {
        let decimal = |index| -> duckdb::Result<Option<Decimal>> {
            Ok(row.get::<_, Option<Dec>>(index)?.map(|amount| amount.0))
        };
        Ok((
            row.get::<_, String>(0)?,
            InsiderTransaction {
                security: row.get(1)?,
                date: row.get(2)?,
                code: row.get(3)?,
                acquired: row.get(4)?,
                shares: decimal(5)?,
                price: decimal(6)?,
                shares_after: decimal(7)?,
                direct: row.get(8)?,
            },
        ))
    })?;
    for trade in trades {
        let (accession, trade) = trade?;
        if let Some(report) = reports.get_mut(&accession) {
            report.transactions.push(trade);
        }
    }
    Ok(reports)
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
        "WITH owners AS (SELECT symbol, accession, \
           string_agg(CAST(owner_cik AS VARCHAR), ',' ORDER BY owner_cik) AS ciks \
           FROM insider_owner GROUP BY symbol, accession), \
         filing AS (SELECT f.*, o.ciks FROM insider_filing f \
           LEFT JOIN owners o ON o.symbol = f.symbol AND o.accession = f.accession \
           WHERE {visible}) \
         SELECT f.symbol, f.accession, f.form, f.filed, f.accepted_at, t.security, t.date, \
         t.code, t.acquired, t.shares, t.price, t.shares_after, t.direct, p.plan_10b5_1 \
         FROM insider_transaction t \
         JOIN filing f ON f.symbol = t.symbol AND f.accession = t.accession \
         LEFT JOIN insider_plan p ON p.symbol = f.symbol AND p.accession = f.accession \
         WHERE NOT EXISTS (SELECT 1 FROM filing a JOIN insider_transaction r \
           ON r.symbol = a.symbol AND r.accession = a.accession \
           WHERE a.form = '4/A' AND a.symbol = f.symbol AND a.ciks = f.ciks \
           AND a.accepted_at > f.accepted_at AND r.security = t.security AND r.date = t.date \
           AND r.code = t.code AND r.acquired = t.acquired AND r.direct = t.direct) \
         ORDER BY f.symbol, f.accepted_at, f.accession, t.line"
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

macro_rules! delisting_notices {
    () => {
        "SELECT f.cik, f.accession, f.form, f.filed FROM sec_filing f \
         WHERE f.form IN ('25', '25-NSE') \
         QUALIFY f.cik IS DISTINCT FROM TRY_CAST(split_part(f.accession, '-', 1) AS UINTEGER) \
         OR NOT bool_or(f.cik IS DISTINCT FROM TRY_CAST(split_part(f.accession, '-', 1) AS UINTEGER)) \
         OVER (PARTITION BY f.accession)"
    };
}
const DELISTING_NOTICES: &str = delisting_notices!();
const LATEST_DELISTING_CHECKS: &str = concat!(
    "SELECT d.accession, d.ticker, d.listing, d.outcome, f.cik, f.filed \
     FROM delisting d JOIN (",
    delisting_notices!(),
    ") f ON f.accession = d.accession \
     QUALIFY row_number() OVER (PARTITION BY d.accession, d.ticker ORDER BY d.batch_id DESC, f.cik) = 1"
);

pub(crate) fn unresolved_delistings(
    connection: &Connection,
    from: NaiveDate,
) -> Result<Vec<SecFiling>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT f.cik, f.accession, f.form, f.filed FROM ({DELISTING_NOTICES}) f \
         WHERE f.filed >= ? \
         AND NOT EXISTS (SELECT 1 FROM delisting d WHERE d.accession = f.accession) \
         ORDER BY f.filed DESC, f.cik, f.accession"
    ))?;
    let notices = statement
        .query_map(params![from], |row| {
            Ok(SecFiling {
                cik: row.get(0)?,
                accession: row.get(1)?,
                form: row.get(2)?,
                filed: row.get(3)?,
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok(notices)
}

pub(crate) fn delisted_companies(connection: &Connection) -> Result<Vec<(Symbol, u32)>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT c.listing, any_value(c.cik ORDER BY c.filed DESC) FROM ({LATEST_DELISTING_CHECKS}) c \
         WHERE c.outcome IN ('held', 'stored') AND c.listing IS NOT NULL \
         GROUP BY c.listing ORDER BY max(c.filed) DESC, c.listing"
    ))?;
    let companies = statement
        .query_map([], |row| Ok((row.get::<_, Sym>(0)?.0, row.get(1)?)))?
        .collect::<duckdb::Result<_>>()?;
    Ok(companies)
}

pub(crate) fn pending_delistings(connection: &Connection) -> Result<Vec<PendingDelisting>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT ticker, filed, accession FROM ({LATEST_DELISTING_CHECKS}) \
         WHERE outcome = 'pending' ORDER BY filed DESC, ticker, accession"
    ))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Sym>(0)?.0,
                row.get::<_, NaiveDate>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let mut pending: Vec<PendingDelisting> = Vec::new();
    for (ticker, filed, accession) in rows {
        match pending.last_mut() {
            Some(last) if last.ticker == ticker && last.filed == filed => {
                last.accessions.push(accession);
            }
            _ => pending.push(PendingDelisting {
                ticker,
                filed,
                accessions: vec![accession],
            }),
        }
    }
    Ok(pending)
}

pub(crate) fn delisting_tally(
    connection: &Connection,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<DelistingTally> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT count(*), count(*) FILTER (WHERE NOT priced) FROM ( \
           SELECT cik, filed, bool_or(outcome IN ('held', 'stored')) AS priced \
           FROM ({LATEST_DELISTING_CHECKS}) WHERE ticker IS NOT NULL AND filed BETWEEN ? AND ? \
           GROUP BY cik, filed HAVING bool_and(outcome <> 'pending'))"
    ))?;
    Ok(statement.query_row(params![from, to], |row| {
        Ok(DelistingTally {
            delistings: row.get(0)?,
            unpriced: row.get(1)?,
        })
    })?)
}

pub(crate) fn delisting_progress(connection: &Connection) -> Result<AccessionProgress> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT count(*), count(*) FILTER (WHERE settled AND priced), \
           count(*) FILTER (WHERE settled AND NOT priced) FROM ( \
           SELECT f.accession, coalesce(bool_and(c.outcome <> 'pending'), false) AS settled, \
             coalesce(bool_or(c.outcome IN ('held', 'stored')), false) AS priced \
           FROM ({DELISTING_NOTICES}) f LEFT JOIN ({LATEST_DELISTING_CHECKS}) c ON c.accession = f.accession \
           GROUP BY f.accession)"
    ))?;
    Ok(statement.query_row([], |row| {
        Ok(AccessionProgress {
            expected: row.get(0)?,
            done: row.get(1)?,
            failed: row.get(2)?,
        })
    })?)
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

pub(crate) fn fails_to_deliver(
    connection: &Connection,
    calendar: &Calendar,
    spans: &[Span],
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<FailToDeliver>> {
    let Some(cutoff) = published_before(as_of, calendar) else {
        return Ok(Vec::new());
    };
    let tickers = span_tickers(spans);
    if tickers.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare_cached(&format!(
        "SELECT symbol, settlement, cusip, quantity, price, published FROM (SELECT *, \
         row_number() OVER (PARTITION BY symbol, settlement, cusip ORDER BY published, batch_id) \
         AS vintage FROM fails_to_deliver WHERE symbol {IN_SYMBOLS} AND settlement >= ? \
         AND published < ?) WHERE vintage = 1 ORDER BY settlement, symbol, cusip"
    ))?;
    let params: Params = vec![symbol_list(&tickers), Box::new(since), Box::new(cutoff)];
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            let published: NaiveDate = row.get(5)?;
            Ok(FailToDeliver {
                symbol: row.get::<_, Sym>(0)?.0,
                settlement: row.get(1)?,
                cusip: row.get(2)?,
                quantity: row.get(3)?,
                price: row
                    .get::<_, Option<Dec>>(4)?
                    .map(|price| price.0.normalize()),
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

pub(crate) fn fails_to_deliver_files(connection: &Connection) -> Result<BTreeSet<String>> {
    let mut statement = connection.prepare_cached("SELECT DISTINCT file FROM fails_to_deliver")?;
    let files = statement
        .query_map([], |row| row.get(0))?
        .collect::<duckdb::Result<BTreeSet<_>>>()?;
    Ok(files)
}

pub(crate) fn article_titles(
    connection: &Connection,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut statement =
        connection.prepare_cached("SELECT DISTINCT article, title FROM article_title")?;
    let mut titles: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for pair in statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
        let (article, title): (String, String) = pair?;
        titles.entry(article).or_default().insert(title);
    }
    Ok(titles)
}

pub(crate) fn page_views_through(connection: &Connection) -> Result<BTreeMap<String, NaiveDate>> {
    let mut statement =
        connection.prepare_cached("SELECT title, max(date) FROM page_views GROUP BY title")?;
    let through = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<duckdb::Result<BTreeMap<_, _>>>()?;
    Ok(through)
}

pub(crate) fn page_views(
    connection: &Connection,
    calendar: &Calendar,
    listings: &Listings,
    symbols: &[Symbol],
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<PageViews>> {
    let Some(cutoff) = published_before(as_of, calendar) else {
        return Ok(Vec::new());
    };
    let on = listed_on(calendar, as_of);
    let spans = spans(listings, symbols, on, since, as_of.exchange_date());
    let Some(history) = index_history(connection, StockIndex::Sp500)? else {
        return Ok(Vec::new());
    };
    let titles = article_titles(connection)?;
    let mut articles_of: BTreeMap<Symbol, BTreeSet<&String>> = BTreeMap::new();
    for member in &history.members {
        let inside = member
            .end
            .and_then(|end| end.pred_opt())
            .unwrap_or(NaiveDate::MAX);
        if let (Some(key), Some(article)) =
            (listings.listing(&member.symbol, inside), &member.article)
        {
            articles_of.entry(key).or_default().insert(article);
        }
    }
    let wanted: Vec<(Symbol, &String)> = resolve(listings, symbols, on)
        .into_iter()
        .flat_map(|(asked, key)| {
            articles_of
                .get(&key)
                .into_iter()
                .flatten()
                .map(move |article| (asked.clone(), *article))
        })
        .collect();
    let names: BTreeSet<&str> = wanted
        .iter()
        .filter_map(|(_, article)| titles.get(*article))
        .flatten()
        .map(String::as_str)
        .collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare_cached(
        "SELECT title, date, views, published FROM page_views \
         WHERE title IN (SELECT unnest(?::VARCHAR[])) AND date >= ? AND published <= ?",
    )?;
    let params: Params = vec![text_list(names), Box::new(since), Box::new(cutoff)];
    let mut by_title: BTreeMap<String, Vec<(NaiveDate, u64, NaiveDate)>> = BTreeMap::new();
    for row in statement.query_map(params_from_iter(params.iter()), |row| {
        Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?)))
    })? {
        let (title, day): (String, (NaiveDate, u64, NaiveDate)) = row?;
        by_title.entry(title).or_default().push(day);
    }
    let mut views = Vec::new();
    for (asked, article) in wanted {
        let mut days: BTreeMap<NaiveDate, (u64, NaiveDate)> = BTreeMap::new();
        let ranges: Vec<_> = spans
            .iter()
            .filter(|span| span.asked == asked)
            .map(|span| span.from.max(since)..=span.to)
            .collect();
        for (date, count, published) in titles
            .get(article)
            .into_iter()
            .flatten()
            .filter_map(|title| by_title.get(title))
            .flatten()
            .filter(|(date, _, _)| ranges.iter().any(|range| range.contains(date)))
        {
            let day = days.entry(*date).or_insert((0, *published));
            day.0 += count;
            day.1 = day.1.max(*published);
        }
        views.extend(
            days.into_iter()
                .map(|(date, (count, published))| PageViews {
                    symbol: asked.clone(),
                    article: article.clone(),
                    date,
                    views: count,
                    available_at: page_views_visible_at(published, calendar).unwrap_or(as_of),
                }),
        );
    }
    views.sort_by(|a, b| (&a.symbol, a.date, &a.article).cmp(&(&b.symbol, b.date, &b.article)));
    Ok(views)
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

pub(crate) fn option_quotes(
    connection: &Connection,
    spans: &[Span],
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<OptionQuote>> {
    let tickers = span_tickers(spans);
    if tickers.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare_cached(&format!(
        "SELECT q.contract, q.underlying, q.expiration, q.option_right, q.strike, q.session, \
         q.recorded_at, q.bid, q.ask, q.bid_size, q.ask_size, q.quoted_at, q.last, q.traded_at, \
         q.volume, q.open_interest, q.open_interest_date, q.implied_volatility, q.delta, \
         q.gamma, q.theta, q.vega, q.rho, b.created_at FROM option_quote q \
         JOIN ingest_batch b ON b.id = q.batch_id \
         WHERE q.underlying {IN_SYMBOLS} AND q.session >= ? AND b.created_at <= ? \
         ORDER BY q.session, q.underlying, q.expiration, q.strike, q.option_right"
    ))?;
    let params: Params = vec![symbol_list(&tickers), Box::new(since), Box::new(Ts(as_of))];
    let decimal = |value: Option<Dec>| value.map(|value| value.0.normalize());
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((
                row.get::<_, NaiveDate>(5)?,
                OptionQuote {
                    contract: row.get::<_, Sym>(0)?.0,
                    underlying: row.get::<_, Sym>(1)?.0,
                    expiration: row.get(2)?,
                    right: row.get::<_, Wire<_>>(3)?.0,
                    strike: row.get::<_, Dec>(4)?.0.normalize(),
                    at: row.get::<_, Ts>(6)?.0,
                    bid: decimal(row.get(7)?),
                    ask: decimal(row.get(8)?),
                    bid_size: row.get(9)?,
                    ask_size: row.get(10)?,
                    quoted_at: row.get::<_, Option<Ts>>(11)?.map(|at| at.0),
                    last: decimal(row.get(12)?),
                    traded_at: row.get::<_, Option<Ts>>(13)?.map(|at| at.0),
                    volume: row.get(14)?,
                    open_interest: row.get(15)?,
                    open_interest_date: row.get(16)?,
                    implied_volatility: decimal(row.get(17)?),
                    delta: decimal(row.get(18)?),
                    gamma: decimal(row.get(19)?),
                    theta: decimal(row.get(20)?),
                    vega: decimal(row.get(21)?),
                    rho: decimal(row.get(22)?),
                    available_at: row.get::<_, Ts>(23)?.0,
                },
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(within_spans(
        spans,
        rows,
        |(session, _)| *session,
        |(_, quote)| &mut quote.underlying,
    )
    .into_iter()
    .map(|(_, quote)| quote)
    .collect())
}

pub(crate) fn option_underlyings_on(
    connection: &Connection,
    session: NaiveDate,
) -> Result<BTreeSet<Symbol>> {
    let mut statement = connection
        .prepare_cached("SELECT DISTINCT underlying FROM option_quote WHERE session = ?")?;
    let underlyings = statement
        .query_map([session], |row| Ok(row.get::<_, Sym>(0)?.0))?
        .collect::<duckdb::Result<BTreeSet<_>>>()?;
    Ok(underlyings)
}

const HALT_COLUMNS: &str = "h.symbol, h.halted_at, h.name, h.market, h.reason, \
     h.pause_threshold, h.resumption_date, h.quotes_resume_at, h.trading_resumes_at";

fn halt_record(row: &duckdb::Row<'_>) -> duckdb::Result<TradingHaltRecord> {
    Ok(TradingHaltRecord {
        symbol: row.get::<_, Sym>(0)?.0,
        halted_at: row.get::<_, Ts>(1)?.0,
        name: row.get(2)?,
        market: row.get(3)?,
        reason: row.get(4)?,
        pause_threshold: row
            .get::<_, Option<Dec>>(5)?
            .map(|price| price.0.normalize()),
        resumption_date: row.get(6)?,
        quotes_resume_at: row.get::<_, Option<Ts>>(7)?.map(|at| at.0),
        trading_resumes_at: row.get::<_, Option<Ts>>(8)?.map(|at| at.0),
    })
}

pub(crate) fn latest_trading_halts(
    connection: &Connection,
    since: Timestamp,
) -> Result<BTreeSet<TradingHaltRecord>> {
    let mut statement = connection.prepare_cached(&format!(
        "SELECT {HALT_COLUMNS} FROM trading_halt h WHERE h.halted_at >= ? \
         QUALIFY row_number() OVER (PARTITION BY h.symbol, h.halted_at ORDER BY h.batch_id DESC) = 1"
    ))?;
    let halts = statement
        .query_map(params![Ts(since)], halt_record)?
        .collect::<duckdb::Result<BTreeSet<_>>>()?;
    Ok(halts)
}

pub(crate) fn trading_halts(
    connection: &Connection,
    spans: Option<&[Span]>,
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<Vec<TradingHalt>> {
    let Some(from) = Timestamp::at_exchange_time(since, NaiveTime::MIN) else {
        return Ok(Vec::new());
    };
    let tickers = spans.map(span_tickers);
    if tickers.as_ref().is_some_and(Vec::is_empty) {
        return Ok(Vec::new());
    }
    let only = if tickers.is_some() {
        format!("AND h.symbol {IN_SYMBOLS}")
    } else {
        String::new()
    };
    let mut statement = connection.prepare_cached(&format!(
        "SELECT {HALT_COLUMNS}, b.created_at FROM trading_halt h \
         JOIN ingest_batch b ON b.id = h.batch_id \
         WHERE h.halted_at >= ? AND b.created_at <= ? {only} \
         ORDER BY h.halted_at, h.symbol, b.created_at"
    ))?;
    let mut params: Params = vec![Box::new(Ts(from)), Box::new(Ts(as_of))];
    if let Some(tickers) = &tickers {
        params.push(symbol_list(tickers));
    }
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            let record = halt_record(row)?;
            Ok(TradingHalt {
                symbol: record.symbol,
                halted_at: record.halted_at,
                name: record.name,
                market: record.market,
                reason: record.reason,
                pause_threshold: record.pause_threshold,
                resumption_date: record.resumption_date,
                quotes_resume_at: record.quotes_resume_at,
                trading_resumes_at: record.trading_resumes_at,
                available_at: row.get::<_, Ts>(9)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(match spans {
        Some(spans) => within_spans(
            spans,
            rows,
            |row| row.halted_at.exchange_date(),
            |row| &mut row.symbol,
        ),
        None => rows,
    })
}

pub(crate) fn latest_etf_dates(connection: &Connection) -> Result<BTreeMap<Symbol, EtfDates>> {
    let mut statement = connection.prepare_cached(
        "SELECT etf, max(holdings), max(shares) FROM ( \
         SELECT etf, max(as_of) AS holdings, NULL::DATE AS shares FROM etf_holding GROUP BY etf \
         UNION ALL SELECT etf, NULL::DATE, max(date) FROM etf_shares GROUP BY etf) GROUP BY etf",
    )?;
    let dates = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Sym>(0)?.0,
                EtfDates {
                    holdings: row.get(1)?,
                    shares: row.get(2)?,
                },
            ))
        })?
        .collect::<duckdb::Result<BTreeMap<_, _>>>()?;
    Ok(dates)
}

pub(crate) fn etf_holdings(
    connection: &Connection,
    spans: &[Span],
    since: NaiveDate,
    as_of: Timestamp,
) -> Result<(Vec<EtfHolding>, Vec<EtfShares>)> {
    let tickers = span_tickers(spans);
    if tickers.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut statement = connection.prepare_cached(&format!(
        "SELECT h.etf, h.as_of, h.name, h.ticker, h.identifier, h.weight, h.quantity, \
         b.created_at FROM etf_holding h JOIN ingest_batch b ON b.id = h.batch_id \
         WHERE h.etf {IN_SYMBOLS} AND h.as_of >= ? AND b.created_at <= ? \
         ORDER BY h.as_of, h.etf, h.weight DESC, h.name"
    ))?;
    let params: Params = vec![symbol_list(&tickers), Box::new(since), Box::new(Ts(as_of))];
    let holdings = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(EtfHolding {
                etf: row.get::<_, Sym>(0)?.0,
                as_of: row.get(1)?,
                name: row.get(2)?,
                ticker: row.get::<_, Option<Sym>>(3)?.map(|ticker| ticker.0),
                identifier: row.get(4)?,
                weight: row.get::<_, Dec>(5)?.0.normalize(),
                quantity: row
                    .get::<_, Option<Dec>>(6)?
                    .map(|quantity| quantity.0.normalize()),
                available_at: row.get::<_, Ts>(7)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let mut statement = connection.prepare_cached(&format!(
        "SELECT s.etf, s.date, s.nav, s.shares_outstanding, s.net_assets, b.created_at \
         FROM etf_shares s JOIN ingest_batch b ON b.id = s.batch_id \
         WHERE s.etf {IN_SYMBOLS} AND s.date >= ? AND b.created_at <= ? \
         ORDER BY s.date, s.etf, b.created_at"
    ))?;
    let shares = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(EtfShares {
                etf: row.get::<_, Sym>(0)?.0,
                date: row.get(1)?,
                nav: row.get::<_, Dec>(2)?.0.normalize(),
                shares_outstanding: row.get::<_, Dec>(3)?.0.normalize(),
                net_assets: row.get::<_, Dec>(4)?.0.normalize(),
                available_at: row.get::<_, Ts>(5)?.0,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok((
        within_spans(spans, holdings, |row| row.as_of, |row| &mut row.etf),
        within_spans(spans, shares, |row| row.date, |row| &mut row.etf),
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

pub(crate) type Vintage = (NaiveDate, NaiveDate, Option<Decimal>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MacroSource {
    Rates,
    Observations(MacroSeries),
    Estimates(MacroSeries),
}

impl MacroSource {
    fn series(self) -> MacroSeries {
        match self {
            Self::Rates => MacroSeries::Treasury3m,
            Self::Observations(series) | Self::Estimates(series) => series,
        }
    }
}

pub(crate) fn macro_vintages(
    connection: &Connection,
    source: MacroSource,
    dates: Option<(NaiveDate, NaiveDate)>,
) -> Result<Vec<Vintage>> {
    let within = |column: &str| match dates {
        Some(_) => format!("AND {column} BETWEEN ? AND ?"),
        None => String::new(),
    };
    let (sql, mut params): (String, Params) = match source {
        MacroSource::Rates => (
            format!(
                "SELECT date, date, percent FROM rate_daily WHERE true {} ORDER BY date",
                within("date")
            ),
            Params::new(),
        ),
        MacroSource::Observations(series) => (
            format!(
                "SELECT date, published, value FROM macro_observation WHERE series = ? {} \
                 ORDER BY date, published",
                within("date")
            ),
            vec![Box::new(wire_name(&series)) as Box<dyn ToSql>],
        ),
        MacroSource::Estimates(series) => {
            let Some((metric, scope)) = series.estimate() else {
                return Ok(Vec::new());
            };
            (
                format!(
                    "SELECT r.issue_date, r.published, f.value FROM earnings_insight_fact f \
                     JOIN earnings_insight r USING (report) WHERE f.metric = ? AND f.scope = ? \
                     AND f.period = '' {} ORDER BY r.issue_date, r.published, r.report",
                    within("r.issue_date")
                ),
                vec![
                    Box::new(wire_name(&metric)) as Box<dyn ToSql>,
                    Box::new(wire_name(&scope)),
                ],
            )
        }
    };
    if let Some((from, to)) = dates {
        params.push(Box::new(from));
        params.push(Box::new(to));
    }
    let normalize = matches!(source, MacroSource::Estimates(_));
    let mut statement = connection.prepare_cached(&sql)?;
    let vintages = statement
        .query_map(params_from_iter(params.iter()), |row| {
            let value = row
                .get::<_, Option<Dec>>(2)?
                .map(|Dec(value)| if normalize { value.normalize() } else { value });
            Ok((row.get(0)?, row.get(1)?, value))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(vintages)
}

pub(crate) fn macro_points(
    connection: &Connection,
    calendar: &Calendar,
    series: &[MacroSeries],
    from: NaiveDate,
    to: NaiveDate,
    as_of: Timestamp,
    mut recall: Option<&mut Store>,
) -> Result<Vec<MacroPoint>> {
    let Some(cutoff) = published_before(as_of, calendar) else {
        return Ok(Vec::new());
    };
    let wanted: BTreeSet<MacroSeries> = series.iter().copied().collect();
    let mut sources = Vec::new();
    if wanted.contains(&MacroSeries::Treasury3m) {
        sources.push(MacroSource::Rates);
    }
    sources.extend(
        wanted
            .iter()
            .filter(|id| **id != MacroSeries::Treasury3m)
            .map(|id| MacroSource::Observations(*id)),
    );
    sources.extend(
        wanted
            .iter()
            .filter(|id| id.estimate().is_some())
            .map(|id| MacroSource::Estimates(*id)),
    );
    let mut points = Vec::new();
    for source in sources {
        let vintages = match recall.as_deref_mut() {
            Some(recall) => recall.vintages(connection, source)?,
            None => Arc::new(macro_vintages(connection, source, Some((from, to)))?),
        };
        let first = vintages.partition_point(|(date, _, _)| *date < from);
        let mut known: Vec<Vintage> = Vec::new();
        for vintage in vintages[first..]
            .iter()
            .take_while(|(date, _, _)| *date <= to)
            .filter(|(_, published, _)| *published < cutoff)
        {
            match known.last_mut() {
                Some(last) if last.0 == vintage.0 => *last = *vintage,
                _ => known.push(*vintage),
            }
        }
        points.extend(known.into_iter().filter_map(|(date, published, value)| {
            Some(MacroPoint {
                series: source.series(),
                date,
                value: value?,
                published,
                available_at: published_visible_at(published, calendar)?,
            })
        }));
    }
    Ok(points)
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

pub(crate) fn ex_date_prices(
    connection: &Connection,
    wanted: &[(Symbol, NaiveDate)],
) -> Result<BTreeMap<(Symbol, NaiveDate), (Decimal, Decimal)>> {
    if wanted.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut statement = connection.prepare_cached(
        "WITH wanted AS (SELECT unnest(?::VARCHAR[]) AS symbol, \
           unnest(?::VARCHAR[])::DATE AS ex_date), \
         near AS (SELECT w.symbol, w.ex_date, b.date, b.open, b.close FROM wanted w \
           JOIN bar_1d b ON b.symbol = w.symbol \
           AND b.date BETWEEN w.ex_date - ? * 2 AND w.ex_date + ?), \
         priced AS (SELECT symbol, ex_date, arg_max(close, date) FILTER (date < ex_date) AS close, \
           arg_min(open, date) FILTER (date >= ex_date) AS open FROM near GROUP BY symbol, ex_date) \
         SELECT symbol, ex_date, close, open FROM priced WHERE close > 0 AND open > 0",
    )?;
    let params: Params = vec![
        text_list(wanted.iter().map(|(symbol, _)| symbol.as_str())),
        Box::new(Value::List(
            wanted
                .iter()
                .map(|(_, date)| Value::Text(date.to_string()))
                .collect(),
        )),
        Box::new(EX_DATE_REACH_DAYS * 2),
        Box::new(EX_DATE_REACH_DAYS),
    ];
    let prices = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((
                (row.get::<_, Sym>(0)?.0, row.get::<_, NaiveDate>(1)?),
                (row.get::<_, Dec>(2)?.0, row.get::<_, Dec>(3)?.0),
            ))
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok(prices)
}

pub(crate) fn split_like_jumps(
    connection: &Connection,
    traded_at_least: Decimal,
) -> Result<Vec<(Symbol, NaiveDate)>> {
    let mut statement = connection.prepare_cached(
        "WITH b AS (SELECT symbol, date, open, lag(close) OVER w AS previous, \
           avg(close * volume) OVER (w ROWS BETWEEN 20 PRECEDING AND 1 PRECEDING) AS traded \
           FROM bar_1d WHERE NOT contains(symbol, '@') WINDOW w AS (PARTITION BY symbol ORDER BY date)), \
         jump AS (SELECT symbol, date, abs(ln(previous / open)) AS moved, traded FROM b \
           WHERE previous > 0 AND open > 0 AND traded >= ? AND abs(ln(previous / open)) > ln(1.4)), \
         ratio AS (SELECT ln(unnest([2, 3, 4, 5, 8, 10, 15, 20, 25, 30, 40, 50, 100])) AS logged) \
         SELECT j.symbol, j.date FROM jump j \
         WHERE EXISTS (SELECT 1 FROM ratio r WHERE abs(j.moved - r.logged) < 0.02) \
         AND NOT EXISTS (SELECT 1 FROM corporate_action a WHERE a.symbol = j.symbol \
           AND a.ex_date BETWEEN j.date - 5 AND j.date + 5 \
           AND a.kind IN ('split', 'stock_dividend', 'spin_off', 'stock_merger')) \
         ORDER BY j.traded DESC, j.symbol, j.date",
    )?;
    let jumps = statement
        .query_map(params![Dec(traded_at_least)], |row| {
            Ok((row.get::<_, Sym>(0)?.0, row.get::<_, NaiveDate>(1)?))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(jumps)
}

pub(crate) fn long_silences(
    connection: &Connection,
    sessions: i64,
) -> Result<Vec<(Symbol, NaiveDate, NaiveDate)>> {
    let mut statement = connection.prepare_cached(
        "WITH session AS (SELECT date, row_number() OVER (ORDER BY date) AS ordinal FROM calendar), \
         event AS (SELECT symbol, date, true AS traded FROM bar_1d WHERE NOT contains(symbol, '@') \
           UNION ALL SELECT other_symbol, ex_date, false FROM corporate_action \
           WHERE kind = 'name_change' AND other_symbol <> symbol AND NOT contains(other_symbol, '@')), \
         apart AS (SELECT e.symbol, lag(e.date) OVER w AS last, e.date AS resumed, \
           lag(e.traded) OVER w AS traded, s.ordinal - lag(s.ordinal) OVER w AS sessions \
           FROM event e JOIN session s USING (date) \
           WINDOW w AS (PARTITION BY e.symbol ORDER BY e.date, e.traded)) \
         SELECT a.symbol, a.last, min(a.resumed) FROM apart a WHERE a.traded AND a.sessions > ? + 1 \
         AND (NOT EXISTS (SELECT 1 FROM instrument i \
           WHERE i.symbol = a.symbol || '@' || strftime(a.last, '%Y-%m-%d')) \
         OR EXISTS (SELECT 1 FROM quiet_session q WHERE q.symbol = a.symbol AND q.date < a.resumed)) \
         GROUP BY a.symbol, a.last ORDER BY a.symbol, a.last",
    )?;
    let ended = statement
        .query_map(params![sessions], |row| {
            Ok((
                row.get::<_, Sym>(0)?.0,
                row.get::<_, NaiveDate>(1)?,
                row.get::<_, NaiveDate>(2)?,
            ))
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
        .between(&symbols, from, to, |_| true)
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

pub(crate) fn nordic_listed(
    connection: &Connection,
    source: DataSource,
) -> Result<Vec<NordicListing>> {
    let mut statement = connection.prepare_cached(
        "SELECT l.listing, l.mic, l.isin, l.symbol, l.name, l.currency, l.sector \
         FROM nordic_listing l JOIN nordic_listing_seen s USING (source, listing) \
         WHERE l.source = $1 \
         AND s.date = (SELECT max(date) FROM nordic_listing_seen WHERE source = $1) \
         QUALIFY row_number() OVER (PARTITION BY l.listing ORDER BY l.recorded_at DESC, l.batch_id DESC) = 1 \
         ORDER BY l.listing",
    )?;
    let listings = statement
        .query_map([wire_name(&source)], |row| {
            Ok(NordicListing {
                listing: row.get::<_, Sym>(0)?.0,
                mic: row.get(1)?,
                isin: row.get(2)?,
                symbol: row.get(3)?,
                name: row.get(4)?,
                currency: row.get(5)?,
                sector: row.get(6)?,
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok(listings)
}

pub(crate) fn nordic_bar_dates(
    connection: &Connection,
    source: DataSource,
) -> Result<BTreeMap<Symbol, NaiveDate>> {
    let mut statement = connection.prepare_cached(
        "SELECT listing, max(date) FROM nordic_bar_1d WHERE source = ? GROUP BY listing",
    )?;
    let dates = statement
        .query_map([wire_name(&source)], |row| {
            Ok((row.get::<_, Sym>(0)?.0, row.get(1)?))
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok(dates)
}

pub(crate) fn nordic_history(
    connection: &Connection,
    source: DataSource,
    listing: &Symbol,
    since: NaiveDate,
) -> Result<(Vec<StoredNordicBar>, Vec<NordicSplit>)> {
    let source = wire_name(&source);
    let mut statement = connection.prepare_cached(
        "SELECT date, open, high, low, close, volume, recorded_at FROM nordic_bar_1d \
         WHERE source = ? AND listing = ? AND date >= ? \
         QUALIFY row_number() OVER (PARTITION BY date ORDER BY recorded_at DESC, batch_id DESC) = 1 \
         ORDER BY date",
    )?;
    let bars = statement
        .query_map(params![source, listing.as_str(), since], |row| {
            Ok(StoredNordicBar {
                bar: NordicBar {
                    date: row.get(0)?,
                    open: row.get::<_, Dec>(1)?.0,
                    high: row.get::<_, Dec>(2)?.0,
                    low: row.get::<_, Dec>(3)?.0,
                    close: row.get::<_, Dec>(4)?.0,
                    volume: row.get(5)?,
                },
                recorded_at: row.get::<_, Ts>(6)?.0,
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    let mut statement = connection.prepare_cached(
        "SELECT date, ratio, detected_at FROM nordic_split \
         WHERE source = ? AND listing = ? ORDER BY detected_at, date",
    )?;
    let splits = statement
        .query_map(params![source, listing.as_str()], |row| {
            Ok(NordicSplit {
                date: row.get(0)?,
                ratio: row.get::<_, Dec>(1)?.0,
                detected_at: row.get::<_, Ts>(2)?.0,
            })
        })?
        .collect::<duckdb::Result<_>>()?;
    Ok((bars, splits))
}

pub(crate) fn fx_rate_dates(connection: &Connection) -> Result<BTreeMap<String, NaiveDate>> {
    let mut statement =
        connection.prepare_cached("SELECT currency, max(date) FROM fx_rate GROUP BY currency")?;
    let dates = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<duckdb::Result<_>>()?;
    Ok(dates)
}
