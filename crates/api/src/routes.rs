use std::convert::Infallible;

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use buizz_core::api::{
    ApiError, BotDetail, BotSummary, DataHealth, EquitySeries, EventsPage, FeedSlots, JobView,
    Leaderboard, LeaderboardQuery, LoginRequest, OrdersPage, PageQuery, PositionView, RunSummary,
    RunsQuery, SESSION_COOKIE, SessionInfo, SpineEvent, StartBacktest,
};
use buizz_core::run::RunId;
use futures::{Stream, StreamExt, stream};
use serde::de::DeserializeOwned;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

fn fixture<T: DeserializeOwned>(name: &str, text: &str) -> T {
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("fixtures/api/{name}.json does not match its type: {error}"))
}

macro_rules! fixture {
    ($name:literal) => {
        fixture($name, include_str!(concat!("../../../fixtures/api/", $name, ".json")))
    };
}

pub fn api_routes<S: Clone + Send + Sync + 'static>() -> OpenApiRouter<S> {
    crate::base_router()
        .routes(routes!(list_bots))
        .routes(routes!(bot_detail))
        .routes(routes!(start_backtest))
        .routes(routes!(go_live))
        .routes(routes!(pause))
        .routes(routes!(list_runs))
        .routes(routes!(run_detail))
        .routes(routes!(cancel_run))
        .routes(routes!(run_equity))
        .routes(routes!(run_orders))
        .routes(routes!(run_positions))
        .routes(routes!(run_events))
        .routes(routes!(leaderboard))
        .routes(routes!(data_health))
        .routes(routes!(data_jobs))
        .routes(routes!(data_feed))
        .routes(routes!(login))
        .routes(routes!(logout))
        .routes(routes!(session))
        .routes(routes!(events))
}

#[utoipa::path(get, path = "/api/bots", tag = "bots",
    responses((status = 200, body = Vec<BotSummary>)))]
async fn list_bots() -> Json<Vec<BotSummary>> {
    Json(fixture!("bots"))
}

#[utoipa::path(get, path = "/api/bots/{name}", tag = "bots",
    params(("name" = String, Path)),
    responses((status = 200, body = BotDetail), (status = 404, body = ApiError)))]
async fn bot_detail(Path(_name): Path<String>) -> Json<BotDetail> {
    Json(fixture!("bot_detail"))
}

#[utoipa::path(post, path = "/api/bots/{name}/backtests", tag = "bots",
    params(("name" = String, Path)),
    request_body = StartBacktest,
    responses(
        (status = 201, body = RunSummary),
        (status = 400, body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, description = "holdout locked or data gaps", body = ApiError)))]
async fn start_backtest(
    Path(_name): Path<String>,
    Json(_request): Json<StartBacktest>,
) -> (StatusCode, Json<RunSummary>) {
    (StatusCode::CREATED, Json(fixture!("backtest_started")))
}

#[utoipa::path(post, path = "/api/bots/{name}/live", tag = "bots",
    params(("name" = String, Path)),
    responses(
        (status = 200, body = RunSummary),
        (status = 404, body = ApiError),
        (status = 409, description = "stream symbol cap reached", body = ApiError)))]
async fn go_live(Path(_name): Path<String>) -> Json<RunSummary> {
    Json(fixture!("live_started"))
}

#[utoipa::path(post, path = "/api/bots/{name}/pause", tag = "bots",
    params(("name" = String, Path)),
    responses((status = 200, body = RunSummary), (status = 404, body = ApiError), (status = 409, body = ApiError)))]
async fn pause(Path(_name): Path<String>) -> Json<RunSummary> {
    Json(fixture!("paused"))
}

#[utoipa::path(get, path = "/api/runs", tag = "runs",
    params(RunsQuery),
    responses((status = 200, body = Vec<RunSummary>)))]
async fn list_runs(Query(_query): Query<RunsQuery>) -> Json<Vec<RunSummary>> {
    Json(fixture!("runs"))
}

#[utoipa::path(get, path = "/api/runs/{id}", tag = "runs",
    params(("id" = RunId, Path)),
    responses((status = 200, body = RunSummary), (status = 404, body = ApiError)))]
async fn run_detail(Path(_id): Path<RunId>) -> Json<RunSummary> {
    Json(fixture!("run"))
}

#[utoipa::path(post, path = "/api/runs/{id}/cancel", tag = "runs",
    params(("id" = RunId, Path)),
    responses((status = 200, body = RunSummary), (status = 404, body = ApiError), (status = 409, body = ApiError)))]
async fn cancel_run(Path(_id): Path<RunId>) -> Json<RunSummary> {
    Json(fixture!("run"))
}

#[utoipa::path(get, path = "/api/runs/{id}/equity", tag = "runs",
    params(("id" = RunId, Path)),
    responses((status = 200, body = EquitySeries), (status = 404, body = ApiError)))]
async fn run_equity(Path(_id): Path<RunId>) -> Json<EquitySeries> {
    Json(fixture!("run_equity"))
}

#[utoipa::path(get, path = "/api/runs/{id}/orders", tag = "runs",
    params(("id" = RunId, Path), PageQuery),
    responses((status = 200, body = OrdersPage), (status = 404, body = ApiError)))]
async fn run_orders(Path(_id): Path<RunId>, Query(_page): Query<PageQuery>) -> Json<OrdersPage> {
    Json(fixture!("run_orders"))
}

#[utoipa::path(get, path = "/api/runs/{id}/positions", tag = "runs",
    params(("id" = RunId, Path)),
    responses((status = 200, body = Vec<PositionView>), (status = 404, body = ApiError)))]
async fn run_positions(Path(_id): Path<RunId>) -> Json<Vec<PositionView>> {
    Json(fixture!("run_positions"))
}

#[utoipa::path(get, path = "/api/runs/{id}/events", tag = "runs",
    params(("id" = RunId, Path), PageQuery),
    responses((status = 200, body = EventsPage), (status = 404, body = ApiError)))]
async fn run_events(Path(_id): Path<RunId>, Query(_page): Query<PageQuery>) -> Json<EventsPage> {
    Json(fixture!("run_events"))
}

#[utoipa::path(get, path = "/api/leaderboard", tag = "leaderboard",
    params(LeaderboardQuery),
    responses((status = 200, body = Leaderboard)))]
async fn leaderboard(Query(_query): Query<LeaderboardQuery>) -> Json<Leaderboard> {
    Json(fixture!("leaderboard"))
}

#[utoipa::path(get, path = "/api/data/health", tag = "data",
    responses((status = 200, body = DataHealth)))]
async fn data_health() -> Json<DataHealth> {
    Json(fixture!("data_health"))
}

#[utoipa::path(get, path = "/api/data/jobs", tag = "data",
    responses((status = 200, body = Vec<JobView>)))]
async fn data_jobs() -> Json<Vec<JobView>> {
    Json(fixture!("data_jobs"))
}

#[utoipa::path(get, path = "/api/data/feed", tag = "data",
    responses((status = 200, body = FeedSlots)))]
async fn data_feed() -> Json<FeedSlots> {
    Json(fixture!("data_feed"))
}

#[utoipa::path(post, path = "/api/auth/login", tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 204, description = "session cookie set", headers(("set-cookie" = String))),
        (status = 401, body = ApiError)))]
async fn login(Json(_request): Json<LoginRequest>) -> Response {
    let cookie = format!("{SESSION_COOKIE}=fixture; Path=/; HttpOnly; SameSite=Strict");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

#[utoipa::path(post, path = "/api/auth/logout", tag = "auth",
    responses((status = 204, description = "session cookie cleared")))]
async fn logout() -> Response {
    let cookie = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

#[utoipa::path(get, path = "/api/auth/session", tag = "auth",
    responses((status = 200, body = SessionInfo)))]
async fn session() -> Json<SessionInfo> {
    Json(fixture!("session"))
}

#[utoipa::path(get, path = "/api/events", tag = "events",
    responses((status = 200, description = "server-sent events; the event name is the `type` field",
        content_type = "text/event-stream", body = SpineEvent)))]
async fn events() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events: Vec<SpineEvent> = fixture!("events");
    let frames = events.into_iter().map(|event| {
        Ok(Event::default()
            .event(event.name())
            .json_data(&event)
            .expect("spine events serialize"))
    });
    Sse::new(stream::iter(frames).chain(stream::pending())).keep_alive(KeepAlive::default())
}
