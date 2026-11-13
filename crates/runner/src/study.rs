use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{Adjustment, Calendar, Session, StockIndex, Symbol, Timeframe};
use buizz_core::run::{CostModel, UNIVERSE_SESSIONS, UniverseSpec};
use buizz_core::study::{
    BROAD_BENCHMARK, CurvePoint, EventOutcome, EventStudy, HorizonReturn, HorizonStats,
    INDEX_BENCHMARK, STUDY_HORIZONS, STUDY_POSITION, STUDY_SESSIONS, StudyEvent, StudyRequest,
    StudySkip, overlap_months, round_trip_cost,
};
use buizz_core::time::Timestamp;
use buizz_core::visibility::{open_auction_session, published_before};
use buizz_market::{HistoryRequest, MarketStore};
use buizz_metrics::{abnormal_stats, luck_t};
use chrono::{Datelike, NaiveDate, TimeDelta};
use futures::{StreamExt, TryStreamExt, stream};
use serde_json::json;

use crate::{Runner, RunnerError};

const CONCURRENT_READS: usize = 4;
const CALENDAR_MARGIN_DAYS: i64 = 14;

#[derive(Clone, Copy)]
struct Prices {
    raw_open: f64,
    open: f64,
    close: f64,
}

type Series = BTreeMap<NaiveDate, Prices>;

struct Planned {
    outcome: usize,
    key: Symbol,
    benchmark: Symbol,
    entry: usize,
    last: usize,
    last_session: Option<NaiveDate>,
}

#[derive(Default)]
struct Horizon {
    gross: Vec<(i32, f64)>,
    net: Vec<(i32, f64)>,
    delisted: u32,
}

#[derive(Default)]
struct CurveSum {
    count: u32,
    gross: f64,
    net: f64,
}

impl Runner {
    pub async fn event_study(&self, request: StudyRequest) -> Result<EventStudy, RunnerError> {
        let label = request.label.trim().to_owned();
        if label.is_empty() {
            return Err(RunnerError::InvalidRequest("a study needs a label".into()));
        }
        let cost = request.cost.clone().unwrap_or_default();
        if let Some(reason) = cost.invalid() {
            return Err(RunnerError::InvalidRequest(reason.into()));
        }
        let market = &self.inner.market;
        let Some(calendar) = study_calendar(market, &request.events).await? else {
            return Err(RunnerError::InvalidRequest(
                "a study needs at least one event".into(),
            ));
        };
        let data_through = market.watermarks().borrow().day;
        let measured = calendar.sessions().partition_point(|session| {
            data_through.is_some_and(|through| session.close <= through)
        });
        let (mut outcomes, planned) = self.plan(&request, &calendar, measured).await?;
        let series = read_series(market, &calendar, &planned).await?;
        let mut horizons: Vec<Horizon> =
            STUDY_HORIZONS.iter().map(|_| Horizon::default()).collect();
        let mut curve: Vec<CurveSum> = (0..STUDY_SESSIONS).map(|_| CurveSum::default()).collect();
        let sessions = calendar.sessions();
        for plan in &planned {
            let outcome = &mut outcomes[plan.outcome];
            let entry = sessions[plan.entry].date;
            let Some((stock, round_trip)) = series.get(&plan.key).and_then(|stock| {
                let entered = stock.get(&entry)?;
                Some((stock, round_trip_cost(&cost, entered.raw_open)?))
            }) else {
                outcome.skipped = Some(StudySkip::NotTraded);
                continue;
            };
            let Some(benchmark) = series
                .get(&plan.benchmark)
                .filter(|benchmark| benchmark.contains_key(&entry))
            else {
                outcome.skipped = Some(StudySkip::BenchmarkNotTraded);
                continue;
            };
            outcome.round_trip_cost = Some(round_trip);
            let month = entry.year() * 12 + i32::try_from(entry.month0()).unwrap_or(0);
            let mut exited = entry;
            for (offset, sum) in (0..=plan.last - plan.entry).zip(&mut curve) {
                let exit = sessions[plan.entry + offset].date;
                if !benchmark.contains_key(&exit) {
                    break;
                }
                let (Some(stock), Some(benchmark)) = (
                    window_return(stock, entry, exit),
                    window_return(benchmark, entry, exit),
                ) else {
                    break;
                };
                exited = exit;
                let gross = stock - benchmark;
                let net = gross - round_trip;
                sum.count += 1;
                sum.gross += gross;
                sum.net += net;
                let held = u32::try_from(offset + 1).unwrap_or(u32::MAX);
                if let Some(index) = STUDY_HORIZONS.iter().position(|sessions| *sessions == held) {
                    outcome.returns[index] = Some(HorizonReturn { stock, benchmark });
                    let horizon = &mut horizons[index];
                    horizon.gross.push((month, gross));
                    horizon.net.push((month, net));
                    if plan.last_session.is_some_and(|last| last < exit) {
                        horizon.delisted += 1;
                    }
                }
            }
            outcome.last_session = plan.last_session.filter(|last| *last < exited);
        }
        let hash = study_hash(&request, &cost)?;
        let trials = self
            .inner
            .state
            .record_study(
                &label,
                &hash,
                u32::try_from(request.events.len()).unwrap_or(u32::MAX),
                Timestamp::now(),
            )
            .await?;
        Ok(EventStudy {
            label,
            hash,
            trials,
            luck_t: luck_t(trials),
            data_through,
            cost,
            position: STUDY_POSITION,
            horizons: STUDY_HORIZONS
                .iter()
                .zip(horizons)
                .map(|(sessions, horizon)| horizon_stats(*sessions, &horizon))
                .collect(),
            curve: (1..)
                .zip(curve)
                .filter(|(_, sum)| sum.count > 0)
                .map(|(session, sum)| CurvePoint {
                    session,
                    count: sum.count,
                    gross: sum.gross / f64::from(sum.count),
                    net: sum.net / f64::from(sum.count),
                })
                .collect(),
            events: outcomes,
        })
    }

    async fn plan(
        &self,
        request: &StudyRequest,
        calendar: &Calendar,
        measured: usize,
    ) -> Result<(Vec<EventOutcome>, Vec<Planned>), RunnerError> {
        let market = &self.inner.market;
        let listings = market.listings();
        let last_sessions: BTreeMap<Symbol, NaiveDate> = market
            .instruments()
            .await?
            .into_iter()
            .filter_map(|instrument| Some((instrument.symbol, instrument.last_session?)))
            .collect();
        let sessions = calendar.sessions();
        let mut members: BTreeMap<NaiveDate, BTreeSet<Symbol>> = BTreeMap::new();
        let mut outcomes = Vec::with_capacity(request.events.len());
        let mut planned = Vec::new();
        for event in &request.events {
            let mut outcome = EventOutcome {
                symbol: event.symbol.clone(),
                at: event.at,
                entry: None,
                benchmark: None,
                round_trip_cost: None,
                last_session: None,
                skipped: None,
                returns: vec![None; STUDY_HORIZONS.len()],
            };
            let (Some(named_on), Some(session)) = (
                published_before(event.at, calendar),
                open_auction_session(event.at, calendar),
            ) else {
                outcome.skipped = Some(StudySkip::NoSession);
                outcomes.push(outcome);
                continue;
            };
            let entry = sessions.partition_point(|earlier| earlier.date < session.date);
            outcome.entry = Some(session.date);
            let Some(key) = listings.listing(&event.symbol, named_on) else {
                outcome.skipped = Some(StudySkip::UnknownSymbol);
                outcomes.push(outcome);
                continue;
            };
            let benchmark = match &request.benchmark {
                Some(benchmark) => benchmark.clone(),
                None => {
                    let member = match members.get(&session.date) {
                        Some(keys) => keys.contains(&key),
                        None => {
                            let keys: BTreeSet<Symbol> = market
                                .universe(
                                    &UniverseSpec::Index {
                                        index: StockIndex::Sp500,
                                    },
                                    session.date,
                                    UNIVERSE_SESSIONS,
                                )
                                .await?
                                .iter()
                                .filter_map(|ticker| listings.listing(ticker, session.date))
                                .collect();
                            let member = keys.contains(&key);
                            members.insert(session.date, keys);
                            member
                        }
                    };
                    Symbol::new(if member {
                        INDEX_BENCHMARK
                    } else {
                        BROAD_BENCHMARK
                    })
                }
            };
            outcome.benchmark = Some(benchmark.clone());
            let last_session = last_sessions.get(&key).copied();
            if last_session.is_some_and(|last| last < session.date) {
                outcome.skipped = Some(StudySkip::NotTraded);
            } else if entry < measured {
                planned.push(Planned {
                    outcome: outcomes.len(),
                    key,
                    benchmark,
                    entry,
                    last: (entry + STUDY_SESSIONS as usize - 1).min(measured - 1),
                    last_session,
                });
            }
            outcomes.push(outcome);
        }
        Ok((outcomes, planned))
    }
}

async fn study_calendar(
    market: &MarketStore,
    events: &[StudyEvent],
) -> Result<Option<Calendar>, RunnerError> {
    let dates = events.iter().map(|event| event.at.exchange_date());
    let (Some(first), Some(last)) = (dates.clone().min(), dates.max()) else {
        return Ok(None);
    };
    let ahead = TimeDelta::days(i64::from(STUDY_SESSIONS) * 2 + CALENDAR_MARGIN_DAYS);
    let from = first - TimeDelta::days(CALENDAR_MARGIN_DAYS);
    Ok(Some(market.calendar(from, last + ahead).await?))
}

async fn read_series(
    market: &MarketStore,
    calendar: &Calendar,
    planned: &[Planned],
) -> Result<BTreeMap<Symbol, Series>, RunnerError> {
    let sessions = calendar.sessions();
    let mut windows: BTreeMap<Symbol, (usize, Timestamp)> = BTreeMap::new();
    let mut widen = |symbol: &Symbol, entry: usize, through: Timestamp| {
        let window = windows.entry(symbol.clone()).or_insert((entry, through));
        *window = (window.0.min(entry), window.1.max(through));
    };
    for plan in planned {
        let through = sessions[plan.last].close;
        let delisted = plan
            .last_session
            .and_then(|last| calendar.session(last))
            .map_or(through, |last| last.close.min(through));
        widen(&plan.key, plan.entry, delisted);
        widen(&plan.benchmark, plan.entry, through);
    }
    stream::iter(windows)
        .map(|(symbol, (entry, through))| async move {
            let from = &sessions[entry];
            let series = prices(market, calendar, &symbol, from, through).await?;
            Ok::<_, RunnerError>((symbol, series))
        })
        .buffer_unordered(CONCURRENT_READS)
        .try_collect()
        .await
}

async fn prices(
    market: &MarketStore,
    calendar: &Calendar,
    symbol: &Symbol,
    from: &Session,
    as_of: Timestamp,
) -> Result<Series, RunnerError> {
    let bars = calendar.between(from.date, as_of.exchange_date()).len();
    let read = |adjust| {
        market.history(HistoryRequest {
            symbols: vec![symbol.clone()],
            timeframe: Timeframe::Day,
            bars: u32::try_from(bars).unwrap_or(u32::MAX),
            adjust,
            as_of,
        })
    };
    let (raw, adjusted) = futures::try_join!(read(Adjustment::Raw), read(Adjustment::All))?;
    let raw_opens: BTreeMap<NaiveDate, f64> = raw
        .into_iter()
        .map(|bar| (bar.start.exchange_date(), bar.open))
        .collect();
    Ok(adjusted
        .into_iter()
        .filter_map(|bar| {
            let date = bar.start.exchange_date();
            Some((
                date,
                Prices {
                    raw_open: *raw_opens.get(&date)?,
                    open: bar.open,
                    close: bar.close,
                },
            ))
        })
        .collect())
}

fn window_return(series: &Series, entry: NaiveDate, exit: NaiveDate) -> Option<f64> {
    let entered = series.get(&entry)?;
    let (_, last) = series.range(entry..=exit).next_back()?;
    Some(last.close / entered.open - 1.0)
}

fn horizon_stats(sessions: u32, horizon: &Horizon) -> HorizonStats {
    let months: BTreeSet<i32> = horizon.gross.iter().map(|(month, _)| *month).collect();
    let lags = overlap_months(sessions);
    HorizonStats {
        sessions,
        count: u32::try_from(horizon.gross.len()).unwrap_or(u32::MAX),
        delisted: horizon.delisted,
        months: u32::try_from(months.len()).unwrap_or(u32::MAX),
        gross: abnormal_stats(&horizon.gross, lags),
        net: abnormal_stats(&horizon.net, lags),
    }
}

fn study_hash(request: &StudyRequest, cost: &CostModel) -> Result<String, RunnerError> {
    let mut events: Vec<&StudyEvent> = request.events.iter().collect();
    events.sort_by(|left, right| (left.at, &left.symbol).cmp(&(right.at, &right.symbol)));
    let settings = json!({
        "events": events,
        "benchmark": request.benchmark,
        "cost": cost,
        "horizons": STUDY_HORIZONS,
        "position": STUDY_POSITION,
    });
    let bytes = serde_json::to_vec(&settings)
        .map_err(|error| RunnerError::InvalidRequest(error.to_string()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}
