use buizz_core::api::RunSummary;
use buizz_core::market::Symbol;
use buizz_core::order::{OrderStatus, OrderType};
use serde_json::{Value, json};

use crate::expect::scenario;
use crate::fake::FakeProviders;
use crate::spine::{Options, Spine};
use crate::world::{World, decimal, stamp};

#[tokio::test(flavor = "multi_thread")]
async fn a_half_hourly_minute_bot_fills_limits_on_touch_expires_day_orders_at_the_half_day_close_and_sells_the_official_close()
 {
    let expected = scenario("lookahead")["minute"].clone();
    let world = World::fixture_market().await;
    let tape: Vec<_> = world
        .minute
        .iter()
        .filter(|bar| bar.symbol == Symbol::new("AAA"))
        .cloned()
        .collect();
    let spine = Spine::start(
        FakeProviders::serve(world).await,
        Options {
            bots: &["intraday"],
            tiingo: false,
            screening: false,
        },
    )
    .await;
    spine.ingested().await;
    let sessions = &expected["sessions"];
    let queued: RunSummary = spine
        .post(
            "/api/bots/intraday/backtests",
            json!({"period": {"from": sessions[0], "to": sessions[1]}}),
        )
        .await;
    let run = spine.finished(queued.id).await;

    let clocks: Vec<Value> = spine
        .logs(run.id)
        .await
        .into_iter()
        .filter(|(_, message)| message.starts_with("step at "))
        .map(|(clock, _)| json!(clock))
        .collect();
    assert_eq!(json!(clocks), expected["every_30m_clocks"]);

    let orders = spine.orders(run.id).await;
    assert_eq!(orders.len(), 3);
    let touched = &orders[0].order;
    let OrderType::Limit { limit_price } = touched.order_type else {
        panic!("the first order is a limit: {touched:?}");
    };
    let touch = tape
        .iter()
        .find(|bar| bar.start >= touched.decided_at && bar.low <= limit_price)
        .expect("the tape touches the bid");
    assert_eq!(touched.status, OrderStatus::Filled);
    assert_eq!(touched.filled_qty, 10);
    assert_eq!(
        touched.updated_at, touch.start,
        "filled on the touching bar"
    );
    assert_eq!(orders[0].avg_fill_price, Some(limit_price.min(touch.open)));

    let expiry = &expected["expiry"];
    let untouched = &orders[1].order;
    assert_eq!(json!(untouched.decided_at), expiry["decided_at"]);
    let OrderType::Limit { limit_price } = untouched.order_type else {
        panic!("the second order is a limit: {untouched:?}");
    };
    let expires_at = stamp(expiry["expires_at"].as_str().expect("an expiry"));
    assert!(
        tape.iter()
            .filter(|bar| bar.start >= untouched.decided_at && bar.start < expires_at)
            .all(|bar| bar.low > limit_price)
    );
    assert_eq!(untouched.status, OrderStatus::Expired);
    assert_eq!(untouched.filled_qty, 0);
    assert_eq!(
        untouched.updated_at, expires_at,
        "expired at the 13:00 close"
    );

    let on_close = &orders[2];
    assert_eq!(on_close.order.order_type, OrderType::MarketOnClose);
    assert_eq!(on_close.order.qty, 10);
    assert_eq!(on_close.order.status, OrderStatus::Filled);
    assert_eq!(on_close.order.updated_at, stamp("2024-12-02T21:00:00Z"));
    assert_eq!(
        on_close.avg_fill_price,
        Some(decimal("99.4480")),
        "the official close of 99.50 less 5 bps and 0.23 bps of impact, not the last minute's {}",
        tape.last().expect("a tape").close
    );
}
