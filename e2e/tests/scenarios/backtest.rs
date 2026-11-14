use buizz_core::api::{EquitySeries, PositionView, PriceChart, RunSummary, SpineEvent, TradeView};
use buizz_core::performance::Behaviour;
use buizz_core::run::{CostModel, RiskLimits, RunConfig, RunId};
use reqwest::StatusCode;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::browser;
use crate::expect::{assert_same, scenario};
use crate::fake::FakeProviders;
use crate::spine::{Options, Spine, USERNAME};
use crate::world::{World, decimal, repository};

async fn cli_backtest(spine: &Spine, config: &RunConfig) -> (RunId, Vec<String>) {
    let output = Command::new("uv")
        .args(["run", "--frozen", "--quiet", "--project"])
        .arg(repository().join("sdk/python"))
        .args(["buizz", "backtest", "bots/red_green", "--from"])
        .arg(config.start.to_string())
        .arg("--to")
        .arg(config.end.expect("a backtest end").to_string())
        .args([
            "--risk",
            "max_drawdown=0.9",
            "--cost",
            "slippage_per_share=0",
        ])
        .env("BUIZZ_URL", &spine.base)
        .env("BUIZZ_ADMIN_USERNAME", USERNAME)
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

fn printed_metric(value: &Value) -> String {
    match value.as_f64() {
        Some(number) if value.is_f64() => format!("{number:.4}"),
        _ if value.is_null() => "-".to_owned(),
        _ => value.to_string(),
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

fn percent_change(change: f64) -> String {
    if format!("{:.2}", change * 100.0) == "0.00" {
        "0.00%".to_owned()
    } else {
        format!("{:+.2}%", change * 100.0)
    }
}

async fn assert_run_view_analysis(spine: &Spine, run: &RunSummary, seen: &Value) {
    let series: EquitySeries = spine.get(&format!("/api/runs/{}/equity", run.id)).await;
    let start = run.config.start_cash.to_f64().expect("a start cash");
    let mut month_ends: Vec<(String, f64)> = Vec::new();
    for point in &series.points {
        let month = point.at.exchange_date().format("%b").to_string();
        let equity = point.equity.to_f64().expect("an equity");
        match month_ends.last_mut() {
            Some((last, end)) if *last == month => *end = equity,
            _ => month_ends.push((month, equity)),
        }
    }
    let mut base = start;
    for (name, end) in month_ends {
        assert_eq!(
            seen["months"][&name],
            json!(percent_change(end / base - 1.0)),
            "{name} in {seen}"
        );
        base = end;
    }
    let benchmark_return = series
        .benchmark_metrics
        .total_return
        .expect("a benchmark return");
    assert_eq!(
        seen["benchmarkReturn"],
        json!(format!(
            "{} {}",
            run.config.benchmark,
            percent_change(benchmark_return)
        )),
        "the benchmark beside the run's return"
    );
    let behaviour: Behaviour = spine.get(&format!("/api/runs/{}/behaviour", run.id)).await;
    let held = behaviour.run.expect("a fingerprint of the finished run");
    assert_eq!(
        (
            held.positions_median,
            held.positions_min,
            held.positions_max
        ),
        (Some(1.0), Some(0), Some(2)),
        "no stock, then BBB, both, AAA alone and none again: {held:#?}"
    );
    assert_eq!(held.holdings, 4, "BBB twice and AAA twice: {held:#?}");
    assert_eq!(
        (held.sells_on_price, held.idle_months),
        (Some(0.0), Some(0.0)),
        "green-day exits carry no price: reason, and both months trade: {held:#?}"
    );
    assert!(
        behaviour.references.is_empty(),
        "one Berkshire quarter end before the period gives no reference"
    );
    assert_eq!(seen["positions"], json!("1 (0–2)"), "the behaviour panel");
    let trades: Vec<TradeView> = spine.get(&format!("/api/runs/{}/trades", run.id)).await;
    let metrics = run.metrics.as_ref().expect("metrics");
    assert_eq!(
        trades.len(),
        metrics.trades as usize,
        "a trade per round trip"
    );
    assert_eq!(
        seen["trades"],
        json!(trades.len()),
        "the trades the page lists"
    );
    let chart = seen["chart"].as_str().unwrap_or_default();
    for told in [
        "4 fills, 2 buys and 2 sells",
        "adjusted for the 2-for-1 split on 2024-11-26",
    ] {
        assert!(
            chart.contains(told),
            "{told:?} in the AAA chart summary {chart:?}"
        );
    }
    assert!(
        trades
            .iter()
            .all(|trade| trade.opening_reason.is_some() && trade.closing_reason.is_some()),
        "every red_green trade opens and closes on a reasoned order: {trades:#?}"
    );
}

async fn assert_split_adjusted_chart(spine: &Spine, run: &RunSummary) {
    let chart: PriceChart = spine.get(&format!("/api/runs/{}/chart/AAA", run.id)).await;
    assert_eq!(chart.bars.len(), 10, "a daily bar per session: {chart:#?}");
    assert_eq!(
        chart.bars.first().map(|bar| bar.open),
        Some(100.0),
        "bars before the 2:1 split are halved"
    );
    assert_eq!(
        chart
            .splits
            .iter()
            .map(|split| (split.ex_date.to_string(), split.ratio))
            .collect::<Vec<_>>(),
        [("2024-11-26".to_owned(), decimal("2"))]
    );
    let fills: Vec<_> = chart
        .fills
        .iter()
        .map(|fill| (fill.qty, fill.price, fill.chart_price))
        .collect();
    assert_eq!(
        fills,
        [
            (125, decimal("198.0990"), decimal("99.0495")),
            (-250, decimal("99.4395"), decimal("99.4395")),
            (251, decimal("99.0597"), decimal("99.0597")),
            (-251, decimal("101.4384"), decimal("101.4384")),
        ],
        "fills sit on the split-adjusted candles"
    );
    assert!(chart.fills.iter().all(|fill| !fill.reason.is_empty()));
    assert_eq!(chart.position, None, "AAA is flat at the end");
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
            screening: false,
        },
    )
    .await;
    spine.ingested().await;
    assert_eq!(spine.anonymous("/api/bots").await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        spine.login(USERNAME, "wrong").await.status(),
        StatusCode::UNAUTHORIZED
    );

    let (cli_run, printed) = cli_backtest(&spine, &config).await;
    for (name, value) in expected["metrics"].as_object().expect("metrics") {
        let line = [name.as_str(), &printed_metric(value)];
        assert!(
            printed
                .iter()
                .any(|printed| printed.split_whitespace().eq(line)),
            "{line:?} in {printed:#?}"
        );
    }
    for line in [
        "orders: 7 (7 filled)",
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
    let mut equity_events = Vec::new();
    loop {
        match events.next().await {
            SpineEvent::Equity {
                run_id, at, equity, ..
            } if run_id == queued.id => equity_events.push((at, equity)),
            SpineEvent::RunStatus { run_id, status, .. }
                if run_id == queued.id && status.is_finished() =>
            {
                break;
            }
            _ => {}
        }
    }
    let first = spine.finished(queued.id).await;
    assert_eq!(first.config, config);
    assert!(
        (1..=10).contains(&equity_events.len()),
        "backtest equity events are coalesced: {equity_events:#?}"
    );
    assert_eq!(
        equity_events.last().copied(),
        first.clock.zip(first.equity),
        "the last equity event is the final equity"
    );

    let actual = observed(&spine, &first).await;
    let tolerance = expected["metrics_tolerance"].as_f64().expect("a tolerance");
    for key in ["steps", "orders", "equity", "benchmark", "final", "metrics"] {
        assert_same(&expected[key], &actual[key], tolerance, key);
    }
    assert_ledger_fills(&expected, &actual["orders"]);
    assert_split_adjusted_chart(&spine, &first).await;

    let rerun: RunSummary = spine.post("/api/bots/red_green/backtests", request).await;
    let second = spine.finished(rerun.id).await;
    assert!(first.result_hash.is_some());
    assert_eq!(second.result_hash, first.result_hash);
    let from_cli = spine.finished(cli_run).await;
    let overridden = RiskLimits {
        max_position_weight: Some(decimal("0.3")),
        max_gross_exposure: Some(decimal("1.0")),
        max_drawdown: Some(decimal("0.9")),
    };
    assert_eq!(
        from_cli.config.risk, overridden,
        "the CLI adds a drawdown stop and keeps the bot's other limits"
    );
    assert_eq!(
        from_cli.config.cost,
        CostModel::default(),
        "a cost field the CLI sets leaves the others at their defaults"
    );
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
    assert_eq!(from_ui.config.risk, overridden, "the dialog does the same");
    let ui_observed = observed(&spine, &from_ui).await;
    for key in ["equity", "benchmark"] {
        assert_same(&cli_observed[key], &ui_observed[key], 0.0, "the UI run");
    }
    assert_run_view_analysis(&spine, &from_ui, &seen).await;
    spine.logout().await;
}
