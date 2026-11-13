use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use buizz_core::api::{
    ApiError, BotDetail, BotStage, BotStatus, BotSummary, CancelledBacktests, GoLive, League,
    LiveSummary, Research, RunSummary, StartBacktest, StartSweep, SweepStarted,
};
use buizz_core::performance::LiveVerdict;
use buizz_core::run::{DesiredState, Mode, PeriodPreset, PresetPeriod, RunBadge, RunStatus};
use buizz_core::time::Timestamp;
use buizz_runner::Benchmarks;
use buizz_state::RunFilter;
use buizz_supervisor::BotSpec;
use futures::future::{try_join, try_join_all};

use crate::AppState;
use crate::error::Failure;
use crate::extract::{JsonBody, Path};

const RECENT_RUNS: u32 = 50;

fn bot_status(runs: &[RunSummary], live: &[&RunSummary]) -> BotStatus {
    let paused =
        |run: &&RunSummary| run.status == RunStatus::Paused || run.desired == DesiredState::Paused;
    if live.iter().any(|run| !paused(run)) {
        return BotStatus::Live;
    }
    if !live.is_empty() {
        return BotStatus::Paused;
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

fn recent_runs(bot: &str) -> RunFilter {
    RunFilter {
        bot: Some(bot.to_owned()),
        limit: Some(RECENT_RUNS),
        ..RunFilter::default()
    }
}

async fn summarize(
    app: &AppState,
    bot: &BotSpec,
    runs: &[RunSummary],
    live: &[RunSummary],
    started: u32,
    benchmarks: &Benchmarks,
) -> Result<BotSummary, Failure> {
    let live: Vec<&RunSummary> = live.iter().filter(|run| run.bot == bot.name).collect();
    let (screened, summaries) = try_join(
        async { Ok::<_, Failure>(app.runner.screening(bot, benchmarks).await?) },
        try_join_all(live.iter().map(|run| async {
            Ok::<_, Failure>(app.runner.live_summary((*run).clone(), started).await?)
        })),
    )
    .await?;
    let passing = screened
        .screening
        .is_some_and(|screening| screening.passing > 0);
    let stage = match (live.is_empty(), passing) {
        (false, _) => BotStage::Live,
        (true, true) => BotStage::Screened,
        (true, false) => BotStage::Draft,
    };
    Ok(BotSummary {
        name: bot.name.clone(),
        description: bot.manifest.description.clone(),
        timeframe: bot.manifest.timeframe,
        params: bot.manifest.params.clone(),
        stage,
        status: bot_status(runs, &live),
        screening: screened.screening,
        backtest: screened.backtest,
        live: summaries,
        last_run_at: runs.first().map(|run| run.created_at),
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
    let benchmarks = app.runner.benchmarks();
    let summaries = try_join_all(scan.bots.iter().map(|spec| async {
        let runs = app.state.runs(recent_runs(&spec.name)).await?;
        summarize(&app, spec, &runs, &live, started, &benchmarks).await
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
    let ((runs, live), (started, holdout_runs)) = try_join(
        try_join(
            app.state.runs(recent_runs(&name)),
            app.state.runs(live_runs(Some(&name), &RunStatus::ACTIVE)),
        ),
        try_join(app.state.live_runs_started(), app.state.holdout_runs(&name)),
    )
    .await?;
    Ok(Json(BotDetail {
        summary: summarize(&app, spec, &runs, &live, started, &app.runner.benchmarks()).await?,
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
    let summaries = try_join_all(
        runs.into_iter()
            .map(|run| app.runner.live_summary(run, started)),
    )
    .await?;
    let (mut ranked, too_early): (Vec<_>, Vec<_>) = summaries
        .into_iter()
        .partition(|summary| {
            summary
                .run
                .live_stats
                .as_ref()
                .is_some_and(|stats| stats.sessions >= LiveVerdict::TRACKING_SESSIONS)
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

#[utoipa::path(post, path = "/api/bots/{name}/sweeps", tag = "bots",
    params(("name" = String, Path)),
    request_body = StartSweep,
    responses(
        (status = 201, body = SweepStarted),
        (status = 400, body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, description = "data gaps", body = ApiError)))]
pub(crate) async fn start_sweep(
    State(app): State<AppState>,
    Path(name): Path<String>,
    JsonBody(request): JsonBody<StartSweep>,
) -> Result<(StatusCode, Json<SweepStarted>), Failure> {
    let sweep = app
        .runner
        .sweep(&name, request, Timestamp::now().exchange_date())
        .await?;
    Ok((StatusCode::CREATED, Json(sweep)))
}

#[utoipa::path(post, path = "/api/bots/{name}/backtests/cancel", tag = "bots",
    params(("name" = String, Path)),
    responses((status = 200, body = CancelledBacktests), (status = 404, body = ApiError)))]
pub(crate) async fn cancel_backtests(
    State(app): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CancelledBacktests>, Failure> {
    Ok(Json(CancelledBacktests {
        cancelled: app.runner.cancel_queued(&name).await?,
    }))
}

#[utoipa::path(get, path = "/api/periods", tag = "bots",
    responses((status = 200, body = Vec<PresetPeriod>)))]
pub(crate) async fn periods() -> Json<Vec<PresetPeriod>> {
    let today = Timestamp::now().exchange_date();
    Json(
        PeriodPreset::SCREENING
            .into_iter()
            .chain([PeriodPreset::Holdout])
            .map(|preset| preset.described(today))
            .collect(),
    )
}

#[utoipa::path(get, path = "/api/bots/{name}/research", tag = "bots",
    params(("name" = String, Path)),
    responses((status = 200, body = Research), (status = 404, body = ApiError)))]
pub(crate) async fn research(
    State(app): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Research>, Failure> {
    Ok(Json(app.runner.research(&name).await?))
}

#[utoipa::path(post, path = "/api/bots/{name}/live", tag = "bots",
    params(("name" = String, Path)),
    request_body = GoLive,
    responses(
        (status = 200, body = RunSummary),
        (status = 400, body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, description = "already live with these params, three live variants, or the stream symbol cap", body = ApiError)))]
pub(crate) async fn go_live(
    State(app): State<AppState>,
    Path(name): Path<String>,
    JsonBody(request): JsonBody<GoLive>,
) -> Result<Json<RunSummary>, Failure> {
    Ok(Json(app.runner.go_live(&name, request.from_run).await?))
}
