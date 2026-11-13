use axum::extract::DefaultBodyLimit;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::{Api, auth, base_router, bots, data, events, runs, studies};

pub(crate) fn protected_routes() -> OpenApiRouter<Api> {
    base_router()
        .routes(routes!(bots::list_bots))
        .routes(routes!(bots::bot_detail))
        .routes(routes!(bots::start_backtest))
        .routes(routes!(bots::start_sweep))
        .routes(routes!(bots::cancel_backtests))
        .routes(routes!(bots::research))
        .routes(routes!(bots::periods))
        .routes(routes!(bots::go_live))
        .routes(routes!(bots::league))
        .routes(routes!(runs::list_runs))
        .routes(routes!(runs::run_detail))
        .routes(routes!(runs::cancel_run))
        .routes(routes!(runs::pause_run))
        .routes(routes!(runs::resume_run))
        .routes(routes!(runs::run_equity))
        .routes(routes!(runs::run_orders))
        .routes(routes!(runs::run_positions))
        .routes(routes!(runs::run_trades))
        .routes(routes!(runs::run_events))
        .routes(routes!(runs::run_chart))
        .routes(routes!(runs::run_logs))
        .routes(routes!(data::data_health))
        .routes(routes!(data::data_jobs))
        .routes(routes!(data::data_feed))
        .routes(routes!(data::data_processes))
        .routes(routes!(events::events))
        .merge(
            base_router()
                .routes(routes!(studies::event_study))
                .layer(DefaultBodyLimit::max(studies::STUDY_BODY_LIMIT)),
        )
}

pub(crate) fn public_routes() -> OpenApiRouter<Api> {
    base_router()
        .routes(routes!(auth::login))
        .routes(routes!(auth::logout))
        .routes(routes!(auth::session))
        .routes(routes!(crate::healthz))
}
