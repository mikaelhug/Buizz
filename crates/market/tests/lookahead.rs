use std::path::{Path, PathBuf};

use buizz_core::market::{
    Adjustment, CorporateAction, CorporateActionKind, DataSource, FundamentalFact,
    FundamentalMetric, PeriodType, Symbol, Timeframe,
};
use buizz_core::protocol::BarRow;
use buizz_core::time::Timestamp;
use buizz_market::{FILE_NAME, HistoryRequest, MarketStore};
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;
use serde_json::Value;

const READERS: usize = 4;
const ROUNDS: usize = 6;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn at(value: &Value) -> Timestamp {
    value.as_str().unwrap().parse().unwrap()
}

fn date(value: &Value) -> NaiveDate {
    value.as_str().unwrap().parse().unwrap()
}

fn floats(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|number| number.as_f64().unwrap())
        .collect()
}

fn assert_close(actual: &[f64], expected: &[f64], tolerance: f64, context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}: {actual:?}");
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{context}: {actual} != {expected}"
        );
    }
}

async fn history(
    store: &MarketStore,
    symbols: &[&str],
    timeframe: Timeframe,
    bars: u32,
    adjust: Adjustment,
    as_of: Timestamp,
) -> Vec<BarRow> {
    store
        .history(HistoryRequest {
            symbols: symbols.iter().map(|symbol| Symbol::new(symbol)).collect(),
            timeframe,
            bars,
            adjust,
            as_of,
        })
        .await
        .unwrap()
}

async fn period_ends(store: &MarketStore, case: &Value) -> Vec<NaiveDate> {
    let metric: FundamentalMetric = serde_json::from_value(case["metric"].clone()).unwrap();
    store
        .fundamentals(
            vec![Symbol::new(case["symbol"].as_str().unwrap())],
            vec![metric],
            at(&case["clock"]),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|point| point.period_end)
        .collect()
}

async fn check_boundaries(store: &MarketStore, expected: &Value) {
    let tolerance = expected["tolerance"].as_f64().unwrap();
    let calendar = store
        .calendar(
            date(&expected["daily"]["start"]),
            date(&expected["daily"]["end"]),
        )
        .await
        .unwrap();

    for case in expected["daily"]["history"].as_array().unwrap() {
        let clock = at(&case["clock"]);
        let adjust: Adjustment = serde_json::from_value(case["adjust"].clone()).unwrap();
        let rows = history(
            store,
            &[case["symbol"].as_str().unwrap()],
            Timeframe::Day,
            u32::try_from(case["bars"].as_u64().unwrap()).unwrap(),
            adjust,
            clock,
        )
        .await;
        for row in &rows {
            let session = calendar.session(row.start.exchange_date()).unwrap();
            assert!(
                session.close <= clock,
                "daily bar {} past {clock}",
                row.start
            );
        }
        let closes: Vec<f64> = rows.iter().map(|row| row.close).collect();
        assert_close(
            &closes,
            &floats(&case["closes"]),
            tolerance,
            &case.to_string(),
        );
        if let Some(volumes) = case.get("volumes") {
            let actual: Vec<f64> = rows.iter().map(|row| row.volume).collect();
            assert_close(&actual, &floats(volumes), tolerance, &case.to_string());
        }
    }

    let fundamentals = expected["daily"]["fundamentals"].as_array().unwrap();
    for case in fundamentals
        .iter()
        .chain(expected["minute"]["fundamentals"].as_array().unwrap())
    {
        let expected_ends: Vec<NaiveDate> = case["period_ends"]
            .as_array()
            .unwrap()
            .iter()
            .map(date)
            .collect();
        assert_eq!(period_ends(store, case).await, expected_ends, "{case}");
    }

    for case in expected["minute"]["daily_history"].as_array().unwrap() {
        let rows = history(
            store,
            &["AAA"],
            Timeframe::Day,
            1,
            Adjustment::Raw,
            at(&case["clock"]),
        )
        .await;
        assert_eq!(
            rows.last().unwrap().start.exchange_date(),
            date(&case["last_bar_date"]),
            "{case}"
        );
    }

    let first = &expected["minute"]["first_step"];
    let rows = history(
        store,
        &["AAA", "BBB"],
        Timeframe::Minute,
        10,
        Adjustment::Raw,
        at(&first["clock"]),
    )
    .await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.start == at(&first["bar_start"])));

    for clock in expected["minute"]["every_30m_clocks"].as_array().unwrap() {
        let clock = at(clock);
        let rows = history(
            store,
            &["AAA", "BBB"],
            Timeframe::Minute,
            30,
            Adjustment::All,
            clock,
        )
        .await;
        assert_eq!(rows.len(), 60, "minute history at {clock}");
        assert!(
            rows.iter()
                .all(|row| row.start + TimeDelta::minutes(1) <= clock)
        );
        assert_eq!(
            rows.last().unwrap().start,
            clock - TimeDelta::minutes(1),
            "latest minute bar at {clock}"
        );
    }
}

fn future_split() -> CorporateAction {
    CorporateAction {
        symbol: Symbol::new("AAA"),
        ex_date: NaiveDate::from_ymd_opt(2024, 12, 3).unwrap(),
        kind: CorporateActionKind::Split {
            old_rate: Decimal::ONE,
            new_rate: Decimal::from(3),
        },
    }
}

fn future_filing() -> FundamentalFact {
    FundamentalFact {
        symbol: Symbol::new("BBB"),
        cik: 9_999_001,
        taxonomy: "us-gaap".to_owned(),
        concept: "NetIncomeLoss".to_owned(),
        unit: "USD".to_owned(),
        metric: Some(FundamentalMetric::NetIncome),
        period_type: Some(PeriodType::Quarter),
        period_start: NaiveDate::from_ymd_opt(2024, 4, 1),
        period_end: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
        value: Decimal::from(98_000_000),
        fiscal_year: Some(2024),
        fiscal_period: Some("Q2".to_owned()),
        form: "10-Q/A".to_owned(),
        accession: "0009999001-24-000051".to_owned(),
        filed: NaiveDate::from_ymd_opt(2024, 12, 2).unwrap(),
        frame: None,
        available_at: "2024-12-03T14:30:00Z".parse().unwrap(),
    }
}

async fn rewrite(store: MarketStore, symbols: Vec<Symbol>, loaded_through: Timestamp) {
    let from: Timestamp = "2024-11-01T00:00:00Z".parse().unwrap();
    let to: Timestamp = "2025-01-01T00:00:00Z".parse().unwrap();
    for round in 0..ROUNDS {
        for timeframe in [Timeframe::Day, Timeframe::Minute] {
            let bars = store
                .bars_between(timeframe, symbols.clone(), from, to)
                .await
                .unwrap();
            store
                .upsert_bars(DataSource::Fixture, timeframe, bars)
                .await
                .unwrap();
        }
        store
            .upsert_corporate_actions(DataSource::Fixture, vec![future_split()])
            .await
            .unwrap();
        store
            .append_fundamental_facts(DataSource::Fixture, vec![future_filing()])
            .await
            .unwrap();
        let through = loaded_through + TimeDelta::minutes(i64::try_from(round).unwrap());
        store
            .advance_watermark(Timeframe::Minute, through)
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn as_of_boundaries_hold_while_a_writer_rewrites_the_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), READERS)
        .await
        .unwrap();
    store
        .load_fixture(&fixtures().join("market"))
        .await
        .unwrap();
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("scenarios/lookahead.expected.json")).unwrap(),
    )
    .unwrap();
    let symbols: Vec<Symbol> = ["AAA", "BBB", "SPY"].map(Symbol::new).to_vec();
    let minute_from: Timestamp = "2024-11-29T14:30:00Z".parse().unwrap();
    let minute_to: Timestamp = "2024-12-03T00:00:00Z".parse().unwrap();
    let loaded = store
        .bars_between(Timeframe::Minute, symbols.clone(), minute_from, minute_to)
        .await
        .unwrap();

    let loaded_through = store
        .watermarks()
        .borrow()
        .minute
        .expect("the fixture advances the minute watermark");
    assert_eq!(loaded_through, "2024-12-02T21:00:00Z".parse().unwrap());
    let writer = tokio::spawn(rewrite(store.clone(), symbols.clone(), loaded_through));
    let readers: Vec<_> = (0..READERS)
        .map(|_| {
            let store = store.clone();
            let expected = expected.clone();
            tokio::spawn(async move {
                for _ in 0..ROUNDS {
                    check_boundaries(&store, &expected).await;
                }
            })
        })
        .collect();
    writer.await.unwrap();
    for reader in readers {
        reader.await.unwrap();
    }
    check_boundaries(&store, &expected).await;

    let rewritten = store
        .bars_between(Timeframe::Minute, symbols, minute_from, minute_to)
        .await
        .unwrap();
    assert_eq!(rewritten, loaded);
    assert_eq!(
        store.watermarks().borrow().minute,
        Some(loaded_through + TimeDelta::minutes(i64::try_from(ROUNDS - 1).unwrap()))
    );
}
