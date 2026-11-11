use buizz_core::api::{RunSummary, SparkPoint};
use buizz_core::market::Timeframe;
use buizz_core::money::round_cash;
use buizz_core::performance::Metrics;
use buizz_core::run::EquityPoint;
use buizz_market::MarketStore;
use buizz_metrics::MetricsInput;
use buizz_state::StateStore;
use chrono::{Days, NaiveDate};
use rust_decimal::Decimal;

use crate::RunnerError;

const RISK_FREE_LOOKBACK: Days = Days::new(31);

pub(crate) async fn benchmark(
    market: &MarketStore,
    run: &RunSummary,
    through: NaiveDate,
) -> Result<Vec<SparkPoint>, RunnerError> {
    let config = &run.config;
    let calendar = market
        .calendar(NaiveDate::MIN, config.end.unwrap_or(through).min(through))
        .await?;
    let sessions = calendar.between(config.start, NaiveDate::MAX);
    let (Some(first), Some(last)) = (sessions.first(), sessions.last()) else {
        return Ok(Vec::new());
    };
    let from = calendar.previous_before(first.date).unwrap_or(first);
    let bars = market
        .bars_between(
            Timeframe::Day,
            vec![config.benchmark.clone()],
            from.open,
            last.close,
        )
        .await?;
    let Some(base) = bars
        .first()
        .map(|bar| bar.close)
        .filter(|close| !close.is_zero())
    else {
        return Ok(Vec::new());
    };
    Ok(bars
        .iter()
        .filter_map(|bar| {
            let session = calendar.containing(bar.start)?;
            Some(SparkPoint {
                at: session.close,
                equity: round_cash(config.start_cash * bar.close / base),
            })
        })
        .collect())
}

pub(crate) async fn metrics(
    market: &MarketStore,
    state: &StateStore,
    run: &RunSummary,
) -> Result<Metrics, RunnerError> {
    let equity: Vec<EquityPoint> = state.equity(run.id).await?;
    let through = equity
        .last()
        .map_or(run.config.start, |point| point.at.exchange_date());
    let benchmark = benchmark(market, run, through).await?;
    let ledger = state.ledger(run.id).await?;
    let risk_free = market
        .risk_free_rates(
            run.config.start - RISK_FREE_LOOKBACK,
            through.max(run.config.start),
        )
        .await?;
    Ok(buizz_metrics::compute(&MetricsInput {
        start_cash: run.config.start_cash,
        equity: &equity,
        benchmark: &benchmark,
        ledger: &ledger,
        risk_free: &risk_free,
    }))
}

pub(crate) async fn benchmark_metrics(
    market: &MarketStore,
    run: &RunSummary,
    benchmark: &[SparkPoint],
) -> Result<Metrics, RunnerError> {
    let held: Vec<EquityPoint> = benchmark
        .iter()
        .filter(|point| point.at.exchange_date() >= run.config.start)
        .map(|point| EquityPoint {
            at: point.at,
            equity: point.equity,
            cash: Decimal::ZERO,
            gross_exposure: point.equity,
        })
        .collect();
    let through = held
        .last()
        .map_or(run.config.start, |point| point.at.exchange_date());
    let risk_free = market
        .risk_free_rates(run.config.start - RISK_FREE_LOOKBACK, through)
        .await?;
    Ok(buizz_metrics::compute(&MetricsInput {
        start_cash: run.config.start_cash,
        equity: &held,
        benchmark,
        ledger: &[],
        risk_free: &risk_free,
    }))
}
