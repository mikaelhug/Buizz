use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use buizz_api::{ApiConfig, AppState};
use buizz_core::run::{CostModel, RiskLimits};
use buizz_feed::{FeedConfig, FeedManager};
use buizz_ingest::{Ingest, IngestConfig};
use buizz_market::MarketStore;
use buizz_runner::{Runner, RunnerConfig, RunnerDeps};
use buizz_sources::Sources;
use buizz_sources::alpaca::HISTORY_START;
use buizz_sources::treasury::{self, TreasuryClient, TreasuryConfig};
use buizz_state::StateStore;
use buizz_supervisor::{Backoff, Supervisor, SupervisorConfig};
use rust_decimal::Decimal;
use tempfile::TempDir;
use tower::ServiceExt;

struct Spine {
    router: Router,
    _data: TempDir,
}

async fn spine() -> Spine {
    let data = tempfile::tempdir().expect("temp dir");
    let web = data.path().join("web");
    std::fs::create_dir_all(&web).expect("web dir");
    std::fs::write(web.join("index.html"), "<!doctype html>").expect("index");
    let market = MarketStore::open(&data.path().join(buizz_market::FILE_NAME), 1)
        .await
        .expect("market store");
    let state = StateStore::open(&data.path().join(buizz_state::FILE_NAME))
        .await
        .expect("state store");
    let sources = Sources {
        alpaca: None,
        tiingo: None,
        edgar: None,
        treasury: TreasuryClient::new(TreasuryConfig {
            base_url: treasury::BASE_URL.to_owned(),
        })
        .expect("treasury client"),
    };
    let feed = FeedManager::new(
        FeedConfig {
            cap: 1,
            stream: None,
        },
        market.clone(),
        None,
    );
    let ingest = Ingest::new(
        IngestConfig {
            tick: Duration::from_secs(60),
            snapshot_dir: data.path().join("backups"),
            snapshot_keep: 1,
            history_start: HISTORY_START,
            enabled: false,
        },
        market.clone(),
        state.clone(),
        sources,
    );
    let supervisor = Supervisor::new(SupervisorConfig {
        spine_url: "ws://127.0.0.1:1".to_owned(),
        max_parallel_backtests: 1,
        restart: Backoff {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(1),
        },
        kill_grace: Duration::from_secs(1),
    });
    let runner = Runner::new(
        RunnerConfig {
            bots_dir: data.path().join("bots"),
            start_cash: Decimal::from(100_000),
            cost: CostModel::default(),
            risk: RiskLimits::default(),
        },
        RunnerDeps {
            market: market.clone(),
            state: state.clone(),
            supervisor,
            feed: feed.clone(),
            ingest: ingest.clone(),
        },
    );
    let router = buizz_api::router(AppState {
        config: Arc::new(ApiConfig {
            admin_password: "correct horse battery".to_owned(),
            web_dir: Some(web),
            secure_cookies: false,
        }),
        runner,
        market,
        state,
        feed,
        ingest,
    });
    Spine {
        router,
        _data: data,
    }
}

struct Reply {
    status: StatusCode,
    content_type: Option<String>,
    body: String,
}

async fn send(spine: &Spine, request: Request<Body>) -> Reply {
    let response = spine
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("infallible router");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    Reply {
        status,
        content_type,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

fn get(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).expect("request")
}

#[tokio::test]
async fn health_answers_without_a_session() {
    let spine = spine().await;
    let reply = send(&spine, get("/healthz")).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn unknown_api_paths_are_json_not_found_while_app_paths_serve_the_ui() {
    let spine = spine().await;
    let reply = send(&spine, get("/api/nope")).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.content_type.as_deref(), Some("application/json"));
    assert!(reply.body.contains("\"error\""));
    let page = send(&spine, get("/bots/red_green")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.starts_with("<!doctype html>"));
}
