mod routes;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use buizz_core::protocol::BOT_PATH;
use buizz_feed::FeedManager;
use buizz_ingest::Ingest;
use buizz_market::MarketStore;
use buizz_runner::Runner;
use buizz_state::StateStore;
use tower_http::services::{ServeDir, ServeFile};
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

pub use routes::api_routes;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiConfig {
    pub admin_password: String,
    pub web_dir: Option<PathBuf>,
    pub secure_cookies: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ApiConfig>,
    pub runner: Runner,
    pub market: MarketStore,
    pub state: StateStore,
    pub feed: FeedManager,
    pub ingest: Ingest,
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    api_routes::<()>().into_openapi()
}

pub fn router(app: AppState) -> Router {
    let web_dir = app.config.web_dir.clone();
    let (api, _) = api_routes().split_for_parts();
    let router = api.route(BOT_PATH, get(bot_socket)).with_state(app);
    match web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            router.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
        }
        None => router,
    }
}

pub fn fixture_router() -> Router {
    api_routes::<()>().split_for_parts().0
}

async fn bot_socket(
    State(app): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match token.and_then(|token| app.runner.authorize(token)) {
        Some(run_id) => upgrade
            .on_upgrade(move |socket| async move { app.runner.attach_bot(run_id, socket).await }),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Buizz", version = "1", description = "Spine HTTP API"),
    tags(
        (name = "bots"),
        (name = "runs"),
        (name = "leaderboard"),
        (name = "data"),
        (name = "auth"),
        (name = "events")
    )
)]
struct ApiDoc;

fn base_router<S: Clone + Send + Sync + 'static>() -> OpenApiRouter<S> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
}
