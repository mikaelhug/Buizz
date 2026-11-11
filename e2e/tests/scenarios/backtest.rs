use buizz_core::api::{EquitySeries, PositionView, RunSummary, SpineEvent};
use buizz_core::run::{RunConfig, RunId};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::browser;
use crate::expect::{assert_same, scenario};
use crate::fake::FakeProviders;
use crate::spine::{Options, Spine};
use crate::world::{World, repository};

async fn cli_backtest(spine: &Spine, config: &RunConfig) -> (RunId, Vec<String>) {
    let output = Command::new("uv")
        .args(["run", "--frozen", "--quiet", "--project"])
        .arg(repository().join("sdk/python"))
        .args(["buizz", "backtest", "bots/red_green", "--from"])
        .arg(config.start.to_string())
        .arg("--to")
        .arg(config.end.expect("a backtest end").to_string())
        .env("BUIZZ_URL", &spine.base)
        .env("BUIZZ_ADMIN_PASSWORD", &spine.password)
        .output()
        .await
        .expect("the buizz CLI runs");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<String> = stdout.lines().map(str::to_owned).collect();
    let id = lines[0]
        .strip_prefix("run ")
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|id| id.parse().ok())
        .map(RunId)
        .unwrap_or_else(|| panic!("the CLI names its run: {stdout}"));
    (id, lines)
}

fn printed_metric(name: &str, value: &Value) -> String {
    match value.as_f64() {
        Some(number) if value.is_f64() => format!("{name:<16} {number:.4}"),
        _ => format!("{name:<16} {value}"),
    }
}

async fn observed(spine: &Spine, run: &RunSummary) -> Value {
    let orders: Vec<Value> = spine
        .orders(run.id)
        .await
        .iter()
        .map(|order| {
            let mut value = serde_json::to_value(order).expect("orders encode");
            if order.order.filled_qty > 0 {
                value["fill_price"] = json!(order.avg_fill_price);
                value["filled_at"] = json!(order.order.updated_at);
            }
            value
        })
        .collect();
    let equity: EquitySeries = spine.get(&format!("/api/runs/{}/equity", run.id)).await;
    let positions: Vec<PositionView> = spine.get(&format!("/api/runs/{}/positions", run.id)).await;
    let last = equity.points.last().expect("an equity point per session");
    json!({
        "steps": equity.points.len(),
        "orders": orders,
        "equity": equity.points,
        "benchmark": equity.benchmark,
        "final": {"cash": last.cash, "equity": last.equity, "positions": positions},
        "metrics": run.metrics,
    })
}

fn assert_ledger_fills(expected: &Value, orders: &Value) {
    for entry in expected["ledger"].as_array().expect("a ledger") {
        let kind = &entry["kind"];
        if kind["type"] != "fill" {
            continue;
        }
        let order = orders
            .as_array()
            .expect("orders")
            .iter()
            .find(|order| order["id"] == kind["order_id"])
            .unwrap_or_else(|| panic!("the order behind ledger entry {entry}"));
        let signed = if order["side"] == "buy" { 1 } else { -1 }
            * order["filled_qty"].as_i64().expect("qty");
        assert_eq!(json!(signed), kind["qty"], "{entry}");
        assert_eq!(order["symbol"], kind["symbol"], "{entry}");
        assert_same(&kind["price"], &order["fill_price"], 0.0, "ledger price");
        assert_same(&entry["at"], &order["filled_at"], 0.0, "ledger time");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn red_green_backtest_matches_the_hand_computation_from_the_cli_the_api_and_the_ui() {
    let expected = scenario("red_green");
    let config: RunConfig = serde_json::from_value(expected["config"].clone()).expect("config");
    let fake = FakeProviders::serve(World::fixture_market().await).await;
    let spine = Spine::start(
        fake,
        Options {
            bots: &["red_green"],
            tiingo: false,
        },
    )
    .await;
    spine.ingested().await;
    assert_eq!(spine.anonymous("/api/bots").await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        spine.login("wrong").await.status(),
        StatusCode::UNAUTHORIZED
    );

    let (cli_run, printed) = cli_backtest(&spine, &config).await;
    for (name, value) in expected["metrics"].as_object().expect("metrics") {
        let line = printed_metric(name, value);
        assert!(printed.contains(&line), "{line:?} in {printed:#?}");
    }
    for line in [
        "orders: 12 (7 filled, 5 rejected)",
        "  AAA      net +0 shares",
        "  BBB      net +511 shares",
    ] {
        assert!(
            printed.iter().any(|printed| printed == line),
            "{line:?} in {printed:#?}"
        );
    }

    let mut events = spine.subscribe().await;
    let request = json!({
        "period": {"from": config.start, "to": config.end},
        "params": config.params,
        "start_cash": config.start_cash,
        "cost": config.cost,
        "risk": config.risk,
        "seed": config.seed,
    });
    let queued: RunSummary = spine
        .post("/api/bots/red_green/backtests", request.clone())
        .await;
    let mut equity_events = 0;
    loop {
        match events.next().await {
            SpineEvent::Equity { run_id, .. } if run_id == queued.id => equity_events += 1,
            SpineEvent::RunStatus { run_id, status, .. }
                if run_id == queued.id && status.is_finished() =>
            {
                break;
            }
            _ => {}
        }
    }
    assert_eq!(equity_events, 10, "one equity event per session close");
    let first = spine.finished(queued.id).await;
    assert_eq!(first.config, config);

    let actual = observed(&spine, &first).await;
    let tolerance = expected["metrics_tolerance"].as_f64().expect("a tolerance");
    for key in ["steps", "orders", "equity", "benchmark", "final", "metrics"] {
        assert_same(&expected[key], &actual[key], tolerance, key);
    }
    assert_ledger_fills(&expected, &actual["orders"]);

    let rerun: RunSummary = spine.post("/api/bots/red_green/backtests", request).await;
    let second = spine.finished(rerun.id).await;
    assert!(first.result_hash.is_some());
    assert_eq!(second.result_hash, first.result_hash);
    let from_cli = spine.finished(cli_run).await;
    let cli_observed = observed(&spine, &from_cli).await;
    assert_same(
        &actual["equity"],
        &cli_observed["equity"],
        0.0,
        "the CLI run",
    );

    let period = [
        ("PERIOD_FROM", config.start.to_string()),
        ("PERIOD_TO", config.end.expect("a backtest end").to_string()),
    ];
    let seen = browser::drive("backtest", &spine, &period).await;
    let from_ui = spine
        .finished(RunId(
            seen["run"].as_i64().expect("the browser names its run"),
        ))
        .await;
    assert_eq!(
        (from_ui.config.start, from_ui.config.end),
        (config.start, config.end)
    );
    let ui_observed = observed(&spine, &from_ui).await;
    for key in ["equity", "benchmark"] {
        assert_same(&cli_observed[key], &ui_observed[key], 0.0, "the UI run");
    }
    spine.logout().await;
}
