use std::collections::BTreeSet;

use buizz_core::api::{ApiError, FeedSlots, PositionView, RunSummary};
use buizz_core::market::{Bar, Symbol};
use buizz_core::order::{OrderStatus, RejectReason, Side};
use buizz_core::run::{RiskLimit, RunBadge, RunEventKind, RunId, RunStatus};
use chrono::TimeDelta;
use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::fake::FakeProviders;
use crate::spine::{Options, Spine, eventually, now};
use crate::world::{World, decimal, stamp};

const SETUP_ALLOWANCE: TimeDelta = TimeDelta::minutes(10);
const LIVE_AHEAD_OF_DECISION: TimeDelta = TimeDelta::minutes(1);

#[tokio::test(flavor = "multi_thread")]
async fn a_full_stream_refuses_a_run_an_oversized_order_is_rejected_and_a_drawdown_stops_the_run() {
    let spine = Spine::start(
        FakeProviders::serve(World::fixture_market().await).await,
        Options {
            bots: &["tape", "wide", "limits"],
            tiingo: false,
        },
    )
    .await;
    spine.ingested().await;

    let holder: RunSummary = spine.post("/api/bots/tape/live", json!({})).await;
    let (status, body) = spine
        .request(Method::POST, "/api/bots/wide/live", Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let refusal: ApiError = serde_json::from_str(&body).expect("an API error");
    assert_eq!(
        refusal.error,
        format!(
            "stream symbol cap exceeded: 29 requested, 28 free; held by tape (run {}, 2 symbols)",
            holder.id
        )
    );
    let slots: FeedSlots = spine.get("/api/data/feed").await;
    assert_eq!((slots.used, slots.cap), (2, 30));
    assert_eq!(
        slots.connections.len(),
        1,
        "the ingest key is the one stream account"
    );
    assert_eq!(slots.holders.len(), 1);
    assert_eq!(slots.holders[0].run_id, holder.id);

    let queued: RunSummary = spine
        .post(
            "/api/bots/limits/backtests",
            json!({
                "period": {"from": "2024-11-20", "to": "2024-12-04"},
                "risk": {"max_position_weight": "0.6", "max_gross_exposure": "1.0", "max_drawdown": "0.005"},
            }),
        )
        .await;
    let run = spine.ended(queued.id).await;
    let breached_at = stamp("2024-11-22T21:00:00Z");
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.message.as_deref(), Some("stopped by drawdown limit"));
    assert!(run.badges.contains(&RunBadge::DrawdownStop));
    assert_eq!(run.clock, Some(stamp("2024-11-25T21:00:00Z")));
    assert!(run.metrics.is_some(), "the stopped run is measured");

    let orders = spine.orders(run.id).await;
    let oversized = &orders[0].order;
    assert_eq!(oversized.symbol.as_str(), "AAA");
    assert_eq!(oversized.status, OrderStatus::Rejected);
    assert_eq!(
        oversized.reject_reason,
        Some(RejectReason::MaxPositionWeight)
    );
    assert_eq!(orders[1].order.status, OrderStatus::Filled);
    let (decided, closing) = orders.split_at(orders.len() - 1);
    assert!(
        decided
            .iter()
            .all(|order| order.order.decided_at < breached_at),
        "no bot decisions after the breach: {orders:#?}"
    );
    let close = &closing[0].order;
    assert_eq!(close.symbol.as_str(), "BBB");
    assert_eq!(close.side, Side::Sell);
    assert_eq!(close.status, OrderStatus::Filled);
    assert_eq!(close.reason, "stopped by drawdown limit");
    assert_eq!(close.decided_at, breached_at);
    assert_eq!(close.updated_at, stamp("2024-11-25T14:30:00Z"));
    let positions: Vec<PositionView> = spine.get(&format!("/api/runs/{}/positions", run.id)).await;
    assert!(positions.is_empty(), "flat after the stop: {positions:#?}");

    let events = spine.events(run.id).await;
    let breach = events
        .iter()
        .find(|event| {
            matches!(
                event.kind,
                RunEventKind::RiskBreach {
                    limit: RiskLimit::MaxDrawdown,
                    ..
                }
            )
        })
        .expect("the drawdown breach is an event");
    assert_eq!(breach.clock, Some(breached_at));
    let RunEventKind::RiskBreach { threshold, .. } = breach.kind else {
        unreachable!("matched above");
    };
    assert_eq!(threshold, decimal("0.005"));
    let steps: Vec<_> = spine
        .logs(run.id)
        .await
        .into_iter()
        .filter(|(_, text)| text.starts_with("step at "))
        .collect();
    assert_eq!(steps.len(), 2, "the bot stops after the breach: {steps:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_stream_account_admits_a_31st_symbol_and_a_before_close_bot_decides_on_rest_minutes_without_a_slot()
 {
    let fake = FakeProviders::serve_stream_accounts(World::live_closing(now()), 2).await;
    let mut spine = Spine::start_behind(
        fake,
        Options {
            bots: &["tape", "wide", "auction"],
            tiingo: false,
        },
        SETUP_ALLOWANCE,
    )
    .await;
    spine.ingested().await;
    let decided_at = spine
        .fake
        .world(|world| world.live_session().close - TimeDelta::minutes(10));
    spine.stop().await;
    spine
        .restart_later(decided_at - LIVE_AHEAD_OF_DECISION - spine.now())
        .await;

    let tape: RunSummary = spine.post("/api/bots/tape/live", json!({})).await;
    let wide: RunSummary = spine.post("/api/bots/wide/live", json!({})).await;
    let auction: RunSummary = spine.post("/api/bots/auction/live", json!({})).await;
    let slots: FeedSlots = spine.get("/api/data/feed").await;
    assert_eq!((slots.used, slots.cap), (31, 60));
    let holders: Vec<RunId> = slots.holders.iter().map(|holder| holder.run_id).collect();
    assert_eq!(
        holders,
        [tape.id, wide.id],
        "the before-close bot holds no slot"
    );
    let used: Vec<u32> = slots
        .connections
        .iter()
        .map(|connection| connection.used)
        .collect();
    assert_eq!(
        used,
        [16, 15],
        "each new symbol goes where there is most room"
    );

    let streamed = eventually("both accounts to stream their symbols", || async {
        let subscriptions = spine.fake.stream_subscriptions();
        let symbols: BTreeSet<&Symbol> = subscriptions.values().flatten().collect();
        (symbols.len() == 31).then_some(subscriptions)
    })
    .await;
    let per_account: Vec<(&str, usize)> = streamed
        .iter()
        .map(|(account, symbols)| (account.as_str(), symbols.len()))
        .collect();
    assert_eq!(per_account, [("e2e-stream-1", 16), ("e2e-stream-2", 15)]);
    eventually("both connections up without errors", || async {
        let slots: FeedSlots = spine.get("/api/data/feed").await;
        slots
            .connections
            .iter()
            .all(|connection| connection.connected && connection.error.is_none())
            .then_some(())
    })
    .await;

    let open = spine.fake.world(|world| world.live_session().open);
    let so_far: Vec<Bar> = spine.fake.world(|world| {
        world
            .minute
            .iter()
            .filter(|bar| {
                bar.symbol == Symbol::new("AAA") && (open..decided_at).contains(&bar.start)
            })
            .cloned()
            .collect()
    });
    let (clock, step) = eventually("the before-close step", || async {
        spine
            .logs(auction.id)
            .await
            .into_iter()
            .find(|(_, message)| message.starts_with("step at "))
    })
    .await;
    assert_eq!(clock, Some(decided_at));
    let high = so_far
        .iter()
        .map(|bar| bar.high)
        .max()
        .expect("minutes so far");
    let low = so_far
        .iter()
        .map(|bar| bar.low)
        .min()
        .expect("minutes so far");
    let volume: u64 = so_far.iter().map(|bar| bar.volume).sum();
    let expected = format!(
        "so far {:.2} {high:.2} {low:.2} {:.2} {volume},",
        so_far[0].open,
        so_far[so_far.len() - 1].close
    );
    assert!(
        step.contains(&expected),
        "the bot sees every minute of the session so far: {step} vs {expected}"
    );
    let slots: FeedSlots = spine.get("/api/data/feed").await;
    assert!(
        slots
            .holders
            .iter()
            .all(|holder| holder.run_id != auction.id),
        "deciding took no stream slot: {slots:?}"
    );
}
