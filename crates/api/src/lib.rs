mod auth;
mod bots;
mod data;
mod error;
mod events;
mod extract;
mod guard;
mod routes;
mod runs;
mod studies;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::extract::{FromRef, State, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{Next, from_fn, from_fn_with_state, map_response};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use buizz_core::protocol::BOT_PATH;
use buizz_feed::FeedManager;
use buizz_ingest::Ingest;
use buizz_market::MarketStore;
use buizz_runner::Runner;
use buizz_state::StateStore;
use tokio::sync::watch;
use tower_http::services::{ServeDir, ServeFile};
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

use crate::auth::{Auth, require_session};
use crate::error::Failure;
use crate::guard::{reject_cross_site, security_headers, spine_host_only};

const HASHED_ASSETS: &str = "/assets/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiConfig {
    pub admin_username: String,
    pub admin_password: String,
    pub web_dir: Option<PathBuf>,
    pub secure_cookies: bool,
    pub spine_ip: IpAddr,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ApiConfig>,
    pub runner: Runner,
    pub market: MarketStore,
    pub state: StateStore,
    pub feed: FeedManager,
    pub ingest: Ingest,
    pub shutdown: watch::Receiver<bool>,
}

#[derive(Clone)]
struct Api {
    app: AppState,
    auth: Arc<Auth>,
}

impl FromRef<Api> for AppState {
    fn from_ref(api: &Api) -> Self {
        api.app.clone()
    }
}

impl FromRef<Api> for Arc<Auth> {
    fn from_ref(api: &Api) -> Self {
        api.auth.clone()
    }
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    routes::protected_routes()
        .merge(routes::public_routes())
        .into_openapi()
}

pub fn router(app: AppState) -> Router {
    let web_dir = app.config.web_dir.clone();
    let api = Api {
        auth: Arc::new(Auth::new(&app.config)),
        app,
    };
    let (protected, _) = routes::protected_routes().split_for_parts();
    let (public, _) = routes::public_routes().split_for_parts();
    let router = protected
        .route_layer(from_fn_with_state(api.auth.clone(), require_session))
        .merge(public)
        .route(
            BOT_PATH,
            get(bot_socket)
                .route_layer(from_fn_with_state(api.app.config.spine_ip, spine_host_only)),
        )
        .route("/api", any(unknown_api_route))
        .route("/api/{*rest}", any(unknown_api_route))
        .with_state(api);
    let router = match web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            let bundle = Router::new()
                .fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
                .layer(from_fn(bundle_caching));
            router.fallback_service(bundle)
        }
        None => router,
    };
    router
        .layer(from_fn(reject_cross_site))
        .layer(map_response(security_headers))
}

async fn bundle_caching(request: Request, next: Next) -> Response {
    let hashed = request.uri().path().starts_with(HASHED_ASSETS);
    let mut response = next.run(request).await;
    let page = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|kind| kind.as_bytes().starts_with(b"text/html"));
    let policy = if hashed && !page && response.status().is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    response
}

#[utoipa::path(get, path = "/healthz", tag = "health",
    responses((status = 204, description = "the spine is serving requests")))]
pub(crate) async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn unknown_api_route() -> Failure {
    Failure::new(StatusCode::NOT_FOUND, "no such API route")
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
        (name = "league"),
        (name = "research"),
        (name = "data"),
        (name = "auth"),
        (name = "events"),
        (name = "health")
    )
)]
struct ApiDoc;

fn base_router() -> OpenApiRouter<Api> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
}
