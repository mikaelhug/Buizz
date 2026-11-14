use buizz_core::api::{BotProcesses, BotSummary, EquitySeries, FeedSlots, OrderView, RunSummary};
use buizz_core::order::{OrderStatus, TimeInForce};
use buizz_core::run::{Mode, RunEventKind, RunId, RunStatus};
use chrono::TimeDelta;
use nix::errno::Errno;
use nix::sys::signal::killpg;
use nix::unistd::Pid;
use reqwest::Method;
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
        .and_then(|summary| summary.live.first())
        .map(|live| live.run.id)
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
            screening: false,
        },
    )
    .await;
    let options = spine
        .ingested()
        .await
        .into_iter()
        .find(|job| job.name == "option_chains")
        .and_then(|job| job.last_message);
    assert_eq!(
        options.as_deref(),
        Some("option chains: 2 rows"),
        "the session closes within half an hour, so SPY's chain is recorded"
    );

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

#[tokio::test(flavor = "multi_thread")]
async fn a_daily_live_bot_is_one_process_per_session_and_keeps_its_state_and_resting_order() {
    let fake = FakeProviders::serve(World::overnight(now())).await;
    let mut spine = Spine::start(
        fake.clone(),
        Options {
            bots: &["overnight"],
            tiingo: false,
            screening: false,
        },
    )
    .await;
    spine.ingested().await;
    let run: RunSummary = spine.post("/api/bots/overnight/live", json!({})).await;
    eventually("the live run to wait for the close", || async {
        (spine.run(run.id).await.status == RunStatus::Running).then_some(())
    })
    .await;
    assert!(
        bot_pids(&spine.events(run.id).await).is_empty(),
        "no bot process before the session's first wake"
    );

    let tomorrow = fake.world(|world| world.live_session());
    spine.stop().await;
    spine
        .restart_later(tomorrow.close + TimeDelta::minutes(45) - now())
        .await;
    let run = eventually("both closes to be stepped", || async {
        let run = spine.run(run.id).await;
        (run.usage.processes == 2).then_some(run)
    })
    .await;

    assert_eq!(run.clock, Some(tomorrow.close));
    assert_eq!(run.status, RunStatus::Running);
    let steps: Vec<String> = spine
        .logs(run.id)
        .await
        .into_iter()
        .map(|(_, message)| message)
        .filter(|message| message.starts_with("step "))
        .collect();
    assert_eq!(
        steps,
        [
            "step 1, 1 in this process, working none",
            "step 2, 1 in this process, working rest",
        ],
        "a new process each session, with the saved state and the resting order"
    );
    let events = spine.events(run.id).await;
    let pids = bot_pids(&events);
    assert_eq!(pids.len(), 2, "one bot process per session");
    assert!(pids.iter().all(|pid| !alive(*pid)), "both processes exited");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, RunEventKind::BotExited { .. })),
        "the sessions ended without a crash: {events:#?}"
    );
    let (_, output) = spine
        .request(Method::GET, &format!("/api/runs/{}/logs", run.id), None)
        .await;
    assert!(
        output.contains("ended with session_end after step 1")
            && output.contains("ended with session_end after step 2"),
        "{output}"
    );

    let orders = spine.orders(run.id).await;
    let [rest] = orders.as_slice() else {
        panic!("one order: {orders:#?}");
    };
    assert_eq!(rest.order.client_id, "rest");
    assert_eq!(rest.order.tif, TimeInForce::Gtc);
    assert_eq!(rest.order.status, OrderStatus::Working);

    assert!(run.usage.peak_memory > 0, "{:?}", run.usage);
    let processes: BotProcesses = spine.get("/api/data/processes").await;
    let footprint = processes
        .footprints
        .iter()
        .find(|footprint| footprint.bot == "overnight" && footprint.mode == Mode::Live)
        .expect("the live bot's footprint");
    assert_eq!(footprint.usage, run.usage);
}
