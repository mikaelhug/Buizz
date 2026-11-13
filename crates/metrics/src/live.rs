use std::collections::BTreeMap;
use std::f64::consts::SQRT_2;

use buizz_core::api::{LiveSummary, RunSummary, SparkPoint};
use buizz_core::money::round_cash;
use buizz_core::performance::{
    Envelope, EnvelopeBreach, LivePoint, LiveStats, LiveVerdict, SharpeMoments,
};
use buizz_core::run::{EquityPoint, RunId};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};

use crate::envelope::Backtest;
use crate::{
    MIN_DAILY_VOLATILITY, TRADING_DAYS, beta, max_drawdown, mean, sample_stdev, simple_returns,
};

const CURVE_POINTS: usize = 60;
const PROVEN_ALPHA: f64 = 0.05;
const QUANTILE_STEPS: u32 = 200;
const QUANTILE_BOUND: f64 = 40.0;
const VOLATILITY_LIMIT: f64 = 1.5;

pub struct LiveInput<'a> {
    pub start_cash: Decimal,
    pub equity: &'a [EquityPoint],
    pub benchmark: &'a [SparkPoint],
    pub paused: &'a [(Timestamp, Option<Timestamp>)],
    pub source: Option<Source<'a>>,
    pub started: u32,
}

pub struct Source<'a> {
    pub run: RunId,
    pub start_cash: Decimal,
    pub equity: &'a [EquityPoint],
    pub benchmark: &'a [SparkPoint],
}

struct Track<'a> {
    sessions: Vec<(&'a EquityPoint, Decimal)>,
    base: &'a SparkPoint,
    held: Vec<f64>,
    index: Vec<f64>,
}

fn track<'a>(
    start_cash: Decimal,
    equity: &'a [EquityPoint],
    benchmark: &'a [SparkPoint],
) -> Option<Track<'a>> {
    let benchmark: BTreeMap<NaiveDate, &SparkPoint> = benchmark
        .iter()
        .map(|point| (point.at.exchange_date(), point))
        .collect();
    let closes: BTreeMap<NaiveDate, &EquityPoint> = equity
        .iter()
        .map(|point| (point.at.exchange_date(), point))
        .collect();
    let sessions: Vec<(&EquityPoint, Decimal)> = closes
        .iter()
        .filter_map(|(date, point)| Some((*point, benchmark.get(date)?.equity)))
        .collect();
    let (first, _) = sessions.first()?;
    let (_, base) = benchmark.range(..first.at.exchange_date()).next_back()?;
    if base.equity <= Decimal::ZERO || start_cash <= Decimal::ZERO {
        return None;
    }
    let held: Vec<f64> = std::iter::once(start_cash)
        .chain(sessions.iter().map(|(point, _)| point.equity))
        .map(|value| value.to_f64().unwrap_or(f64::NAN))
        .collect();
    let index: Vec<f64> = std::iter::once(base.equity)
        .chain(sessions.iter().map(|(_, close)| *close))
        .map(|value| value.to_f64().unwrap_or(f64::NAN))
        .collect();
    Some(Track {
        sessions,
        base,
        held,
        index,
    })
}

fn envelope(run: RunId, backtest: &Backtest, live: &LiveStats) -> Envelope {
    let age = live.sessions as usize;
    let excess_floor = backtest.excess_floor(age);
    let drawdown_ceiling = backtest.drawdown_ceiling(age);
    let backtest_volatility = backtest.volatility();
    let breaches = [
        (
            EnvelopeBreach::Excess,
            excess_floor.is_some_and(|floor| live.excess_return < floor),
        ),
        (
            EnvelopeBreach::Drawdown,
            drawdown_ceiling.is_some_and(|ceiling| live.max_drawdown > ceiling),
        ),
        (
            EnvelopeBreach::Volatility,
            live.volatility
                .zip(backtest_volatility)
                .is_some_and(|(live, backtest)| live > backtest * VOLATILITY_LIMIT),
        ),
    ];
    Envelope {
        source: run,
        source_sessions: u32::try_from(backtest.sessions()).unwrap_or(u32::MAX),
        windows: u32::try_from(backtest.windows(age)).unwrap_or(u32::MAX),
        excess_floor,
        drawdown_ceiling,
        volatility: backtest_volatility,
        breaches: breaches
            .into_iter()
            .filter_map(|(breach, failed)| failed.then_some(breach))
            .collect(),
    }
}

pub fn live(input: &LiveInput<'_>) -> Option<LiveStats> {
    let Track {
        sessions,
        base,
        held,
        index,
    } = track(input.start_cash, input.equity, input.benchmark)?;
    let returns = simple_returns(&held)?;
    let market = simple_returns(&index)?;
    let excess: Vec<f64> = returns.iter().zip(&market).map(|(r, b)| r - b).collect();
    let growth = |values: &[f64]| Some(values.last()? / values.first()? - 1.0);
    let total_return = growth(&held)?;
    let benchmark_return = growth(&index)?;
    let count = sessions.len();
    let years = count as f64 / TRADING_DAYS;
    let annualized = |growth: f64| (1.0 + growth).max(0.0).powf(1.0 / years) - 1.0;
    let (cagr, benchmark_cagr) = (annualized(total_return), annualized(benchmark_return));
    let excess_stdev = sample_stdev(&excess);
    let moments = sharpe_moments(&excess);
    let max_drawdown = max_drawdown(&held)?;
    let volatility = sample_stdev(&returns).map(|stdev| stdev * TRADING_DAYS.sqrt());
    let backtest = input.source.as_ref().and_then(|source| {
        let track = track(source.start_cash, source.equity, source.benchmark)?;
        let backtest = Backtest {
            held: track.held,
            index: track.index,
        };
        Some((source.run, backtest))
    });
    let rebased = |close: Decimal| round_cash(input.start_cash * close / base.equity);
    let floor = |age: usize, benchmark: Decimal| {
        let (_, backtest) = backtest.as_ref()?;
        let excess = Decimal::from_f64(backtest.excess_floor(age)?)?;
        Some(round_cash(benchmark + input.start_cash * excess))
    };
    let curve: Vec<(usize, LivePoint)> = std::iter::once(LivePoint {
        at: base.at,
        equity: input.start_cash,
        benchmark: input.start_cash,
        floor: None,
    })
    .chain(sessions.iter().map(|(point, close)| LivePoint {
        at: point.at,
        equity: point.equity,
        benchmark: rebased(*close),
        floor: None,
    }))
    .enumerate()
    .collect();
    let curve = thin(curve)
        .into_iter()
        .map(|(age, point)| LivePoint {
            floor: floor(age, point.benchmark),
            ..point
        })
        .collect();
    let sessions_count = u32::try_from(count).unwrap_or(u32::MAX);
    let mut stats = LiveStats {
        through: sessions.last()?.0.at.exchange_date(),
        sessions: sessions_count,
        paused_sessions: u32::try_from(
            sessions
                .iter()
                .filter(|(point, _)| paused_at(input.paused, point.at))
                .count(),
        )
        .unwrap_or(u32::MAX),
        cagr,
        benchmark_cagr,
        excess_cagr: cagr - benchmark_cagr,
        excess_cagr_error: excess_stdev.map(|stdev| stdev * TRADING_DAYS / (count as f64).sqrt()),
        total_return,
        benchmark_return,
        excess_return: total_return - benchmark_return,
        excess_error: excess_stdev.map(|stdev| stdev * (count as f64).sqrt()),
        max_drawdown,
        volatility,
        beta: beta(&returns, &market),
        probabilistic_sharpe: moments.and_then(|moments| probabilistic_sharpe(&moments, 0.0)),
        excess: moments,
        verdict: LiveVerdict::Incubating,
        envelope: None,
        curve,
    };
    stats.envelope = backtest
        .as_ref()
        .map(|(run, backtest)| envelope(*run, backtest, &stats));
    stats.verdict = verdict(&stats, input.started);
    Some(stats)
}

fn verdict(stats: &LiveStats, started: u32) -> LiveVerdict {
    let falsified = stats
        .envelope
        .as_ref()
        .is_some_and(|envelope| !envelope.breaches.is_empty());
    let proven = stats
        .probabilistic_sharpe
        .is_some_and(|confidence| confidence >= proven_confidence(started));
    if stats.sessions < LiveVerdict::TRACKING_SESSIONS {
        LiveVerdict::Incubating
    } else if falsified {
        LiveVerdict::Falsified
    } else if proven {
        LiveVerdict::Proven
    } else if stats.envelope.is_none() {
        LiveVerdict::NoBacktest
    } else {
        LiveVerdict::Tracking
    }
}

fn paused_at(paused: &[(Timestamp, Option<Timestamp>)], at: Timestamp) -> bool {
    paused
        .iter()
        .any(|(from, to)| *from <= at && to.is_none_or(|to| at < to))
}

pub fn backtest_curve(equity: &[EquityPoint], benchmark: &[SparkPoint]) -> Vec<LivePoint> {
    let mut held = benchmark.iter().peekable();
    let mut last = None;
    let curve = equity
        .iter()
        .filter_map(|point| {
            while let Some(next) = held.next_if(|next| next.at <= point.at) {
                last = Some(next.equity);
            }
            Some(LivePoint {
                at: point.at,
                equity: point.equity,
                benchmark: last?,
                floor: None,
            })
        })
        .collect();
    thin(curve)
}

fn thin<T>(curve: Vec<T>) -> Vec<T> {
    let stride = curve.len().div_ceil(CURVE_POINTS).max(1);
    let last = curve.len().saturating_sub(1);
    curve
        .into_iter()
        .enumerate()
        .filter(|(position, _)| position % stride == 0 || *position == last)
        .map(|(_, point)| point)
        .collect()
}

fn sharpe_moments(returns: &[f64]) -> Option<SharpeMoments> {
    let stdev = sample_stdev(returns).filter(|stdev| *stdev > MIN_DAILY_VOLATILITY)?;
    let center = mean(returns);
    let central = |power: i32| {
        mean(
            &returns
                .iter()
                .map(|value| (value - center).powi(power))
                .collect::<Vec<_>>(),
        )
    };
    let spread = central(2).sqrt();
    Some(SharpeMoments {
        sharpe: center / stdev,
        skew: central(3) / spread.powi(3),
        kurtosis: central(4) / spread.powi(4),
        observations: u32::try_from(returns.len()).ok()?,
    })
}

fn sharpe_variance(moments: &SharpeMoments) -> Option<f64> {
    let sharpe = moments.sharpe;
    let variance = 1.0 - moments.skew * sharpe + (moments.kurtosis - 1.0) / 4.0 * sharpe * sharpe;
    (variance > 0.0).then_some(variance)
}

fn probabilistic_sharpe(moments: &SharpeMoments, threshold: f64) -> Option<f64> {
    let observations = f64::from(moments.observations);
    if observations < 2.0 {
        return None;
    }
    let spread = sharpe_variance(moments)?.sqrt();
    Some(normal_cdf(
        (moments.sharpe - threshold) * (observations - 1.0).sqrt() / spread,
    ))
}

fn min_track_record(moments: &SharpeMoments, confidence: f64) -> Option<f64> {
    if moments.sharpe <= 0.0 || !(0.0..1.0).contains(&confidence) {
        return None;
    }
    let quantile = normal_quantile(confidence);
    Some(1.0 + sharpe_variance(moments)? * (quantile / moments.sharpe).powi(2))
}

fn proven_confidence(started: u32) -> f64 {
    1.0 - PROVEN_ALPHA / f64::from(started.max(1))
}

pub fn live_summary(mut run: RunSummary, started: u32) -> LiveSummary {
    if let Some(stats) = run.live_stats.as_mut() {
        stats.verdict = verdict(stats, started);
    }
    LiveSummary {
        sessions_needed: run
            .live_stats
            .as_ref()
            .and_then(|stats| sessions_needed(stats, started)),
        run,
        source: None,
        first_session: None,
    }
}

fn sessions_needed(stats: &LiveStats, started: u32) -> Option<u32> {
    let record = min_track_record(stats.excess.as_ref()?, proven_confidence(started))?;
    let needed = record.ceil().min(f64::from(u32::MAX));
    Some((needed as u32).saturating_sub(stats.sessions))
}

fn normal_cdf(z: f64) -> f64 {
    0.5 * libm::erfc(-z / SQRT_2)
}

fn normal_quantile(probability: f64) -> f64 {
    let (mut low, mut high) = (-QUANTILE_BOUND, QUANTILE_BOUND);
    for _ in 0..QUANTILE_STEPS {
        let middle = (low + high) / 2.0;
        if normal_cdf(middle) < probability {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) / 2.0
}
