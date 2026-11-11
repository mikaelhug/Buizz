use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use buizz_core::api::{ApiError, BotDetail, BotStatus, BotSummary, RunSummary, StartBacktest};
use buizz_core::run::{BotManifest, DesiredState, Mode, RunBadge, RunStatus};
use buizz_state::RunFilter;
use futures::future::{try_join, try_join_all};

use crate::AppState;
use crate::error::Failure;
use crate::extract::{JsonBody, Path};
use crate::time::exchange_today;

const RECENT_RUNS: u32 = 50;
const SPARKLINE_POINTS: u32 = 60;

fn bot_status(runs: &[RunSummary], live: Option<&RunSummary>) -> BotStatus {
    if let Some(live) = live {
        return if live.status == RunStatus::Paused || live.desired == DesiredState::Paused {
            BotStatus::Paused
        } else {
            BotStatus::Live
        };
    }
    if runs.iter().any(|run| !run.status.is_finished()) {
        return BotStatus::Backtesting;
    }
    match runs.first() {
        Some(run) if run.badges.contains(&RunBadge::DrawdownStop) => BotStatus::Stopped,
        Some(run) if run.status == RunStatus::Failed => BotStatus::Error,
        _ => BotStatus::Idle,
    }
}

async fn recent_runs(app: &AppState, bot: &str) -> Result<Vec<RunSummary>, Failure> {
    let filter = RunFilter {
        bot: Some(bot.to_owned()),
        limit: Some(RECENT_RUNS),
        ..RunFilter::default()
    };
    Ok(app.state.runs(filter).await?)
}

async fn summarize(
    app: &AppState,
    name: &str,
    manifest: &BotManifest,
    runs: &[RunSummary],
) -> Result<BotSummary, Failure> {
    let live = runs
        .iter()
        .find(|run| run.mode == Mode::Live && !run.status.is_finished());
    let focus = live.or_else(|| runs.iter().find(|run| run.equity.is_some()));
    let sparkline = async {
        match focus {
            Some(run) => app.state.sparkline(run.id, SPARKLINE_POINTS).await,
            None => Ok(Vec::new()),
        }
    };
    let (sparkline, holdout_runs) = try_join(sparkline, app.state.holdout_runs(name)).await?;
    let stats = runs
        .iter()
        .find_map(|run| run.metrics.clone().map(|metrics| (run.id, metrics)));
    Ok(BotSummary {
        name: name.to_owned(),
        description: manifest.description.clone(),
        timeframe: manifest.timeframe,
        status: bot_status(runs, live),
        live_run: live.map(|run| run.id),
        equity: focus.and_then(|run| run.equity),
        sparkline,
        stats_run: stats.as_ref().map(|(id, _)| *id),
        stats: stats.map(|(_, metrics)| metrics),
        holdout_runs,
        last_run_at: runs.first().map(|run| run.created_at),
    })
}

#[utoipa::path(get, path = "/api/bots", tag = "bots",
    responses((status = 200, body = Vec<BotSummary>)))]
pub(crate) async fn list_bots(
    State(app): State<AppState>,
) -> Result<Json<Vec<BotSummary>>, Failure> {
    let scan = app.runner.bots();
    let summaries = try_join_all(scan.bots.iter().map(|bot| async {
        let runs = recent_runs(&app, &bot.name).await?;
        summarize(&app, &bot.name, &bot.manifest, &runs).await
    }))
    .await?;
    Ok(Json(summaries))
}

#[utoipa::path(get, path = "/api/bots/{name}", tag = "bots",
    params(("name" = String, Path)),
    responses((status = 200, body = BotDetail), (status = 404, body = ApiError)))]
pub(crate) async fn bot_detail(
    State(app): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<BotDetail>, Failure> {
    let scan = app.runner.bots();
    let bot = scan
        .bots
        .iter()
        .find(|bot| bot.name == name)
        .ok_or_else(|| Failure::not_found(format!("bot {name}")))?;
    let runs = recent_runs(&app, &bot.name).await?;
    Ok(Json(BotDetail {
        summary: summarize(&app, &bot.name, &bot.manifest, &runs).await?,
        manifest: bot.manifest.clone(),
        bot_hash: bot.content_hash.clone(),
        runs,
    }))
}

#[utoipa::path(post, path = "/api/bots/{name}/backtests", tag = "bots",
    params(("name" = String, Path)),
    request_body = StartBacktest,
    responses(
        (status = 201, body = RunSummary),
        (status = 400, body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, description = "holdout locked or data gaps", body = ApiError)))]
pub(crate) async fn start_backtest(
    State(app): State<AppState>,
    Path(name): Path<String>,
    JsonBody(request): JsonBody<StartBacktest>,
) -> Result<(StatusCode, Json<RunSummary>), Failure> {
    let run = app
        .runner
        .start_backtest(&name, request, exchange_today())
        .await?;
    Ok((StatusCode::CREATED, Json(run)))
}

#[utoipa::path(post, path = "/api/bots/{name}/live", tag = "bots",
    params(("name" = String, Path)),
    responses(
        (status = 200, body = RunSummary),
        (status = 404, body = ApiError),
        (status = 409, description = "stream symbol cap reached", body = ApiError)))]
pub(crate) async fn go_live(
    State(app): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<RunSummary>, Failure> {
    Ok(Json(app.runner.go_live(&name).await?))
}

#[utoipa::path(post, path = "/api/bots/{name}/pause", tag = "bots",
    params(("name" = String, Path)),
    responses((status = 200, body = RunSummary), (status = 404, body = ApiError), (status = 409, body = ApiError)))]
pub(crate) async fn pause(
    State(app): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<RunSummary>, Failure> {
    Ok(Json(app.runner.pause(&name).await?))
}
