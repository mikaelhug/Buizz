use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use axum::Router;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use buizz_core::api::{RunSummary, SpineEvent, StartBacktest};
use buizz_core::market::{Adjustment, Symbol, Timeframe};
use buizz_core::protocol::BOT_PATH;
use buizz_core::run::{Period, RunConfig, RunStatus};
use buizz_feed::FeedManager;
use buizz_ingest::Ingest;
use buizz_market::{HistoryRequest, MarketStore};
use buizz_runner::{Runner, RunnerConfig, RunnerDeps};
use buizz_state::StateStore;
use buizz_supervisor::{Backoff, Supervisor, SupervisorConfig};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

async fn bot_socket(
    State(runner): State<Runner>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match token.and_then(|token| runner.authorize(token)) {
        Some(run_id) => {
            upgrade.on_upgrade(move |socket| async move { runner.attach_bot(run_id, socket).await })
        }
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn finished(
    events: &mut broadcast::Receiver<SpineEvent>,
    run: RunSummary,
    state: &StateStore,
) -> RunSummary {
    let wait = async {
        loop {
            match events.recv().await {
                Ok(SpineEvent::RunStatus { run_id, status, .. })
                    if run_id == run.id && status.is_finished() =>
                {
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => panic!("runner events closed"),
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(120), wait)
        .await
        .expect("the backtest finishes");
    let summary = state.run(run.id).await.unwrap().unwrap();
    assert_eq!(
        summary.status,
        RunStatus::Completed,
        "{:?}",
        summary.message
    );
    summary
}

fn assert_same(expected: &Value, actual: &Value, at: &str) {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, value) in expected {
                let actual = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("{at}.{key} is missing"));
                assert_same(value, actual, &format!("{at}.{key}"));
            }
        }
        (Value::Array(expected), Value::Array(actual)) => {
            assert_eq!(expected.len(), actual.len(), "{at} length");
            for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
                assert_same(expected, actual, &format!("{at}[{index}]"));
            }
        }
        (Value::String(expected), Value::String(actual)) => {
            match (Decimal::from_str(expected), Decimal::from_str(actual)) {
                (Ok(expected), Ok(actual)) => assert_eq!(expected, actual, "{at}"),
                _ => assert_eq!(expected, actual, "{at}"),
            }
        }
        (Value::Number(expected), Value::Number(actual))
            if expected.is_f64() || actual.is_f64() =>
        {
            let (expected, actual) = (expected.as_f64().unwrap(), actual.as_f64().unwrap());
            assert!(
                (expected - actual).abs() <= 1e-9,
                "{at}: {actual} vs {expected}"
            );
        }
        _ => assert_eq!(expected, actual, "{at}"),
    }
}

async fn observed(
    runner: &Runner,
    market: &MarketStore,
    state: &StateStore,
    run: &RunSummary,
) -> Value {
    let fills = state.fills(run.id).await.unwrap();
    let orders: Vec<Value> = state
        .orders(run.id, None, 1000)
        .await
        .unwrap()
        .iter()
        .map(|order| {
            let mut value = serde_json::to_value(order).unwrap();
            if let Some(fill) = fills.iter().find(|fill| fill.order_id == order.id) {
                value["fill_price"] = json!(fill.price);
                value["filled_at"] = json!(fill.at);
            }
            value
        })
        .collect();
    let mut portfolio = state.portfolio(run.id).await.unwrap();
    let held: Vec<Symbol> = portfolio.positions.keys().cloned().collect();
    let closes = market
        .history(HistoryRequest {
            symbols: held,
            timeframe: Timeframe::Day,
            bars: 1,
            adjust: Adjustment::Raw,
            as_of: run.clock.unwrap(),
        })
        .await
        .unwrap();
    for close in closes {
        portfolio.mark(&close.symbol, Decimal::from_f64(close.close).unwrap());
    }
    let positions: Vec<Value> = portfolio
        .open_positions()
        .map(|(symbol, position)| {
            json!({"symbol": symbol, "qty": position.qty, "last_price": position.last_price})
        })
        .collect();
    let equity = state.equity(run.id).await.unwrap();
    json!({
        "steps": equity.len(),
        "ledger": state.ledger(run.id).await.unwrap(),
        "orders": orders,
        "equity": equity,
        "benchmark": runner.benchmark(run).await.unwrap(),
        "final": {
            "cash": portfolio.cash,
            "equity": portfolio.equity(),
            "positions": positions,
        },
        "metrics": run.metrics,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn red_green_backtest_through_a_real_bot_matches_the_hand_computation() {
    let workdir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(workdir.path()).unwrap();
    let market = MarketStore::open(&workdir.path().join(buizz_market::FILE_NAME), 2)
        .await
        .unwrap();
    market
        .load_fixture(&root().join("fixtures/market"))
        .await
        .unwrap();
    let state = StateStore::open(&workdir.path().join(buizz_state::FILE_NAME))
        .await
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("fixtures/scenarios/red_green.expected.json"))
            .unwrap(),
    )
    .unwrap();
    let config: RunConfig = serde_json::from_value(expected["config"].clone()).unwrap();
    let runner = Runner::new(
        RunnerConfig {
            bots_dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bots"),
            start_cash: config.start_cash,
            cost: config.cost,
            risk: config.risk,
        },
        RunnerDeps {
            market: market.clone(),
            state: state.clone(),
            supervisor: Supervisor::new(SupervisorConfig {
                spine_url: format!("ws://127.0.0.1:{port}"),
                max_parallel_backtests: 1,
                restart: Backoff {
                    initial: Duration::from_secs(1),
                    max: Duration::from_secs(1),
                },
                kill_grace: Duration::from_secs(5),
            }),
            feed: FeedManager {},
            ingest: Ingest {},
        },
    );
    let app = Router::new()
        .route(BOT_PATH, get(bot_socket))
        .with_state(runner.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    let mut events = runner.events();
    let request = StartBacktest {
        preset: None,
        period: Some(Period {
            from: config.start,
            to: config.end.unwrap(),
        }),
        params: Some(config.params.clone()),
        start_cash: Some(config.start_cash),
        cost: Some(config.cost),
        risk: Some(config.risk),
        seed: Some(config.seed),
        allow_holdout: false,
    };
    let today = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();

    let queued = runner
        .start_backtest("red_green", request.clone(), today)
        .await
        .unwrap();
    let first = finished(&mut events, queued, &state).await;

    assert_eq!(first.config, config);
    let actual = observed(&runner, &market, &state, &first).await;
    for key in [
        "steps",
        "ledger",
        "orders",
        "equity",
        "benchmark",
        "final",
        "metrics",
    ] {
        assert_same(&expected[key], &actual[key], key);
    }

    let queued = runner
        .start_backtest("red_green", request, today)
        .await
        .unwrap();
    let second = finished(&mut events, queued, &state).await;
    assert!(first.result_hash.is_some());
    assert_eq!(second.result_hash, first.result_hash);
    runner.shutdown().await;
}
