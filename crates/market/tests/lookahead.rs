use std::path::{Path, PathBuf};

use buizz_core::market::{
    Adjustment, CorporateAction, CorporateActionKind, DataSource, FundamentalFact,
    FundamentalMetric, InsiderFiling, InsiderOwner, InsiderTransaction, NewsArticle, PeriodType,
    Session, Symbol, Timeframe,
};
use buizz_core::protocol::BarRow;
use buizz_core::time::Timestamp;
use buizz_market::{ActionWindow, ChartBars, ChartRequest, FILE_NAME, HistoryRequest, MarketStore};
use chrono::{Datelike, NaiveDate, TimeDelta};
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
        let split = future_split();
        store
            .replace_corporate_actions(
                DataSource::Fixture,
                vec![split.clone()],
                ActionWindow {
                    symbols: vec![split.symbol.clone()],
                    from: split.ex_date,
                    through: split.ex_date,
                    covered_to: Some(split.ex_date),
                },
            )
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

#[tokio::test]
async fn a_late_closure_stays_a_published_session_until_it_was_announced() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    let day = |text: &str| text.parse::<NaiveDate>().unwrap();
    let sessions: Vec<Session> = day("2018-11-26")
        .iter_days()
        .take_while(|date| *date <= day("2018-12-11"))
        .filter(|date| date.weekday().number_from_monday() <= 5 && *date != day("2018-12-05"))
        .map(|date| Session {
            date,
            open: format!("{date}T14:30:00Z").parse().unwrap(),
            close: format!("{date}T21:00:00Z").parse().unwrap(),
        })
        .collect();
    store
        .upsert_sessions(DataSource::Fixture, sessions)
        .await
        .unwrap();
    let dates = |clock: &str| -> Vec<NaiveDate> {
        store
            .published_sessions(day("2018-12-03"), day("2018-12-07"), clock.parse().unwrap())
            .iter()
            .map(|session| session.date)
            .collect()
    };

    assert_eq!(
        store.published_sessions(
            day("2018-12-05"),
            day("2018-12-05"),
            "2018-11-30T21:00:00Z".parse().unwrap(),
        ),
        [Session {
            date: day("2018-12-05"),
            open: "2018-12-05T14:30:00Z".parse().unwrap(),
            close: "2018-12-05T21:00:00Z".parse().unwrap(),
        }]
    );
    assert_eq!(
        dates("2018-12-03T14:29:00Z"),
        [
            "2018-12-03",
            "2018-12-04",
            "2018-12-05",
            "2018-12-06",
            "2018-12-07"
        ]
        .map(day)
    );
    assert_eq!(
        dates("2018-12-03T14:30:00Z"),
        ["2018-12-03", "2018-12-04", "2018-12-06", "2018-12-07"].map(day)
    );
}

#[tokio::test]
async fn a_price_chart_stops_at_the_clock_and_adjusts_only_for_splits_visible_by_then() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    store
        .load_fixture(&fixtures().join("market"))
        .await
        .unwrap();
    let stamp = |text: &str| text.parse::<Timestamp>().unwrap();
    let chart = |timeframe, from: &str, before: Option<&str>, bars, as_of: &str| {
        store.chart(ChartRequest {
            symbol: Symbol::new("AAA"),
            timeframe,
            from: stamp(from),
            after: None,
            before: before.map(stamp),
            bars,
            as_of: stamp(as_of),
        })
    };
    let opens = |chart: &ChartBars| chart.bars.iter().map(|bar| bar.open).collect::<Vec<_>>();
    let starts = |chart: &ChartBars| chart.bars.iter().map(|bar| bar.start).collect::<Vec<_>>();

    let before_split = chart(
        Timeframe::Day,
        "2024-11-20T05:00:00Z",
        None,
        100,
        "2024-11-25T21:00:00Z",
    )
    .await
    .unwrap();
    assert_close(
        &opens(&before_split),
        &[200.0, 203.0, 198.0, 197.5],
        1e-9,
        "raw until the ex-date opens",
    );
    assert!(before_split.splits.is_empty());

    let split_day = chart(
        Timeframe::Day,
        "2024-11-20T05:00:00Z",
        None,
        100,
        "2024-11-26T17:00:00Z",
    )
    .await
    .unwrap();
    assert_close(
        &opens(&split_day),
        &[100.0, 101.5, 99.0, 98.75],
        1e-9,
        "halved once the split is visible, the open session still hidden",
    );
    assert_eq!(split_day.splits.len(), 1);
    assert_eq!(split_day.splits[0].volume, Decimal::TWO);

    let minutes = chart(
        Timeframe::Minute,
        "2024-11-29T05:00:00Z",
        None,
        2,
        "2024-11-29T14:35:30Z",
    )
    .await
    .unwrap();
    assert_eq!(
        starts(&minutes),
        [stamp("2024-11-29T14:33:00Z"), stamp("2024-11-29T14:34:00Z")]
    );
    assert!(minutes.older);
    let earlier = chart(
        Timeframe::Minute,
        "2024-11-29T05:00:00Z",
        Some("2024-11-29T14:33:00Z"),
        2,
        "2024-11-29T14:35:30Z",
    )
    .await
    .unwrap();
    assert_eq!(
        starts(&earlier),
        [stamp("2024-11-29T14:31:00Z"), stamp("2024-11-29T14:32:00Z")]
    );
    assert!(earlier.older);
}

async fn november_2026() -> (tempfile::TempDir, MarketStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    let sessions: Vec<Session> = "2026-11-02"
        .parse::<NaiveDate>()
        .unwrap()
        .iter_days()
        .take(12)
        .filter(|date| date.weekday().number_from_monday() <= 5)
        .map(|date| Session {
            date,
            open: format!("{date}T14:30:00Z").parse().unwrap(),
            close: format!("{date}T21:00:00Z").parse().unwrap(),
        })
        .collect();
    store
        .upsert_sessions(DataSource::Fixture, sessions)
        .await
        .unwrap();
    (dir, store)
}

fn insider_filing(accession: &str, form: &str, accepted_at: &str, price: i64) -> InsiderFiling {
    let accepted_at: Timestamp = accepted_at.parse().unwrap();
    InsiderFiling {
        symbol: Symbol::new("AAPL"),
        issuer_cik: 320193,
        accession: accession.to_owned(),
        form: form.to_owned(),
        filed: accepted_at.exchange_date(),
        accepted_at,
        owners: vec![InsiderOwner {
            cik: 1780525,
            name: "Newstead Jennifer".to_owned(),
            director: false,
            officer: true,
            ten_percent_owner: false,
            other: false,
            officer_title: Some("SVP, GC and Government Affairs".to_owned()),
        }],
        transactions: vec![InsiderTransaction {
            security: "Common Stock".to_owned(),
            date: "2026-11-03".parse().unwrap(),
            code: "S".to_owned(),
            acquired: false,
            shares: Some(Decimal::from(2399)),
            price: Some(Decimal::new(price, 2)),
            shares_after: Some(Decimal::from(29997)),
            direct: true,
        }],
    }
}

#[tokio::test]
async fn an_insider_trade_is_seen_from_the_next_open_and_its_amendment_is_a_later_fact() {
    let (_dir, store) = november_2026().await;
    let original = insider_filing("0001140361-26-042149", "4", "2026-11-05T23:30:13Z", 32575);
    let amendment = insider_filing("0001140361-26-043001", "4/A", "2026-11-09T15:00:00Z", 32757);
    let written = store
        .append_insider_filings(DataSource::Edgar, vec![original.clone(), amendment])
        .await
        .unwrap();
    assert_eq!(written.rows, 2);
    let again = store
        .append_insider_filings(DataSource::Edgar, vec![original])
        .await
        .unwrap();
    assert_eq!(again.rows, 0);
    let since: NaiveDate = "2026-11-01".parse().unwrap();
    let seen = |clock: &str| {
        let store = store.clone();
        let clock: Timestamp = clock.parse().unwrap();
        async move {
            store
                .insider_trades(vec![Symbol::new("AAPL")], since, clock)
                .await
                .unwrap()
                .into_iter()
                .map(|trade| {
                    (
                        trade.form,
                        trade.transaction.price.unwrap(),
                        trade.available_at.to_string(),
                    )
                })
                .collect::<Vec<_>>()
        }
    };

    assert!(seen("2026-11-06T14:29:59Z").await.is_empty());
    let original_seen = (
        "4".to_owned(),
        Decimal::new(32575, 2),
        "2026-11-06T14:30:00Z".to_owned(),
    );
    assert_eq!(
        seen("2026-11-06T14:30:00Z").await,
        std::slice::from_ref(&original_seen)
    );
    assert_eq!(
        seen("2026-11-09T21:00:00Z").await,
        std::slice::from_ref(&original_seen)
    );
    assert_eq!(
        seen("2026-11-10T14:30:00Z").await,
        [
            original_seen,
            (
                "4/A".to_owned(),
                Decimal::new(32757, 2),
                "2026-11-10T14:30:00Z".to_owned()
            )
        ]
    );
}

fn article(updated_at: &str, received_at: &str, backfilled: bool) -> NewsArticle {
    NewsArticle {
        id: 24843171,
        symbols: vec![Symbol::new("AAPL"), Symbol::new("QQQ")],
        headline: format!("Apple Shares Rise ({updated_at})"),
        summary: String::new(),
        source: "benzinga".to_owned(),
        created_at: "2026-11-05T20:41:12Z".parse().unwrap(),
        updated_at: updated_at.parse().unwrap(),
        received_at: received_at.parse().unwrap(),
        backfilled,
    }
}

#[tokio::test]
async fn news_keeps_every_version_and_shows_each_from_when_it_was_known() {
    let (_dir, store) = november_2026().await;
    let original = article("2026-11-05T20:41:12Z", "2026-11-05T20:41:13Z", false);
    let edit = article("2026-11-05T20:47:55Z", "2026-11-05T20:47:56Z", false);
    let later_edit = article("2026-11-05T22:10:00Z", "2026-11-07T05:00:00Z", true);
    let recorded = store
        .append_news(DataSource::AlpacaStream, vec![original.clone(), edit], None)
        .await
        .unwrap();
    assert_eq!(recorded.rows, 2);
    let backfill = store
        .append_news(
            DataSource::Alpaca,
            vec![later_edit, original],
            Some(("2026-11-05".parse().unwrap(), "2026-11-05".parse().unwrap())),
        )
        .await
        .unwrap();
    assert_eq!(backfill.rows, 1, "the stream already holds the original");
    let versions = |clock: &str| {
        let store = store.clone();
        let clock: Timestamp = clock.parse().unwrap();
        async move {
            store
                .news(
                    vec![Symbol::new("QQQ")],
                    "2026-11-05".parse().unwrap(),
                    clock,
                )
                .await
                .unwrap()
                .into_iter()
                .map(|article| (article.updated_at.to_string(), article.backfilled))
                .collect::<Vec<_>>()
        }
    };

    assert!(versions("2026-11-05T20:41:12Z").await.is_empty());
    assert_eq!(
        versions("2026-11-05T20:47:55Z").await,
        [("2026-11-05T20:41:12Z".to_owned(), false)]
    );
    assert_eq!(versions("2026-11-05T22:10:00Z").await.len(), 3);
}
