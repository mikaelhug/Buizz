use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use buizz_core::ledger::{LedgerEntry, Portfolio};
use buizz_core::market::{
    Bar, Calendar, CorporateAction, CorporateActionKind, Feed, Session, Symbol, Timeframe,
};
use buizz_core::order::{Fill, Order, OrderId, OrderRequest, OrderSize, OrderType, TimeInForce};
use buizz_core::run::{EquityPoint, RunConfig};
use buizz_core::visibility::{bar_visible_at, order_session};
use buizz_engine::{Cycle, Engine};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde_json::{Value, json};

struct Market {
    calendar: Calendar,
    bars: Vec<Bar>,
    actions: Vec<CorporateAction>,
}

#[derive(Default)]
struct Outcome {
    steps: usize,
    ledger: Vec<LedgerEntry>,
    orders: BTreeMap<OrderId, Order>,
    fills: Vec<Fill>,
    equity: Vec<EquityPoint>,
    portfolio: Portfolio,
    hash: String,
}

fn fixture(path: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path),
    )
    .expect("fixture is readable")
}

fn rows(path: &str) -> Vec<BTreeMap<String, String>> {
    let text = fixture(path);
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().expect("csv header").split(',').collect();
    lines
        .map(|line| {
            header
                .iter()
                .zip(line.split(','))
                .map(|(key, value)| ((*key).to_owned(), value.to_owned()))
                .collect()
        })
        .collect()
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).expect("decimal")
}

fn date(text: &str) -> NaiveDate {
    text.parse().expect("date")
}

fn load_market(universe: &[Symbol]) -> Market {
    let calendar = Calendar::new(
        rows("fixtures/market/calendar.csv")
            .iter()
            .map(|row| Session {
                date: date(&row["date"]),
                open: row["open"].parse().expect("open"),
                close: row["close"].parse().expect("close"),
            })
            .collect(),
    );
    let bars = rows("fixtures/market/bar_1d.csv")
        .iter()
        .map(|row| Bar {
            symbol: Symbol::new(&row["symbol"]),
            start: calendar.session(date(&row["date"])).expect("session").open,
            open: decimal(&row["open"]),
            high: decimal(&row["high"]),
            low: decimal(&row["low"]),
            close: decimal(&row["close"]),
            volume: row["volume"].parse().expect("volume"),
            feed: Feed::Sip,
        })
        .filter(|bar| universe.contains(&bar.symbol))
        .collect();
    let actions = rows("fixtures/market/corporate_actions.csv")
        .iter()
        .map(|row| CorporateAction {
            symbol: Symbol::new(&row["symbol"]),
            ex_date: date(&row["ex_date"]),
            kind: match row["kind"].as_str() {
                "split" => CorporateActionKind::Split {
                    old_rate: decimal(&row["old_rate"]),
                    new_rate: decimal(&row["new_rate"]),
                },
                _ => CorporateActionKind::CashDividend {
                    rate: decimal(&row["rate"]),
                },
            },
        })
        .collect();
    Market {
        calendar,
        bars,
        actions,
    }
}

fn average_daily_volume(
    market: &Market,
    session: &Session,
    sessions: u32,
) -> BTreeMap<Symbol, u64> {
    let window: Vec<_> = market
        .calendar
        .sessions()
        .iter()
        .filter(|earlier| earlier.date < session.date)
        .rev()
        .take(sessions as usize)
        .map(|earlier| earlier.open)
        .collect();
    let mut totals: BTreeMap<Symbol, (u64, u64)> = BTreeMap::new();
    for bar in market.bars.iter().filter(|bar| window.contains(&bar.start)) {
        let total = totals.entry(bar.symbol.clone()).or_default();
        *total = (total.0 + bar.volume, total.1 + 1);
    }
    totals
        .into_iter()
        .map(|(symbol, (volume, count))| (symbol, volume / count))
        .collect()
}

fn red_green(
    engine: &Engine,
    bars: &[Bar],
    weight: Decimal,
    session: NaiveDate,
) -> Vec<OrderRequest> {
    bars.iter()
        .filter_map(|bar| {
            let (weight, reason) =
                if bar.close < bar.open && engine.portfolio().qty(&bar.symbol) == 0 {
                    (weight, "red day")
                } else if bar.close > bar.open {
                    (Decimal::ZERO, "green day")
                } else {
                    return None;
                };
            Some(OrderRequest {
                client_id: format!("{}-{session}", bar.symbol),
                symbol: bar.symbol.clone(),
                size: OrderSize::TargetWeight { weight },
                order_type: OrderType::Market,
                tif: TimeInForce::Day,
                reason: reason.to_owned(),
            })
        })
        .collect()
}

fn backtest(market: &Market, config: &RunConfig, restart_after: Option<usize>) -> Outcome {
    let weight = decimal(&config.params["weight"].to_string());
    let sessions = market
        .calendar
        .between(config.start, config.end.expect("backtest end"));
    let mut engine = Engine::new(config, sessions[0].open);
    let mut outcome = Outcome::default();
    let mut collect = |engine: &mut Engine| {
        let outbox = engine.drain();
        outcome.ledger.extend(outbox.ledger);
        outcome.fills.extend(outbox.fills);
        outcome.equity.extend(outbox.equity_points);
        for order in outbox.orders {
            outcome.orders.insert(order.id, order);
        }
    };
    for (index, session) in sessions.iter().enumerate() {
        let bars: Vec<Bar> = market
            .bars
            .iter()
            .filter(|bar| bar.start == session.open)
            .cloned()
            .collect();
        let actions: Vec<CorporateAction> = market
            .actions
            .iter()
            .filter(|action| {
                market
                    .calendar
                    .first_on_or_after(action.ex_date)
                    .map(|s| s.date)
                    == Some(session.date)
            })
            .cloned()
            .collect();
        let clock = bar_visible_at(Timeframe::Day, session.open, session);
        engine.run_cycle(&Cycle {
            session,
            start: session.open,
            visible_at: clock,
            bars: &bars,
            actions: &actions,
            average_daily_volume: &average_daily_volume(market, session, config.cost.adv_sessions),
            delisted: &[],
        });
        collect(&mut engine);
        let orders = red_green(&engine, &bars, weight, session.date);
        let order_session =
            order_session(Timeframe::Day, clock, &market.calendar).expect("next session");
        engine.accept(clock, order_session.date, orders, Vec::new());
        collect(&mut engine);
        if restart_after == Some(index) {
            engine = Engine::restore(config, &engine.checkpoint()).expect("checkpoint restores");
        }
    }
    outcome.steps = sessions.len();
    outcome.portfolio = engine.portfolio().clone();
    outcome.hash = engine.result_hash();
    outcome
}

fn assert_same(expected: &Value, actual: &Value, at: &str) {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, value) in expected {
                let actual = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("{at}.{key} is missing"));
                assert_same(value, actual, &format!("{at}.{key}"));
            }
        }
        (Value::Array(expected), Value::Array(actual)) => {
            assert_eq!(expected.len(), actual.len(), "{at} length");
            for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
                assert_same(expected, actual, &format!("{at}[{index}]"));
            }
        }
        (Value::String(expected), Value::String(actual)) => {
            match (Decimal::from_str(expected), Decimal::from_str(actual)) {
                (Ok(expected), Ok(actual)) => assert_eq!(expected, actual, "{at}"),
                _ => assert_eq!(expected, actual, "{at}"),
            }
        }
        _ => assert_eq!(expected, actual, "{at}"),
    }
}

#[test]
fn red_green_reproduces_the_hand_computed_backtest() {
    let expected: Value =
        serde_json::from_str(&fixture("fixtures/scenarios/red_green.expected.json")).expect("json");
    let config: RunConfig = serde_json::from_value(expected["config"].clone()).expect("config");
    let universe = [Symbol::new("AAA"), Symbol::new("BBB")];
    let market = load_market(&universe);

    let outcome = backtest(&market, &config, None);

    let orders: Vec<Value> = outcome
        .orders
        .values()
        .map(|order| {
            let mut value = serde_json::to_value(order).expect("order json");
            if let Some(fill) = outcome.fills.iter().find(|fill| fill.order_id == order.id) {
                value["fill_price"] = json!(fill.price);
                value["filled_at"] = json!(fill.at);
            }
            value
        })
        .collect();
    let positions: Vec<Value> = outcome
        .portfolio
        .open_positions()
        .map(|(symbol, position)| json!({"symbol": symbol, "qty": position.qty, "last_price": position.last_price}))
        .collect();
    let actual = json!({
        "steps": outcome.steps,
        "ledger": outcome.ledger,
        "orders": orders,
        "equity": outcome.equity,
        "final": {
            "cash": outcome.portfolio.cash,
            "equity": outcome.portfolio.equity(),
            "positions": positions,
        },
    });
    for key in ["steps", "ledger", "orders", "equity", "final"] {
        assert_same(&expected[key], &actual[key], key);
    }

    assert_eq!(outcome.hash, backtest(&market, &config, None).hash);
    let restarted = backtest(&market, &config, Some(4));
    assert_eq!(restarted.ledger, outcome.ledger);
    assert_eq!(restarted.orders, outcome.orders);
    assert_eq!(restarted.equity, outcome.equity);
    assert_eq!(restarted.hash, outcome.hash);
}
