use buizz_core::api::{BotSummary, EquitySeries, FeedSlots, OrderView, RunSummary};
use buizz_core::order::OrderStatus;
use buizz_core::run::{RunEventKind, RunId, RunStatus};
use chrono::TimeDelta;
use nix::errno::Errno;
use nix::sys::signal::killpg;
use nix::unistd::Pid;
use serde_json::{Value, json};

use crate::browser;
use crate::fake::FakeProviders;
use crate::spine::{Options, Spine, eventually, now};
use crate::world::{World, minute_floor, stamp};

pub async fn live_run(spine: &Spine, bot: &str) -> RunSummary {
    let bots: Vec<BotSummary> = spine.get("/api/bots").await;
    let id = bots
        .iter()
        .find(|summary| summary.name == bot)
        .and_then(|summary| summary.live_run)
        .unwrap_or_else(|| panic!("{bot} has a live run"));
    spine.run(id).await
}

pub fn bot_pids(events: &[buizz_core::run::RunEvent]) -> Vec<i32> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            RunEventKind::Log { message, .. } => message
                .strip_prefix("bot process ")
                .and_then(|rest| rest.strip_suffix(" started"))
                .and_then(|pid| pid.parse().ok()),
            _ => None,
        })
        .collect()
}

pub fn alive(group: i32) -> bool {
    killpg(Pid::from_raw(group), None) != Err(Errno::ESRCH)
}

pub async fn through_the_close(spine: &Spine, id: RunId) -> RunSummary {
    let close = spine.fake.world(|world| world.live_session().close);
    eventually("the live run to reach the close", || async {
        let run = spine.run(id).await;
        run.clock.is_some_and(|clock| clock >= close).then_some(run)
    })
    .await
}

fn comparable(order: &OrderView, shift: TimeDelta) -> Value {
    let mut value = serde_json::to_value(order).expect("orders encode");
    let fields = value.as_object_mut().expect("an order object");
    fields.remove("client_id");
    fields.remove("session");
    for time in ["decided_at", "updated_at"] {
        let at = stamp(fields[time].as_str().expect("a time")) + shift;
        fields.insert(time.to_owned(), json!(at));
    }
    value
}

pub async fn assert_matches_backtest(spine: &Spine, live: &RunSummary) {
    let (session, previous) = spine.fake.world(|world| {
        let session = world.live_session();
        let previous = world.sessions[world.sessions.len() - 2];
        world.mirror_live_minutes(minute_floor(live.created_at), previous.date);
        (session, previous)
    });
    let queued: RunSummary = spine
        .post(
            &format!("/api/bots/{}/backtests", live.bot),
            json!({"period": {"from": previous.date, "to": previous.date}, "allow_holdout": true}),
        )
        .await;
    let backtest = spine.finished(queued.id).await;
    let shift = session.open - previous.open;
    let live_orders = spine.orders(live.id).await;
    let replayed: Vec<Value> = spine
        .orders(backtest.id)
        .await
        .iter()
        .map(|order| comparable(order, shift))
        .collect();
    assert!(!live_orders.is_empty(), "the live run traded");
    let live_orders: Vec<Value> = live_orders
        .iter()
        .map(|order| comparable(order, TimeDelta::zero()))
        .collect();
    assert_eq!(
        live_orders, replayed,
        "live orders match the backtest on the same bars"
    );
    let curve = |id: RunId| async move {
        let series: EquitySeries = spine.get(&format!("/api/runs/{id}/equity")).await;
        series
            .points
            .into_iter()
            .map(|point| (point.equity, point.cash, point.gross_exposure))
            .collect::<Vec<_>>()
    };
    assert_eq!(curve(live.id).await, curve(backtest.id).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn live_pause_resume_and_stop_in_the_browser_while_an_unpaused_twin_matches_its_backtest() {
    let fake = FakeProviders::serve(World::live(now())).await;
    let spine = Spine::start(
        fake,
        Options {
            bots: &["tape", "tape_twin"],
            tiingo: false,
        },
    )
    .await;
    spine.ingested().await;

    let seen = browser::drive("live", &spine, &[]).await;

    let runs: Vec<RunSummary> = spine.get("/api/runs?bot=tape&mode=live").await;
    let tape = &runs[0];
    assert_eq!(tape.status, RunStatus::Cancelled, "stop ends the live run");
    let feed: FeedSlots = spine.get("/api/data/feed").await;
    assert!(
        feed.holders.iter().all(|holder| holder.run_id != tape.id),
        "the stopped run released its stream slots: {feed:?}"
    );
    let events = spine.events(tape.id).await;
    let statuses: Vec<RunStatus> = events
        .iter()
        .filter_map(|event| match event.kind {
            RunEventKind::Status { status, .. } => Some(status),
            _ => None,
        })
        .collect();
    assert!(statuses.contains(&RunStatus::Paused), "{statuses:?}");
    let pids = bot_pids(&events);
    assert_eq!(
        pids.len(),
        2,
        "pause stops the bot and resume starts a new one"
    );
    assert!(!alive(pids[0]), "the paused bot process is gone");
    assert!(!alive(pids[1]), "the stopped bot process is gone");
    let paused = (
        stamp(seen["paused"]["from"].as_str().expect("paused from")),
        stamp(seen["paused"]["through"].as_str().expect("paused through")),
    );
    for order in spine.orders(tape.id).await {
        assert_ne!(
            order.order.status,
            OrderStatus::Working,
            "stop cancels working orders"
        );
        let decided = order.order.decided_at;
        assert!(
            decided <= paused.0 || decided > paused.1 + TimeDelta::minutes(1),
            "no decisions while paused: {order:?}"
        );
    }

    let twin = live_run(&spine, "tape_twin").await;
    let twin = through_the_close(&spine, twin.id).await;
    assert_matches_backtest(&spine, &twin).await;
}
