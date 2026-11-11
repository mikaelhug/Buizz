use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;

use axum::Json;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use buizz_core::api::{
    ApiError, EquitySeries, EventsPage, Leaderboard, LeaderboardQuery, LeaderboardRow, OrderView,
    OrdersPage, PageQuery, PositionView, RunSummary, RunsQuery,
};
use buizz_core::market::{Adjustment, Symbol};
use buizz_core::money::round_price;
use buizz_core::order::{Fill, OrderId};
use buizz_core::run::{CostModel, Mode, Period, RunId, RunStatus};
use buizz_market::HistoryRequest;
use buizz_state::RunFilter;
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
const LOG_DIR: &str = "data/logs";
const LOG_TAIL_BYTES: u64 = 256 * 1024;
const WEIGHT_DECIMALS: u32 = 4;

async fn require_run(app: &AppState, id: RunId) -> Result<RunSummary, Failure> {
    app.state
        .run(id)
        .await?
        .ok_or_else(|| Failure::not_found(format!("run {id}")))
}

fn page_limit(page: &PageQuery) -> u32 {
    page.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
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
    Ok(Json(EquitySeries {
        run_id: id,
        points,
        benchmark,
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
    let limit = page_limit(&page);
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
    params(("id" = RunId, Path), PageQuery),
    responses((status = 200, body = EventsPage), (status = 404, body = ApiError)))]
pub(crate) async fn run_events(
    State(app): State<AppState>,
    Path(id): Path<RunId>,
    Query(page): Query<PageQuery>,
) -> Result<Json<EventsPage>, Failure> {
    require_run(&app, id).await?;
    let limit = page_limit(&page);
    let items = app.state.events(id, page.after, limit).await?;
    let next_after = next_after(&items, limit, |event| event.seq);
    Ok(Json(EventsPage { items, next_after }))
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
    let path = std::path::Path::new(LOG_DIR).join(format!("{id}.log"));
    let text = match tokio::fs::File::open(&path).await {
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

#[utoipa::path(get, path = "/api/leaderboard", tag = "leaderboard",
    params(LeaderboardQuery),
    responses((status = 200, body = Leaderboard)))]
pub(crate) async fn leaderboard(
    State(app): State<AppState>,
    Query(query): Query<LeaderboardQuery>,
) -> Result<Json<Leaderboard>, Failure> {
    let cost = CostModel::default();
    let runs = app
        .state
        .runs(RunFilter {
            mode: Some(Mode::Backtest),
            statuses: vec![RunStatus::Completed],
            limit: Some(LEADERBOARD_RUNS),
            ..RunFilter::default()
        })
        .await?;
    let mut groups: Vec<(Period, Vec<LeaderboardRow>)> = Vec::new();
    let mut seen: HashSet<(Period, String)> = HashSet::new();
    for run in runs.iter().filter(|run| run.config.cost == cost) {
        let period = query.preset.resolve(run.created_at.exchange_date());
        let (true, Some(metrics), Some(finished_at)) = (
            compared_period(run, period),
            run.metrics.clone(),
            run.finished_at,
        ) else {
            continue;
        };
        if !seen.insert((period, run.bot.clone())) {
            continue;
        }
        let row = LeaderboardRow {
            bot: run.bot.clone(),
            run_id: run.id,
            metrics,
            finished_at,
        };
        match groups.iter_mut().find(|(group, _)| *group == period) {
            Some((_, rows)) => rows.push(row),
            None => groups.push((period, vec![row])),
        }
    }
    let (period, mut rows) = groups
        .into_iter()
        .max_by_key(|(period, rows)| (rows.len(), period.to))
        .unwrap_or_else(|| (query.preset.resolve(exchange_today()), Vec::new()));
    rows.sort_by(|a, b| {
        let sharpe = |row: &LeaderboardRow| row.metrics.sharpe.unwrap_or(f64::NEG_INFINITY);
        sharpe(b).total_cmp(&sharpe(a))
    });
    Ok(Json(Leaderboard {
        preset: query.preset,
        period,
        cost,
        rows,
    }))
}
