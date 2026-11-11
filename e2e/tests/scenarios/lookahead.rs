use std::collections::BTreeMap;

use buizz_core::api::{OrderView, RunSummary};
use buizz_core::market::Session;
use buizz_core::order::OrderStatus;
use buizz_core::time::Timestamp;
use chrono::TimeDelta;
use serde_json::{Value, json};

use crate::expect::{assert_same, scenario};
use crate::fake::FakeProviders;
use crate::spine::{Options, Spine};
use crate::world::{World, date, stamp};

struct Step {
    clock: Timestamp,
    report: Value,
}

impl Step {
    fn history(&self, timeframe: &str, bars: u64, adjust: &str, symbol: &str) -> &Value {
        self.report["history"]
            .as_array()
            .expect("history views")
            .iter()
            .find(|view| {
                view["tf"] == timeframe
                    && view["bars"] == bars
                    && view["adjust"] == adjust
                    && view["symbol"] == symbol
            })
            .unwrap_or_else(|| panic!("{timeframe} {bars} {adjust} {symbol} at {}", self.clock))
    }

    fn starts(view: &Value) -> Vec<Timestamp> {
        view["starts"]
            .as_array()
            .expect("bar starts")
            .iter()
            .map(|start| Timestamp::from_nanos(start.as_i64().expect("nanoseconds")))
            .collect()
    }

    fn period_ends(&self, symbol: &Value, metric: &Value) -> Vec<Value> {
        let mut ends: Vec<Value> = self.report["fundamentals"]
            .as_array()
            .expect("fundamentals")
            .iter()
            .filter(|point| point["symbol"] == *symbol && point["metric"] == *metric)
            .map(|point| point["period_end"].clone())
            .collect();
        ends.sort_by_key(ToString::to_string);
        ends
    }
}

async fn steps(spine: &Spine, run: &RunSummary) -> Vec<Step> {
    spine
        .logs(run.id)
        .await
        .into_iter()
        .filter_map(|(_, message)| serde_json::from_str::<Value>(&message).ok())
        .map(|report| Step {
            clock: stamp(report["clock"].as_str().expect("a clock")),
            report,
        })
        .collect()
}

fn at<'a>(steps: &'a [Step], clock: &Value) -> &'a Step {
    let clock = stamp(clock.as_str().expect("a clock"));
    steps
        .iter()
        .find(|step| step.clock == clock)
        .unwrap_or_else(|| panic!("a step at {clock}"))
}

fn order_decided_at<'a>(orders: &'a [OrderView], decided: &Value) -> &'a OrderView {
    let decided = stamp(decided.as_str().expect("a decision time"));
    orders
        .iter()
        .find(|order| order.order.decided_at == decided)
        .unwrap_or_else(|| panic!("an order decided at {decided}"))
}

fn assert_filled_at(orders: &[OrderView], cases: &Value) {
    for case in cases.as_array().expect("fill cases") {
        let order = order_decided_at(orders, &case["decided_at"]);
        assert_eq!(order.order.status, OrderStatus::Filled, "{case}");
        assert_eq!(json!(order.order.updated_at), case["fills_at"], "{case}");
    }
}

fn session_of(sessions: &[Session], start: Timestamp) -> Session {
    *sessions
        .iter()
        .find(|session| session.date == start.exchange_date())
        .unwrap_or_else(|| panic!("a session for the bar at {start}"))
}

async fn backtest(spine: &Spine, bot: &str, from: &Value, to: &Value) -> RunSummary {
    spine
        .post(
            &format!("/api/bots/{bot}/backtests"),
            json!({"period": {"from": from, "to": to}}),
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_probe_asking_for_everything_never_sees_past_the_clock() {
    let expected = scenario("lookahead");
    let tolerance = expected["tolerance"].as_f64().expect("a tolerance");
    let world = World::fixture_market().await;
    let sessions = world.sessions.clone();
    let spine = Spine::start(
        FakeProviders::serve(world).await,
        Options {
            bots: &["probe", "probe_bar", "probe_30m", "probe_1h", "probe_1555"],
            tiingo: false,
        },
    )
    .await;
    spine.ingested().await;
    let daily = &expected["daily"];
    let minute = &expected["minute"];
    let (minute_from, minute_to) = (&minute["sessions"][0], &minute["sessions"][1]);
    let runs: BTreeMap<&str, RunSummary> = [
        (
            "probe",
            backtest(&spine, "probe", &daily["start"], &daily["end"]).await,
        ),
        (
            "bar",
            backtest(&spine, "probe_bar", minute_from, minute_to).await,
        ),
        (
            "every:30m",
            backtest(&spine, "probe_30m", minute_from, minute_to).await,
        ),
        (
            "every:1h",
            backtest(&spine, "probe_1h", minute_from, minute_to).await,
        ),
        (
            "at:15:55",
            backtest(&spine, "probe_1555", minute_from, minute_to).await,
        ),
    ]
    .into_iter()
    .collect();
    let mut seen: BTreeMap<&str, Vec<Step>> = BTreeMap::new();
    for (name, run) in &runs {
        spine.finished(run.id).await;
        seen.insert(name, steps(&spine, run).await);
    }

    let probe = &seen["probe"];
    let clocks: Vec<Value> = probe.iter().map(|step| json!(step.clock)).collect();
    assert_eq!(json!(clocks), daily["step_clocks"]);
    for step in probe {
        for view in step.report["history"].as_array().expect("history views") {
            for start in Step::starts(view) {
                let session = session_of(&sessions, start);
                assert!(
                    session.close <= step.clock,
                    "daily bar {start} seen at {}",
                    step.clock
                );
            }
        }
    }
    for case in daily["history"].as_array().expect("history cases") {
        let view = at(probe, &case["clock"]).history(
            "1d",
            case["bars"].as_u64().expect("bars"),
            case["adjust"].as_str().expect("adjust"),
            case["symbol"].as_str().expect("symbol"),
        );
        assert_same(
            &case["closes"],
            &view["closes"],
            tolerance,
            &case.to_string(),
        );
        if let Some(volumes) = case.get("volumes") {
            assert_same(volumes, &view["volumes"], tolerance, &case.to_string());
        }
    }
    for case in daily["fundamentals"].as_array().expect("fundamental cases") {
        let step = at(probe, &case["clock"]);
        assert_eq!(
            json!(step.period_ends(&case["symbol"], &case["metric"])),
            case["period_ends"],
            "{case}"
        );
    }
    assert_filled_at(&spine.orders(runs["probe"].id).await, &daily["fills"]);

    for (wake, count) in minute["step_counts"].as_object().expect("step counts") {
        assert_eq!(
            json!(seen[wake.as_str()].len()),
            *count,
            "steps with wake {wake}"
        );
    }
    let every_30m: Vec<Value> = seen["every:30m"]
        .iter()
        .map(|step| json!(step.clock))
        .collect();
    assert_eq!(json!(every_30m), minute["every_30m_clocks"]);

    let bar = &seen["bar"];
    let first = &minute["first_step"];
    assert_eq!(json!(bar[0].clock), first["clock"]);
    for symbol in ["AAA", "BBB"] {
        let starts = Step::starts(bar[0].history("1m", 10, "raw", symbol));
        assert_eq!(
            json!(starts),
            json!([first["bar_start"]]),
            "{symbol} at the first step"
        );
    }
    for (day, last) in minute["last_step"].as_object().expect("last steps") {
        let clock = bar
            .iter()
            .filter(|step| step.clock.exchange_date() == date(day))
            .map(|step| step.clock)
            .max();
        assert_eq!(json!(clock), *last, "last step on {day}");
    }
    for step in bar {
        for symbol in ["AAA", "BBB"] {
            for start in Step::starts(step.history("1m", 30, "all", symbol)) {
                assert!(
                    start + TimeDelta::minutes(1) <= step.clock,
                    "minute bar {start} seen at {}",
                    step.clock
                );
            }
        }
    }
    for clock in minute["every_30m_clocks"].as_array().expect("clocks") {
        let step = at(bar, clock);
        let mut starts = Vec::new();
        for symbol in ["AAA", "BBB"] {
            starts.extend(Step::starts(step.history("1m", 30, "all", symbol)));
        }
        assert_eq!(starts.len(), 60, "minute history at {clock}");
        assert_eq!(
            starts.iter().max().copied(),
            Some(step.clock - TimeDelta::minutes(1)),
            "latest minute bar at {clock}"
        );
    }
    for case in minute["daily_history"]
        .as_array()
        .expect("daily history cases")
    {
        let starts = Step::starts(at(bar, &case["clock"]).history("1d", 1, "raw", "AAA"));
        let last = starts.last().expect("a daily bar").exchange_date();
        assert_eq!(json!(last), case["last_bar_date"], "{case}");
    }
    for case in minute["fundamentals"]
        .as_array()
        .expect("fundamental cases")
    {
        let step = at(bar, &case["clock"]);
        assert_eq!(
            json!(step.period_ends(&case["symbol"], &case["metric"])),
            case["period_ends"],
            "{case}"
        );
    }
    let orders = spine.orders(runs["bar"].id).await;
    assert_filled_at(&orders, &minute["fills"]);
    for case in minute["order_sessions"].as_array().expect("order sessions") {
        let order = order_decided_at(&orders, &case["decided_at"]);
        assert_eq!(json!(order.order.session), case["session"], "{case}");
    }
}
