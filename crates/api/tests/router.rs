use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderName, Request, StatusCode, header};
use buizz_api::{ApiConfig, AppState};
use buizz_core::api::Leaderboard;
use buizz_core::performance::Metrics;
use buizz_core::protocol::BOT_PATH;
use buizz_core::run::{
    BotManifest, CostModel, DesiredState, Mode, PeriodPreset, RiskLimits, RunConfig, RunId,
    RunStatus,
};
use buizz_core::time::Timestamp;
use buizz_feed::{FeedConfig, FeedManager};
use buizz_ingest::{Ingest, IngestConfig};
use buizz_market::MarketStore;
use buizz_runner::{Runner, RunnerConfig, RunnerDeps};
use buizz_sources::Sources;
use buizz_sources::alpaca::HISTORY_START;
use buizz_sources::treasury::{self, TreasuryClient, TreasuryConfig};
use buizz_state::{NewRun, RunOutcome, StateStore};
use buizz_supervisor::{Supervisor, SupervisorConfig};
use rust_decimal::Decimal;
use tempfile::TempDir;
use tower::ServiceExt;

const SPINE_IP: [u8; 4] = [10, 42, 0, 9];

struct Spine {
    router: Router,
    state: StateStore,
    data: TempDir,
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
        fred: None,
        treasury: TreasuryClient::new(TreasuryConfig {
            base_url: treasury::BASE_URL.to_owned(),
        })
        .expect("treasury client"),
        wikipedia: None,
        ibkr: None,
        nasdaq: None,
        factset: None,
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
            only: None,
            enabled: false,
        },
        market.clone(),
        state.clone(),
        sources,
    );
    let supervisor = Supervisor::new(SupervisorConfig {
        spine_url: "ws://127.0.0.1:1".to_owned(),
        log_dir: data.path().join("logs"),
        max_parallel_backtests: 1,
        restart_delay: Duration::from_secs(1),
        kill_grace: Duration::from_secs(1),
    });
    let runner = Runner::new(
        RunnerConfig {
            bots_dir: data.path().join("bots"),
            start_cash: Decimal::from(100_000),
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
            spine_ip: IpAddr::from(SPINE_IP),
        }),
        runner,
        market,
        state: state.clone(),
        feed,
        ingest,
        shutdown: tokio::sync::watch::channel(false).1,
    });
    Spine {
        router,
        state,
        data,
    }
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn header(&self, name: HeaderName) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

async fn send(spine: &Spine, request: Request<Body>) -> Reply {
    let response = spine
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("infallible router");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    Reply {
        status,
        headers,
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
    assert_eq!(reply.header(header::CONTENT_TYPE), Some("application/json"));
    assert!(reply.body.contains("\"error\""));
    let page = send(&spine, get("/bots/red_green")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.starts_with("<!doctype html>"));
}

async fn session_cookie(spine: &Spine) -> String {
    let login = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"correct horse battery"}"#))
        .expect("request");
    let response = spine
        .router
        .clone()
        .oneshot(login)
        .await
        .expect("infallible router");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .expect("session cookie")
        .to_owned()
}

#[tokio::test]
async fn malformed_inputs_answer_with_an_api_error() {
    let spine = spine().await;
    let cookie = session_cookie(&spine).await;
    let bad_path = Request::get("/api/runs/abc")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .expect("request");
    let bad_query = Request::get("/api/runs?limit=many")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .expect("request");
    let bad_body = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{"))
        .expect("request");
    for request in [bad_path, bad_query, bad_body] {
        let reply = send(&spine, request).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);
        assert_eq!(reply.header(header::CONTENT_TYPE), Some("application/json"));
        assert!(reply.body.starts_with("{\"error\":"), "{}", reply.body);
    }
}

#[tokio::test]
async fn pages_forbid_framing_sniffing_and_foreign_scripts() {
    let spine = spine().await;
    let page = send(&spine, get("/")).await;
    assert_eq!(page.status, StatusCode::OK);
    let policy = page
        .header(header::CONTENT_SECURITY_POLICY)
        .expect("content security policy");
    assert!(policy.contains("script-src 'self'"));
    assert!(policy.contains("frame-ancestors 'none'"));
    assert_eq!(page.header(header::X_CONTENT_TYPE_OPTIONS), Some("nosniff"));
    assert_eq!(page.header(header::X_FRAME_OPTIONS), Some("DENY"));
    assert_eq!(page.header(header::REFERRER_POLICY), Some("same-origin"));
}

fn bot_dial(peer: [u8; 4]) -> Request<Body> {
    let mut request = Request::get(BOT_PATH)
        .header(header::AUTHORIZATION, "Bearer guessed")
        .body(Body::empty())
        .expect("request");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((peer, 40_000))));
    request
}

#[tokio::test]
async fn the_bot_socket_refuses_peers_outside_the_spine_host() {
    let spine = spine().await;
    let outside = send(&spine, bot_dial([10, 42, 0, 7])).await;
    assert_eq!(outside.status, StatusCode::FORBIDDEN);
    let local = send(&spine, bot_dial([127, 0, 0, 1])).await;
    assert_ne!(local.status, StatusCode::FORBIDDEN);
    let bound = send(&spine, bot_dial(SPINE_IP)).await;
    assert_ne!(bound.status, StatusCode::FORBIDDEN);
}

const STEADY: &str = r#"
command = ["python", "bot.py"]
timeframe = "1d"
universe = { symbols = ["SPY"] }
params = { lookback = 20 }
risk = { max_drawdown = 0.2 }
"#;

async fn completed_backtest(spine: &Spine, manifest: &BotManifest, risk: RiskLimits) -> RunId {
    let created_at = Timestamp::now();
    let period = PeriodPreset::OneYear.resolve(created_at.exchange_date());
    let config = RunConfig {
        bot: "steady".to_owned(),
        mode: Mode::Backtest,
        params: manifest.params.clone(),
        universe: manifest.universe.clone(),
        timeframe: manifest.timeframe,
        wake: manifest.wake,
        start: period.from,
        end: Some(period.to),
        start_cash: Decimal::from(100_000),
        cost: CostModel::default(),
        risk,
        seed: 1,
        benchmark: manifest.benchmark.clone(),
        holdout: false,
    };
    let id = spine
        .state
        .create_run(NewRun {
            config,
            status: RunStatus::Running,
            desired: DesiredState::Running,
            bot_hash: String::new(),
            engine_version: String::new(),
            data_watermark: None,
            badges: Vec::new(),
            created_at,
        })
        .await
        .expect("run created");
    spine
        .state
        .finish_run(
            id,
            RunOutcome {
                status: RunStatus::Completed,
                finished_at: Timestamp::now(),
                result_hash: None,
                metrics: Some(Metrics::default()),
                message: None,
                badge: None,
            },
        )
        .await
        .expect("run finished");
    id
}

#[tokio::test]
async fn the_leaderboard_ranks_a_bot_by_its_run_as_configured_not_by_an_override() {
    let spine = spine().await;
    let bot = spine.data.path().join("bots").join("steady");
    std::fs::create_dir_all(&bot).expect("bot dir");
    std::fs::write(bot.join(BotManifest::FILE_NAME), STEADY).expect("bot.toml");
    let manifest = BotManifest::from_toml(STEADY).expect("manifest");
    let configured = completed_backtest(&spine, &manifest, manifest.risk).await;
    completed_backtest(&spine, &manifest, RiskLimits::default()).await;

    let cookie = session_cookie(&spine).await;
    let request = Request::get("/api/leaderboard?preset=1y")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .expect("request");
    let reply = send(&spine, request).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let leaderboard: Leaderboard = serde_json::from_str(&reply.body).expect("leaderboard");
    let ranked: Vec<RunId> = leaderboard.rows.iter().map(|row| row.run_id).collect();
    assert_eq!(
        ranked,
        [configured],
        "the newer run without the drawdown stop does not rank"
    );
}
