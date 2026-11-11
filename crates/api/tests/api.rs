use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use buizz_api::{ApiConfig, AppState};
use buizz_core::api::{BotSummary, EquitySeries, OrdersPage, PositionView, RunSummary, SpineEvent};
use buizz_core::run::{CostModel, RiskLimits, RunStatus};
use buizz_feed::{FeedConfig, FeedManager};
use buizz_ingest::{Ingest, IngestConfig};
use buizz_market::MarketStore;
use buizz_runner::{Runner, RunnerConfig, RunnerDeps};
use buizz_sources::Sources;
use buizz_sources::alpaca_stream::SYMBOL_CAP;
use buizz_sources::treasury::{self, TreasuryClient, TreasuryConfig};
use buizz_state::StateStore;
use buizz_supervisor::{Backoff, Supervisor, SupervisorConfig};
use reqwest::{Client, Response, StatusCode, header};
use rust_decimal::Decimal;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::time::timeout;

const PASSWORD: &str = "correct horse";
const BOT: &str = "red_green";
const EVENT_DEADLINE: Duration = Duration::from_secs(120);

const RED_GREEN: &str = r#"from buizz import Bot, Context, run


class RedGreen(Bot):
    def on_step(self, ctx: Context) -> None:
        for symbol, bar in ctx.bars.items():
            if bar.close < bar.open and ctx.position(symbol).qty == 0:
                ctx.order_target_weight(symbol, ctx.params["weight"], reason="red day")
            elif bar.close > bar.open:
                ctx.order_target_weight(symbol, 0, reason="green day")


if __name__ == "__main__":
    run(RedGreen)
"#;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root exists")
}

fn write_bot(bots: &Path, sdk: &Path) {
    let dir = bots.join(BOT);
    fs::create_dir_all(&dir).expect("bot directory is created");
    fs::write(dir.join("bot.py"), RED_GREEN).expect("bot source is written");
    let manifest = format!(
        "description = \"Buys red days, sells green days.\"\n\
         command = [\"uv\", \"run\", \"--frozen\", \"--project\", \"{}\", \"python\", \"bot.py\"]\n\
         timeframe = \"1d\"\n\
         wake = \"bar\"\n\n\
         [universe]\n\
         symbols = [\"AAA\", \"BBB\"]\n\n\
         [params]\n\
         weight = 0.25\n",
        sdk.display()
    );
    fs::write(dir.join("bot.toml"), manifest).expect("bot manifest is written");
}

async fn start_spine(data: &Path, bots: &Path, fixtures: &Path) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the listener binds");
    let address = listener.local_addr().expect("the listener has an address");

    let market = MarketStore::open(&data.join(buizz_market::FILE_NAME), 2)
        .await
        .expect("the market store opens");
    market
        .load_fixture(fixtures)
        .await
        .expect("the fixture loads");
    let state = StateStore::open(&data.join(buizz_state::FILE_NAME))
        .await
        .expect("the state store opens");
    let sources = Sources {
        alpaca: None,
        tiingo: None,
        edgar: None,
        treasury: TreasuryClient::new(TreasuryConfig {
            base_url: treasury::BASE_URL.to_owned(),
        })
        .expect("the treasury client builds"),
    };
    let feed = FeedManager::new(
        FeedConfig {
            cap: SYMBOL_CAP,
            stream: None,
        },
        market.clone(),
        None,
    );
    let ingest = Ingest::new(
        IngestConfig {
            tick: Duration::from_secs(60),
            snapshot_dir: data.join("backups"),
            snapshot_keep: 1,
            history_start: chrono::NaiveDate::from_ymd_opt(2016, 1, 1).expect("valid date"),
        },
        market.clone(),
        state.clone(),
        sources,
    );
    let supervisor = Supervisor::new(SupervisorConfig {
        spine_url: format!("ws://{address}"),
        max_parallel_backtests: 1,
        restart: Backoff {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(10),
        },
        kill_grace: Duration::from_secs(5),
    });
    let runner = Runner::new(
        RunnerConfig {
            bots_dir: bots.to_owned(),
            start_cash: Decimal::new(100_000, 0),
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
    runner.recover().await.expect("recovery succeeds");

    let app = buizz_api::router(AppState {
        config: Arc::new(ApiConfig {
            admin_password: PASSWORD.to_owned(),
            web_dir: None,
            secure_cookies: false,
        }),
        runner,
        market,
        state,
        feed,
        ingest,
    });
    tokio::spawn(async move { axum::serve(listener, app).await });
    address
}

async fn login(client: &Client, base: &str, password: &str) -> Response {
    client
        .post(format!("{base}/api/auth/login"))
        .json(&json!({ "password": password }))
        .send()
        .await
        .expect("login is answered")
}

fn session_cookie(response: &Response) -> String {
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("login sets a cookie");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Strict"), "{set_cookie}");
    set_cookie
        .split(';')
        .next()
        .expect("cookie has a value")
        .to_owned()
}

async fn get(client: &Client, cookie: &str, url: String) -> Response {
    client
        .get(url)
        .header(header::COOKIE, cookie)
        .send()
        .await
        .expect("request is answered")
}

async fn until_finished(mut events: Response, run_id: buizz_core::run::RunId) -> Vec<SpineEvent> {
    let mut buffer = String::new();
    let mut seen = Vec::new();
    loop {
        let chunk = timeout(EVENT_DEADLINE, events.chunk())
            .await
            .expect("events arrive in time")
            .expect("the stream reads")
            .expect("the stream stays open");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let frame: String = buffer.drain(..end + 2).collect();
            let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) else {
                continue;
            };
            let event: SpineEvent = serde_json::from_str(data).expect("events are spine events");
            if let SpineEvent::RunStatus {
                run_id: id, status, ..
            } = &event
                && *id == run_id
                && status.is_finished()
            {
                assert_eq!(*status, RunStatus::Completed, "the run ended in {status:?}");
                seen.push(event);
                return seen;
            }
            seen.push(event);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn log_in_start_a_backtest_follow_it_over_sse_and_read_the_result() {
    let temp = tempfile::tempdir().expect("temp dir");
    std::env::set_current_dir(temp.path()).expect("the working directory changes");
    let repository = repository();
    let bots = temp.path().join("bots");
    write_bot(&bots, &repository.join("sdk/python"));
    let address = start_spine(temp.path(), &bots, &repository.join("fixtures/market")).await;
    let base = format!("http://{address}");
    let client = Client::new();

    let refused = client
        .get(format!("{base}/api/bots"))
        .send()
        .await
        .expect("request is answered");
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        login(&client, &base, "wrong").await.status(),
        StatusCode::UNAUTHORIZED
    );

    let granted = login(&client, &base, PASSWORD).await;
    assert_eq!(granted.status(), StatusCode::NO_CONTENT);
    let cookie = session_cookie(&granted);

    let bots: Vec<BotSummary> = get(&client, &cookie, format!("{base}/api/bots"))
        .await
        .json()
        .await
        .expect("bots decode");
    assert_eq!(bots.len(), 1);
    assert_eq!(bots[0].name, BOT);

    let events = get(&client, &cookie, format!("{base}/api/events")).await;
    assert_eq!(events.status(), StatusCode::OK);

    let started = client
        .post(format!("{base}/api/bots/{BOT}/backtests"))
        .header(header::COOKIE, &cookie)
        .json(&json!({ "period": { "from": "2024-11-20", "to": "2024-12-04" } }))
        .send()
        .await
        .expect("backtest request is answered");
    let status = started.status();
    let body = started.text().await.expect("backtest body");
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let run: RunSummary = serde_json::from_str(&body).expect("run decodes");

    let seen = until_finished(events, run.id).await;
    assert!(
        seen.iter()
            .any(|event| matches!(event, SpineEvent::Equity { run_id, .. } if *run_id == run.id))
    );

    let finished: RunSummary = get(&client, &cookie, format!("{base}/api/runs/{}", run.id))
        .await
        .json()
        .await
        .expect("run decodes");
    assert_eq!(finished.status, RunStatus::Completed);
    assert!(finished.result_hash.is_some());
    assert_eq!(finished.metrics.expect("metrics").trades, 3);

    let equity: EquitySeries = get(
        &client,
        &cookie,
        format!("{base}/api/runs/{}/equity", run.id),
    )
    .await
    .json()
    .await
    .expect("equity decodes");
    assert_eq!(equity.points.len(), 10);
    assert_eq!(equity.benchmark.len(), 10);
    assert_eq!(
        equity.points.last().expect("points").equity,
        Decimal::from_str("100448.46").expect("decimal")
    );

    let orders: OrdersPage = get(
        &client,
        &cookie,
        format!("{base}/api/runs/{}/orders", run.id),
    )
    .await
    .json()
    .await
    .expect("orders decode");
    assert_eq!(orders.items.len(), 12);

    let positions: Vec<PositionView> = get(
        &client,
        &cookie,
        format!("{base}/api/runs/{}/positions", run.id),
    )
    .await
    .json()
    .await
    .expect("positions decode");
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].symbol.as_str(), "BBB");
    assert_eq!(positions[0].qty, 511);

    let logout = client
        .post(format!("{base}/api/auth/logout"))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .expect("logout is answered");
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let after_logout = get(&client, &cookie, format!("{base}/api/bots")).await;
    assert_eq!(after_logout.status(), StatusCode::UNAUTHORIZED);
}
