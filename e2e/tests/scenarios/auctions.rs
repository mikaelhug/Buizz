use buizz_core::api::{PositionView, RunSummary};
use buizz_core::order::{OrderStatus, OrderType, RejectReason, Side, TimeInForce};
use buizz_core::run::RunStatus;
use serde_json::json;

use crate::fake::FakeProviders;
use crate::spine::{Options, Spine};
use crate::world::{World, date, decimal, stamp};

#[tokio::test(flavor = "multi_thread")]
async fn a_daily_bot_trades_the_official_close_and_open_on_margin_and_a_stop_fills_on_its_breakout()
{
    let spine = Spine::start(
        FakeProviders::serve(World::fixture_market().await).await,
        Options {
            bots: &["auction", "auction_late"],
            tiingo: false,
            screening: false,
        },
    )
    .await;
    spine.ingested().await;

    let queued: RunSummary = spine
        .post(
            "/api/bots/auction/backtests",
            json!({"period": {"from": "2024-11-29", "to": "2024-12-02"}, "start_cash": "500"}),
        )
        .await;
    let run = spine.finished(queued.id).await;
    assert_eq!(run.status, RunStatus::Completed);

    let steps: Vec<_> = spine
        .logs(run.id)
        .await
        .into_iter()
        .map(|(_, message)| message)
        .filter(|message| message.starts_with("step at "))
        .collect();
    assert_eq!(
        steps,
        [
            "step at 2024-11-29T17:50:00Z: so far 99.50 100.40 99.10 99.87 1523800, last daily close 99.00",
            "step at 2024-12-02T20:50:00Z: so far 100.50 101.20 99.20 99.63 3605060, last daily close 100.00",
        ],
        "ten minutes before each close the bot sees the minutes so far and only the previous daily bar"
    );

    let orders = spine.orders(run.id).await;
    let [on_close, on_open, breakout] = orders.as_slice() else {
        panic!("three orders: {orders:#?}");
    };
    assert_eq!(on_close.order.order_type, OrderType::MarketOnClose);
    assert_eq!(on_close.order.session, Some(date("2024-11-29")));
    assert_eq!(on_close.order.status, OrderStatus::Filled);
    assert_eq!(on_close.order.updated_at, stamp("2024-11-29T18:00:00Z"));
    assert_eq!(
        on_close.avg_fill_price,
        Some(decimal("100.0522")),
        "the official half-day close of 100.00 plus 5 bps and 0.22 bps of impact"
    );

    assert_eq!(on_open.order.order_type, OrderType::MarketOnOpen);
    assert_eq!(on_open.order.side, Side::Sell);
    assert_eq!(on_open.order.status, OrderStatus::Filled);
    assert_eq!(on_open.order.updated_at, stamp("2024-12-02T14:30:00Z"));
    assert_eq!(
        on_open.avg_fill_price,
        Some(decimal("100.4475")),
        "the official open of 100.50 less 5 bps and 0.22 bps of impact"
    );

    assert_eq!(
        breakout.order.order_type,
        OrderType::Stop {
            stop_price: decimal("49.5000")
        }
    );
    assert_eq!(breakout.order.tif, TimeInForce::Gtc);
    assert_eq!(breakout.order.status, OrderStatus::Filled);
    assert_eq!(
        breakout.avg_fill_price,
        Some(decimal("49.5260")),
        "BBB opened at 49.00 and touched 49.60, so the stop fills at 49.50 plus 5 bps and 0.25 bps of impact"
    );

    let positions: Vec<PositionView> = spine.get(&format!("/api/runs/{}/positions", run.id)).await;
    assert_eq!(positions.len(), 1, "flat in AAA: {positions:#?}");
    assert_eq!(positions[0].symbol.as_str(), "BBB");
    assert_eq!(positions[0].qty, 10);
    assert_eq!(
        run.equity,
        Some(decimal("499.58")),
        "500 - 1001.52 for the close - 0.12 to finance the 501.52 overdraft overnight at 4.5% + 1.5% / 252 \
         + 1003.48 for the open - 496.26 for the stop + 494.00 of BBB"
    );

    let queued: RunSummary = spine
        .post(
            "/api/bots/auction_late/backtests",
            json!({"period": {"from": "2024-11-29", "to": "2024-11-29"}}),
        )
        .await;
    let late = spine.finished(queued.id).await;
    let orders = spine.orders(late.id).await;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].order.status, OrderStatus::Rejected);
    assert_eq!(
        orders[0].order.reject_reason,
        Some(RejectReason::AuctionClosed),
        "12:55 on the half day is after the 12:50 cutoff"
    );
}
