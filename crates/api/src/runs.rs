use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;

use axum::Json;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use buizz_core::api::{
    ApiError, ChartFill, ChartPosition, ChartQuery, ChartSplit, EquitySeries, EventsPage,
    EventsQuery, Leaderboard, LeaderboardBenchmark, LeaderboardQuery, LeaderboardRow, OrderView,
    OrdersPage, PageQuery, PositionView, PriceBar, PriceChart, RunSummary, RunsQuery, TradeView,
};
use buizz_core::market::{Adjustment, AdjustmentFactor, Symbol};
use buizz_core::money::round_price;
use buizz_core::order::{Fill, OrderId};
use buizz_core::run::{BotManifest, CostModel, Mode, Period, RunBadge, RunId, RunStatus};
use buizz_core::time::Timestamp;
use buizz_market::{ChartRequest, HistoryRequest};
use buizz_state::RunFilter;
use chrono::NaiveTime;
use rust_decimal::Decimal;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::AppState;
use crate::error::Failure;
use crate::extract::{Path, Query};
use crate::time::exchange_today;

const DEFAULT_RUNS: u32 = 100;
const MAX_RUNS: u32 = 500;
const DEFAULT_PAGE: u32 = 200;
const MAX_PAGE: u32 = 1000;
const LEADERBOARD_RUNS: u32 = 1000;
const LOG_TAIL_BYTES: u64 = 256 * 1024;
const WEIGHT_DECIMALS: u32 = 4;
const CHART_BARS: usize = 5000;

async fn require_run(app: &AppState, id: RunId) -> Result<RunSummary, Failure> {
    app.state
        .run(id)
        .await?
        .ok_or_else(|| Failure::not_found(format!("run {id}")))
}

fn page_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}

fn next_after<T>(items: &[T], limit: u32, key: impl Fn(&T) -> u64) -> Option<u64> {
    let full = u32::try_from(items.len()).is_ok_and(|len| len >= limit);
    full.then(|| items.last().map(key)).flatten()
}

#[utoipa::path(get, path = "/api/runs", tag = "runs",
    params(RunsQuery),
    responses((status = 200, body = Vec<RunSummary>)))]
pub(crate) async fn list_runs(
    State(app): State<AppState>,
    Query(query): Query<RunsQuery>,
) -> Result<Json<Vec<RunSummary>>, Failure> {
    let filter = RunFilter {
        bot: query.bot,
        mode: query.mode,
        before: query.before,
        limit: Some(query.limit.unwrap_or(DEFAULT_RUNS).clamp(1, MAX_RUNS)),
        ..RunFilter::default()
    };
    Ok(Json(app.state.runs(filter).await?))
}

#[utoipa::path(get, path = "/api/runs/{id}", tag = "runs",
    params(("id" = RunId, Path)),
    responses((status = 200, body = RunSummary), (status = 404, body = ApiError)))]
pub(crate) async fn run_detail(
    State(app): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<Json<RunSummary>, Failure> {
    Ok(Json(require_run(&app, id).await?))
}

#[utoipa::path(post, path = "/api/runs/{id}/cancel", tag = "runs",
    params(("id" = RunId, Path)),
    responses((status = 200, body = RunSummary), (status = 404, body = ApiError), (status = 409, body = ApiError)))]
pub(crate) async fn cancel_run(
    State(app): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<Json<RunSummary>, Failure> {
    Ok(Json(app.runner.cancel(id).await?))
}

#[utoipa::path(get, path = "/api/runs/{id}/equity", tag = "runs",
    params(("id" = RunId, Path)),
    responses((status = 200, body = EquitySeries), (status = 404, body = ApiError)))]
pub(crate) async fn run_equity(
    State(app): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<Json<EquitySeries>, Failure> {
    let run = require_run(&app, id).await?;
    let points = app.state.equity(id).await?;
    let benchmark = app.runner.benchmark(&run).await?;
    let benchmark_metrics = app.runner.benchmark_metrics(&run, &benchmark).await?;
    let interim_metrics = if run.status.is_finished() || points.is_empty() {
        None
    } else {
        Some(app.runner.metrics(&run).await?)
    };
    Ok(Json(EquitySeries {
        run_id: id,
        points,
        benchmark,
        benchmark_metrics,
        interim_metrics,
    }))
}

fn average_fill_prices(fills: &[Fill]) -> HashMap<OrderId, Decimal> {
    let mut totals: HashMap<OrderId, (Decimal, Decimal)> = HashMap::new();
    for fill in fills {
        let qty = Decimal::from(fill.qty.unsigned_abs());
        let (value, total_qty) = totals.entry(fill.order_id).or_default();
        *value += qty * fill.price;
        *total_qty += qty;
    }
    totals
        .into_iter()
        .filter(|(_, (_, qty))| !qty.is_zero())
        .map(|(order_id, (value, qty))| (order_id, round_price(value / qty)))
        .collect()
}

#[utoipa::path(get, path = "/api/runs/{id}/orders", tag = "runs",
    params(("id" = RunId, Path), PageQuery),
    responses((status = 200, body = OrdersPage), (status = 404, body = ApiError)))]
pub(crate) async fn run_orders(
    State(app): State<AppState>,
    Path(id): Path<RunId>,
    Query(page): Query<PageQuery>,
) -> Result<Json<OrdersPage>, Failure> {
    require_run(&app, id).await?;
    let limit = page_limit(page.limit);
    let orders = app.state.orders(id, page.after, limit).await?;
    let prices = match (orders.first(), orders.last()) {
        (Some(first), Some(last)) => {
            average_fill_prices(&app.state.fills(id, first.id..=last.id).await?)
        }
        _ => HashMap::new(),
    };
    let next_after = next_after(&orders, limit, |order| order.id.0);
    let items = orders
        .into_iter()
        .map(|order| OrderView {
            avg_fill_price: prices.get(&order.id).copied(),
            order,
        })
        .collect();
    Ok(Json(OrdersPage { items, next_after }))
}

#[utoipa::path(get, path = "/api/runs/{id}/trades", tag = "runs",
    params(("id" = RunId, Path)),
    responses((status = 200, body = Vec<TradeView>), (status = 404, body = ApiError)))]
pub(crate) async fn run_trades(
    State(app): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<Json<Vec<TradeView>>, Failure> {
    require_run(&app, id).await?;
    let trips = buizz_metrics::round_trips(&app.state.ledger(id).await?);
    let reasons: HashMap<OrderId, String> = app
        .state
        .orders(id, None, u32::MAX)
        .await?
        .into_iter()
        .map(|order| (order.id, order.reason))
        .collect();
    let reason = |order: Option<OrderId>| order.and_then(|order| reasons.get(&order).cloned());
    Ok(Json(
        trips
            .into_iter()
            .map(|trip| TradeView {
                opening_reason: reason(trip.opened_by),
                closing_reason: reason(trip.closed_by),
                trip,
            })
            .collect(),
    ))
}

async fn latest_closes(
    app: &AppState,
    run: &RunSummary,
    symbols: Vec<Symbol>,
) -> Result<Vec<(Symbol, Decimal)>, Failure> {
    let Some(clock) = run.clock.filter(|_| !symbols.is_empty()) else {
        return Ok(Vec::new());
    };
    let rows = app
        .market
        .history(HistoryRequest {
            symbols,
            timeframe: run.config.timeframe,
            bars: 1,
            adjust: Adjustment::Raw,
            as_of: clock,
        })
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let close = Decimal::try_from(row.close).ok()?;
            Some((row.symbol, round_price(close)))
        })
        .collect())
}

#[utoipa::path(get, path = "/api/runs/{id}/positions", tag = "runs",
    params(("id" = RunId, Path)),
    responses((status = 200, body = Vec<PositionView>), (status = 404, body = ApiError)))]
pub(crate) async fn run_positions(
    State(app): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<Json<Vec<PositionView>>, Failure> {
    let run = require_run(&app, id).await?;
    let mut portfolio = app.state.portfolio(id).await?;
    let held = portfolio
        .open_positions()
        .map(|(symbol, _)| symbol.clone())
        .collect();
    for (symbol, close) in latest_closes(&app, &run, held).await? {
        portfolio.mark(&symbol, close);
    }
    let equity = portfolio.equity();
    let views = portfolio
        .open_positions()
        .map(|(symbol, position)| {
            let market_value = position.market_value();
            PositionView {
                symbol: symbol.clone(),
                qty: position.qty,
                avg_cost: position.average_cost(),
                last_price: position.last_price,
                market_value,
                unrealized_pnl: market_value - position.cost_basis,
                realized_pnl: position.realized_pnl,
                weight: if equity.is_zero() {
                    Decimal::ZERO
                } else {
                    (market_value / equity).round_dp(WEIGHT_DECIMALS)
                },
            }
        })
        .collect();
    Ok(Json(views))
}

#[utoipa::path(get, path = "/api/runs/{id}/events", tag = "runs",
    params(("id" = RunId, Path), EventsQuery),
    responses((status = 200, body = EventsPage), (status = 404, body = ApiError)))]
pub(crate) async fn run_events(
    State(app): State<AppState>,
    Path(id): Path<RunId>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<EventsPage>, Failure> {
    require_run(&app, id).await?;
    let limit = page_limit(query.limit);
    let items = app.state.events(id, query.after, limit, query.kind).await?;
    let next_after = next_after(&items, limit, |event| event.seq);
    Ok(Json(EventsPage { items, next_after }))
}

#[utoipa::path(get, path = "/api/runs/{id}/chart/{symbol}", tag = "runs",
    params(("id" = RunId, Path), ("symbol" = Symbol, Path), ChartQuery),
    responses((status = 200, body = PriceChart), (status = 404, body = ApiError)))]
pub(crate) async fn run_chart(
    State(app): State<AppState>,
    Path((id, symbol)): Path<(RunId, Symbol)>,
    Query(query): Query<ChartQuery>,
) -> Result<Json<PriceChart>, Failure> {
    let run = require_run(&app, id).await?;
    let timeframe = run.config.timeframe;
    let empty = PriceChart {
        symbol: symbol.clone(),
        timeframe,
        bars: Vec::new(),
        older: false,
        fills: Vec::new(),
        splits: Vec::new(),
        position: None,
    };
    let (Some(clock), Some(from)) = (
        run.clock,
        Timestamp::at_exchange_time(run.config.start, NaiveTime::MIN),
    ) else {
        return Ok(Json(empty));
    };
    let chart = app
        .market
        .chart(ChartRequest {
            symbol: symbol.clone(),
            timeframe,
            from,
            after: query.after,
            before: query.before,
            bars: CHART_BARS,
            as_of: clock,
        })
        .await?;
    let fills = app
        .state
        .symbol_fills(id, symbol.clone())
        .await?
        .into_iter()
        .map(|(fill, reason)| {
            let (factor, _) = AdjustmentFactor::cumulative(fill.at.exchange_date(), &chart.splits);
            ChartFill {
                order_id: fill.order_id,
                at: fill.at,
                qty: fill.qty,
                chart_price: round_price(fill.price * factor),
                price: fill.price,
                reason,
            }
        })
        .collect();
    let position = app
        .state
        .portfolio(id)
        .await?
        .open_positions()
        .find(|(held, _)| **held == symbol)
        .map(|(_, position)| ChartPosition {
            qty: position.qty,
            avg_cost: position.average_cost(),
        });
    Ok(Json(PriceChart {
        bars: chart
            .bars
            .into_iter()
            .map(|bar| PriceBar {
                start: bar.start,
                open: bar.open,
                high: bar.high,
                low: bar.low,
                close: bar.close,
                volume: bar.volume,
            })
            .collect(),
        older: chart.older,
        fills,
        splits: chart
            .splits
            .iter()
            .map(|split| ChartSplit {
                ex_date: split.ex_date,
                ratio: split.volume,
            })
            .collect(),
        position,
        ..empty
    }))
}

#[utoipa::path(get, path = "/api/runs/{id}/logs", tag = "runs",
    params(("id" = RunId, Path)),
    responses(
        (status = 200, description = "the tail of the bot process log", content_type = "text/plain",
            body = String),
        (status = 404, body = ApiError)))]
pub(crate) async fn run_logs(
    State(app): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, Failure> {
    require_run(&app, id).await?;
    let text = match tokio::fs::File::open(app.runner.log_path(id)).await {
        Ok(mut file) => {
            let length = file.metadata().await?.len();
            let start = length.saturating_sub(LOG_TAIL_BYTES);
            file.seek(SeekFrom::Start(start)).await?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).await?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            match (start, text.split_once('\n')) {
                (0, _) | (_, None) => text,
                (_, Some((_, rest))) => rest.to_owned(),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text))
}

fn compared_period(run: &RunSummary, period: Period) -> bool {
    run.config.start == period.from && run.config.end == Some(period.to)
}

fn trials(runs: &[RunSummary]) -> HashMap<&str, u32> {
    let mut counts = HashMap::new();
    for run in runs {
        *counts.entry(run.bot.as_str()).or_default() += 1;
    }
    counts
}

async fn benchmarks(
    app: &AppState,
    compared: &[&RunSummary],
) -> Result<Vec<LeaderboardBenchmark>, Failure> {
    let mut seen = HashSet::new();
    let mut benchmarks = Vec::new();
    for run in compared {
        if !seen.insert(&run.config.benchmark) {
            continue;
        }
        let points = app.runner.benchmark(run).await?;
        benchmarks.push(LeaderboardBenchmark {
            symbol: run.config.benchmark.clone(),
            metrics: app.runner.benchmark_metrics(run, &points).await?,
        });
    }
    Ok(benchmarks)
}

fn as_configured(run: &RunSummary, manifest: &BotManifest, cost: &CostModel) -> bool {
    run.config.cost == *cost
        && run.config.risk == manifest.risk
        && run.config.params == manifest.params
}

#[utoipa::path(get, path = "/api/leaderboard", tag = "leaderboard",
    params(LeaderboardQuery),
    responses((status = 200, body = Leaderboard)))]
pub(crate) async fn leaderboard(
    State(app): State<AppState>,
    Query(query): Query<LeaderboardQuery>,
) -> Result<Json<Leaderboard>, Failure> {
    let cost = CostModel::default();
    let scan = app.runner.bots();
    let manifests: HashMap<&str, &BotManifest> = scan
        .bots
        .iter()
        .map(|bot| (bot.name.as_str(), &bot.manifest))
        .collect();
    let runs = app
        .state
        .runs(RunFilter {
            mode: Some(Mode::Backtest),
            statuses: vec![RunStatus::Completed, RunStatus::Failed],
            limit: Some(LEADERBOARD_RUNS),
            ..RunFilter::default()
        })
        .await?
        .into_iter()
        .filter(|run| {
            run.status == RunStatus::Completed || run.badges.contains(&RunBadge::DrawdownStop)
        })
        .collect::<Vec<_>>();
    let trials = trials(&runs);
    let mut groups: Vec<(Period, Vec<&RunSummary>)> = Vec::new();
    let mut seen: HashSet<(Period, &str)> = HashSet::new();
    let ranked = runs.iter().filter(|run| {
        manifests
            .get(run.bot.as_str())
            .is_some_and(|manifest| as_configured(run, manifest, &cost))
    });
    for run in ranked {
        let period = query.preset.resolve(run.created_at.exchange_date());
        if !compared_period(run, period)
            || run.metrics.is_none()
            || run.finished_at.is_none()
            || !seen.insert((period, run.bot.as_str()))
        {
            continue;
        }
        match groups.iter_mut().find(|(group, _)| *group == period) {
            Some((_, compared)) => compared.push(run),
            None => groups.push((period, vec![run])),
        }
    }
    let (period, compared) = groups
        .into_iter()
        .max_by_key(|(period, compared)| (compared.len(), period.to))
        .unwrap_or_else(|| (query.preset.resolve(exchange_today()), Vec::new()));
    let benchmarks = benchmarks(&app, &compared).await?;
    let rows = compared
        .into_iter()
        .filter_map(|run| {
            Some(LeaderboardRow {
                bot: run.bot.clone(),
                run_id: run.id,
                params: run.config.params.clone(),
                metrics: run.metrics.clone()?,
                badges: run.badges.clone(),
                trials: trials.get(run.bot.as_str()).copied().unwrap_or_default(),
                finished_at: run.finished_at?,
            })
        })
        .collect();
    Ok(Json(Leaderboard {
        preset: query.preset,
        period,
        cost,
        rows,
        benchmarks,
    }))
}
