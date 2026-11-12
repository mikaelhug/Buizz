use buizz_core::api::{RunSummary, SparkPoint, SpineEvent};
use buizz_core::market::{Bar, RiskFreeRate, Session, Timeframe};
use buizz_core::performance::{LiveStats, Metrics};
use buizz_core::run::{EquityPoint, Mode, RunEventKind, RunEventType, RunId, RunStatus};
use buizz_core::time::Timestamp;
use buizz_market::MarketStore;
use buizz_metrics::{LiveInput, MetricsInput, Source};
use buizz_state::{RunFilter, StateStore};
use chrono::{Days, NaiveDate};
use rust_decimal::Decimal;

use crate::{Inner, RunnerError};

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
    let bars: Vec<(Session, Bar)> = bars
        .into_iter()
        .filter_map(|bar| Some((*calendar.containing(bar.start)?, bar)))
        .collect();
    let Some((base, _)) = bars.first() else {
        return Ok(Vec::new());
    };
    let actions = market
        .corporate_actions(vec![config.benchmark.clone()], base.date, last.date)
        .await?;
    Ok(buizz_metrics::buy_and_hold(
        config.start_cash,
        &bars,
        &actions,
    ))
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
) -> Result<(Metrics, Vec<RiskFreeRate>), RunnerError> {
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
    let metrics = buizz_metrics::compute(&MetricsInput {
        start_cash: run.config.start_cash,
        equity: &held,
        benchmark,
        ledger: &[],
        risk_free: &risk_free,
    });
    Ok((metrics, risk_free))
}

async fn paused_spans(
    state: &StateStore,
    run_id: RunId,
) -> Result<Vec<(Timestamp, Option<Timestamp>)>, RunnerError> {
    let events = state
        .events(run_id, None, u32::MAX, vec![RunEventType::Status])
        .await?;
    let mut spans: Vec<(Timestamp, Option<Timestamp>)> = Vec::new();
    for event in events {
        let RunEventKind::Status { status, .. } = event.kind else {
            continue;
        };
        let open = spans.last().is_some_and(|(_, to)| to.is_none());
        match (status == RunStatus::Paused, open) {
            (true, false) => spans.push((event.at, None)),
            (false, true) => {
                if let Some((_, to)) = spans.last_mut() {
                    *to = Some(event.at);
                }
            }
            _ => {}
        }
    }
    Ok(spans)
}

async fn source_backtest(
    state: &StateStore,
    run: &RunSummary,
) -> Result<Option<RunSummary>, RunnerError> {
    if let Some(source) = run.source_run {
        return Ok(state.run(source).await?);
    }
    let hash = |run: &RunSummary| {
        run.reproducibility
            .as_ref()
            .map(|record| record.bot_hash.clone())
    };
    let backtests = state
        .runs(RunFilter {
            bot: Some(run.bot.clone()),
            mode: Some(Mode::Backtest),
            statuses: vec![RunStatus::Completed],
            ..RunFilter::default()
        })
        .await?;
    Ok(backtests
        .into_iter()
        .filter(|backtest| {
            !backtest.config.holdout
                && backtest.config.params == run.config.params
                && backtest.config.cost == run.config.cost
                && backtest.config.risk == run.config.risk
                && hash(backtest) == hash(run)
        })
        .max_by_key(|backtest| {
            let end = backtest.config.end.unwrap_or(backtest.config.start);
            (end - backtest.config.start, backtest.id)
        }))
}

async fn live_stats(
    market: &MarketStore,
    state: &StateStore,
    run: &RunSummary,
    started: u32,
) -> Result<Option<LiveStats>, RunnerError> {
    let equity = state.equity(run.id).await?;
    let Some(last) = equity.last() else {
        return Ok(None);
    };
    let source = source_backtest(state, run).await?;
    let source_equity = match &source {
        Some(source) => state.equity(source.id).await?,
        None => Vec::new(),
    };
    let source_benchmark = match (&source, source_equity.last()) {
        (Some(source), Some(close)) => benchmark(market, source, close.at.exchange_date()).await?,
        _ => Vec::new(),
    };
    let benchmark = benchmark(market, run, last.at.exchange_date()).await?;
    let paused = paused_spans(state, run.id).await?;
    Ok(buizz_metrics::live(&LiveInput {
        start_cash: run.config.start_cash,
        equity: &equity,
        benchmark: &benchmark,
        paused: &paused,
        source: source.as_ref().map(|source| Source {
            run: source.id,
            start_cash: source.config.start_cash,
            equity: &source_equity,
            benchmark: &source_benchmark,
        }),
        started,
    }))
}

pub(crate) async fn live_session(inner: &Inner, run_id: RunId) {
    if let Err(error) = record_live_session(inner, run_id).await {
        tracing::warn!(run = %run_id, %error, "cannot record the live session stats");
    }
}

async fn record_live_session(inner: &Inner, run_id: RunId) -> Result<(), RunnerError> {
    let state = &inner.state;
    let Some(run) = state.run(run_id).await? else {
        return Err(RunnerError::UnknownRun(run_id));
    };
    let started = state.live_runs_started().await?;
    let Some(stats) = live_stats(&inner.market, state, &run, started).await? else {
        return Ok(());
    };
    if run.live_stats.as_ref() == Some(&stats) {
        return Ok(());
    }
    state.set_live_stats(run_id, stats.clone()).await?;
    inner.broadcast(SpineEvent::LiveSession {
        run_id,
        bot: run.bot.clone(),
        summary: Box::new(buizz_metrics::live_summary(
            RunSummary {
                live_stats: Some(stats),
                ..run
            },
            started,
        )),
    });
    Ok(())
}
