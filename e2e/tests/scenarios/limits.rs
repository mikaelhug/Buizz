use buizz_core::api::{ApiError, FeedSlots, PositionView, RunSummary};
use buizz_core::order::{OrderStatus, RejectReason, Side};
use buizz_core::run::{RiskLimit, RunBadge, RunEventKind, RunStatus};

use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::fake::FakeProviders;
use crate::spine::{Options, Spine};
use crate::world::{World, decimal, stamp};

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
        .request(Method::POST, "/api/bots/wide/live", None)
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
