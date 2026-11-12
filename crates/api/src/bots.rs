use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use buizz_core::api::{
    ApiError, BotDetail, BotStage, BotStatus, BotSummary, League, LiveSummary, RunSummary,
    StartBacktest,
};
use buizz_core::performance::LiveVerdict;
use buizz_core::run::{BotManifest, DesiredState, Mode, RunBadge, RunStatus};
use buizz_core::time::Timestamp;
use buizz_metrics::live_summary;
use buizz_state::RunFilter;
use futures::future::{try_join, try_join_all};

use crate::AppState;
use crate::error::Failure;
use crate::extract::{JsonBody, Path};

const RECENT_RUNS: u32 = 50;

struct Bot<'a> {
    name: &'a str,
    manifest: &'a BotManifest,
    hash: &'a str,
}

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

fn live_runs(bot: Option<&str>, statuses: &[RunStatus]) -> RunFilter {
    RunFilter {
        bot: bot.map(str::to_owned),
        mode: Some(Mode::Live),
        statuses: statuses.to_vec(),
        ..RunFilter::default()
    }
}

async fn summarize(
    app: &AppState,
    bot: Bot<'_>,
    live: &[RunSummary],
    started: u32,
) -> Result<BotSummary, Failure> {
    let activity = app.state.bot_activity(bot.name, bot.hash).await?;
    let live: Vec<LiveSummary> = live
        .iter()
        .filter(|run| run.bot == bot.name)
        .map(|run| live_summary(run.clone(), started))
        .collect();
    let stage = match (live.is_empty(), activity.screened) {
        (false, _) => BotStage::Live,
        (true, true) => BotStage::Screened,
        (true, false) => BotStage::Draft,
    };
    Ok(BotSummary {
        name: bot.name.to_owned(),
        description: bot.manifest.description.clone(),
        timeframe: bot.manifest.timeframe,
        stage,
        live,
        last_run_at: activity.last_run_at,
    })
}

#[utoipa::path(get, path = "/api/bots", tag = "bots",
    responses((status = 200, body = Vec<BotSummary>)))]
pub(crate) async fn list_bots(
    State(app): State<AppState>,
) -> Result<Json<Vec<BotSummary>>, Failure> {
    let scan = app.runner.bots();
    let (live, started) = try_join(
        app.state.runs(live_runs(None, &RunStatus::ACTIVE)),
        app.state.live_runs_started(),
    )
    .await?;
    let summaries = try_join_all(scan.bots.iter().map(|spec| {
        let bot = Bot {
            name: &spec.name,
            manifest: &spec.manifest,
            hash: &spec.content_hash,
        };
        summarize(&app, bot, &live, started)
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
    let spec = scan
        .bots
        .iter()
        .find(|bot| bot.name == name)
        .ok_or_else(|| Failure::not_found(format!("bot {name}")))?;
    let recent = RunFilter {
        bot: Some(name.clone()),
        limit: Some(RECENT_RUNS),
        ..RunFilter::default()
    };
    let ((runs, live), (started, holdout_runs)) = try_join(
        try_join(
            app.state.runs(recent),
            app.state.runs(live_runs(Some(&name), &RunStatus::ACTIVE)),
        ),
        try_join(app.state.live_runs_started(), app.state.holdout_runs(&name)),
    )
    .await?;
    let bot = Bot {
        name: &spec.name,
        manifest: &spec.manifest,
        hash: &spec.content_hash,
    };
    Ok(Json(BotDetail {
        status: bot_status(&runs, live.first()),
        summary: summarize(&app, bot, &live, started).await?,
        holdout_runs,
        manifest: spec.manifest.clone(),
        bot_hash: spec.content_hash.clone(),
        runs,
    }))
}

fn confidence(summary: &LiveSummary) -> f64 {
    summary
        .run
        .live_stats
        .as_ref()
        .and_then(|stats| stats.probabilistic_sharpe)
        .unwrap_or(f64::NEG_INFINITY)
}

#[utoipa::path(get, path = "/api/league", tag = "league",
    responses((status = 200, body = League)))]
pub(crate) async fn league(State(app): State<AppState>) -> Result<Json<League>, Failure> {
    let runs = app.state.runs(live_runs(None, &[])).await?;
    let started = u32::try_from(runs.len()).unwrap_or(u32::MAX);
    let (mut ranked, too_early): (Vec<_>, Vec<_>) = runs
        .into_iter()
        .map(|run| live_summary(run, started))
        .partition(|summary| {
            summary
                .run
                .live_stats
                .as_ref()
                .is_some_and(|stats| stats.verdict != LiveVerdict::Incubating)
        });
    ranked.sort_by(|left, right| confidence(right).total_cmp(&confidence(left)));
    Ok(Json(League {
        started,
        ranked,
        too_early,
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
        .start_backtest(&name, request, Timestamp::now().exchange_date())
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
