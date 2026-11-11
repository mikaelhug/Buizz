use buizz_core::api::{ApiError, FeedSlots, RunSummary};
use buizz_core::order::{OrderStatus, RejectReason};
use buizz_core::run::{LogLevel, RiskLimit, RunEventKind};
use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::fake::FakeProviders;
use crate::spine::{Options, Spine};
use crate::world::{World, decimal, stamp};

#[tokio::test(flavor = "multi_thread")]
async fn a_full_stream_refuses_a_run_an_oversized_order_is_rejected_and_a_drawdown_pauses_the_bot()
{
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
    let run = spine.finished(queued.id).await;
    let breached_at = stamp("2024-11-22T21:00:00Z");
    let message = "drawdown 0.59% breached the 0.50% limit; the bot is paused";
    assert_eq!(run.message.as_deref(), Some(message));

    let orders = spine.orders(run.id).await;
    let oversized = &orders[0].order;
    assert_eq!(oversized.symbol.as_str(), "AAA");
    assert_eq!(oversized.status, OrderStatus::Rejected);
    assert_eq!(
        oversized.reject_reason,
        Some(RejectReason::MaxPositionWeight)
    );
    assert_eq!(orders[1].order.status, OrderStatus::Filled);
    assert!(
        orders
            .iter()
            .all(|order| order.order.decided_at < breached_at),
        "no decisions after the breach: {orders:#?}"
    );

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
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        RunEventKind::Log { level: LogLevel::Warn, message: text } if text == message
    )));
    let steps: Vec<_> = spine
        .logs(run.id)
        .await
        .into_iter()
        .filter(|(_, text)| text.starts_with("step at "))
        .collect();
    assert_eq!(steps.len(), 2, "the bot stops after the breach: {steps:#?}");
}
