use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use buizz_core::market::{
    Adjustment, AssetClass, Bar, CorporateAction, CorporateActionKind, DataSource, Dataset, Fact,
    FactStatus, FailToDeliverRecord, Feed, Filing, FundamentalFact, FundamentalMetric,
    IndexHistory, IndexMember, InsiderFiling, InsiderOwner, InsiderTransaction, Instrument,
    NewsArticle, PeriodType, RecipeScope, RecipeState, RecipeVersion, Session, StockIndex, Symbol,
    Timeframe, TitleViews,
};
use buizz_core::protocol::BarRow;
use buizz_core::run::UniverseSpec;
use buizz_core::time::Timestamp;
use buizz_market::{
    ActionWindow, ChartBars, ChartRequest, FILE_NAME, FactsRequest, HistoryRequest, MarketStore,
};
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
            listed_on: stamp(as_of).exchange_date(),
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
        plan_10b5_1: (form == "4").then_some(true),
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
async fn an_insider_trade_is_seen_from_the_next_open_and_its_amendment_replaces_it_from_its_own() {
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
                        trade.plan_10b5_1,
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
        Some(true),
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
        [(
            "4/A".to_owned(),
            Decimal::new(32757, 2),
            "2026-11-10T14:30:00Z".to_owned(),
            None
        )],
        "the amended sale is one sale, not a second one"
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

#[tokio::test]
async fn reorganizations_adjust_history_once_priced_and_a_reused_ticker_starts_afresh() {
    let (_dir, store) = november_2026().await;
    let daily = |symbol: &str, date: &str, open: i64, close: i64| Bar {
        symbol: Symbol::new(symbol),
        start: format!("{date}T14:30:00Z").parse().unwrap(),
        open: Decimal::from(open),
        high: Decimal::from(open.max(close)),
        low: Decimal::from(open.min(close)),
        close: Decimal::from(close),
        volume: 1_000,
        feed: Feed::Sip,
    };
    let bars = vec![
        daily("AAA", "2026-11-02", 100, 100),
        daily("AAA", "2026-11-03", 100, 100),
        daily("AAA", "2026-11-04", 90, 90),
        daily("AAA", "2026-11-05", 90, 90),
        daily("AAA", "2026-11-06", 72, 72),
        daily("BBB", "2026-11-04", 20, 20),
        daily("CCC", "2026-11-02", 50, 50),
        daily("CCC", "2026-11-03", 50, 50),
        daily("CCC", "2026-11-09", 5, 5),
        daily("CCC", "2026-11-10", 5, 5),
    ];
    store
        .upsert_bars(DataSource::Fixture, Timeframe::Day, bars)
        .await
        .unwrap();
    let action = |symbol: &str, date: &str, kind| CorporateAction {
        symbol: Symbol::new(symbol),
        ex_date: date.parse().unwrap(),
        kind,
    };
    let actions = vec![
        action(
            "AAA",
            "2026-11-04",
            CorporateActionKind::SpinOff {
                new_symbol: Symbol::new("BBB"),
                old_rate: Decimal::TWO,
                new_rate: Decimal::ONE,
            },
        ),
        action(
            "AAA",
            "2026-11-06",
            CorporateActionKind::StockDividend {
                rate: "0.25".parse().unwrap(),
            },
        ),
        action(
            "CCC",
            "2026-11-04",
            CorporateActionKind::CashMerger {
                rate: Decimal::from(51),
            },
        ),
    ];
    let window = ActionWindow {
        symbols: vec![Symbol::new("AAA"), Symbol::new("CCC")],
        from: "2026-11-02".parse().unwrap(),
        through: "2026-11-13".parse().unwrap(),
        covered_to: None,
    };
    store
        .replace_corporate_actions(DataSource::Fixture, actions, window)
        .await
        .unwrap();
    let closes = |symbol: &'static str, adjust, clock: &'static str| {
        let store = store.clone();
        async move {
            history(
                &store,
                &[symbol],
                Timeframe::Day,
                10,
                adjust,
                clock.parse().unwrap(),
            )
            .await
            .iter()
            .map(|row| row.close)
            .collect::<Vec<_>>()
        }
    };

    assert_close(
        &closes("AAA", Adjustment::All, "2026-11-04T15:00:00Z").await,
        &[100.0, 100.0],
        1e-9,
        "the spun-off shares have no price until their first session closes",
    );
    assert_close(
        &closes("AAA", Adjustment::All, "2026-11-04T21:00:00Z").await,
        &[90.0, 90.0, 90.0],
        1e-9,
        "half a BBB at its 20 open is 10 of the 100 close: 1 - 10/100",
    );
    assert_close(
        &closes("AAA", Adjustment::Split, "2026-11-06T21:00:00Z").await,
        &[80.0, 80.0, 72.0, 72.0, 72.0],
        1e-9,
        "a 25% stock dividend divides earlier prices by 1.25, a spin-off is no split",
    );
    assert_close(
        &closes("CCC", Adjustment::Raw, "2026-11-03T21:00:00Z").await,
        &[50.0, 50.0],
        1e-9,
        "the acquired company's history while it traded",
    );
    assert_close(
        &closes("CCC", Adjustment::Raw, "2026-11-10T21:00:00Z").await,
        &[5.0, 5.0],
        1e-9,
        "the listing that later reuses the ticker starts after the merger",
    );
}

#[tokio::test]
async fn a_ticker_names_the_company_that_traded_under_it_at_the_clock() {
    let (_dir, store) = november_2026().await;
    let ended = Symbol::ended(&Symbol::new("STI"), "2026-11-04".parse().unwrap());
    let instrument = |symbol: &Symbol, asset_class, last_session: Option<&str>| Instrument {
        symbol: symbol.clone(),
        name: symbol.to_string(),
        exchange: String::new(),
        asset_class,
        active: last_session.is_none(),
        shortable: true,
        first_session: None,
        last_session: last_session.map(|date| date.parse().unwrap()),
    };
    store
        .upsert_instruments(
            DataSource::Fixture,
            vec![
                instrument(&Symbol::new("FB"), AssetClass::Etf, None),
                instrument(&Symbol::new("META"), AssetClass::Stock, None),
                instrument(&Symbol::new("METV"), AssetClass::Etf, None),
                instrument(&Symbol::new("STI"), AssetClass::Stock, None),
                instrument(&ended, AssetClass::Stock, Some("2026-11-04")),
            ],
        )
        .await
        .unwrap();
    let daily = |symbol: &Symbol, dates: &[&str], close: i64| -> Vec<Bar> {
        dates
            .iter()
            .map(|date| Bar {
                symbol: symbol.clone(),
                start: format!("{date}T14:30:00Z").parse().unwrap(),
                open: Decimal::from(close),
                high: Decimal::from(close),
                low: Decimal::from(close),
                close: Decimal::from(close),
                volume: 1_000,
                feed: Feed::Sip,
            })
            .collect()
    };
    let facebook = ["2026-11-02", "2026-11-03", "2026-11-04", "2026-11-05"];
    let later = [
        "2026-11-06",
        "2026-11-09",
        "2026-11-10",
        "2026-11-11",
        "2026-11-12",
    ];
    let bars = [
        daily(&Symbol::new("META"), &facebook, 100),
        daily(&Symbol::new("META"), &later, 110),
        daily(&Symbol::new("FB"), &facebook, 100),
        daily(&Symbol::new("FB"), &["2026-11-11", "2026-11-12"], 25),
        daily(&Symbol::new("METV"), &facebook, 15),
        daily(&ended, &facebook[..3], 40),
        daily(&Symbol::new("STI"), &["2026-11-11", "2026-11-12"], 4),
    ]
    .concat();
    store
        .upsert_bars(DataSource::Fixture, Timeframe::Day, bars)
        .await
        .unwrap();
    let renamed = |old: &str, new: &str, date: &str| CorporateAction {
        symbol: Symbol::new(old),
        ex_date: date.parse().unwrap(),
        kind: CorporateActionKind::NameChange {
            new_symbol: Symbol::new(new),
        },
    };
    store
        .replace_name_changes(
            DataSource::Fixture,
            vec![
                renamed("META", "METV", "2026-11-04"),
                renamed("FB", "META", "2026-11-06"),
            ],
            ActionWindow {
                symbols: Vec::new(),
                from: "2026-11-02".parse().unwrap(),
                through: "2026-11-13".parse().unwrap(),
                covered_to: None,
            },
        )
        .await
        .unwrap();
    let member = |symbol: &str, end: Option<&str>, article: &str| IndexMember {
        index: StockIndex::Sp500,
        symbol: Symbol::new(symbol),
        start: Some("2026-11-02".parse().unwrap()),
        end: end.map(|date| date.parse().unwrap()),
        article: Some(article.to_owned()),
    };
    store
        .replace_index_history(
            DataSource::Fixture,
            IndexHistory {
                index: StockIndex::Sp500,
                members: vec![
                    member("META", None, "Meta Platforms"),
                    member("STI", Some("2026-11-05"), "SunTrust Banks"),
                ],
                changes: 2,
                reliable_from: None,
            },
            "2026-11-13".parse().unwrap(),
        )
        .await
        .unwrap();
    let closes = |symbol: &'static str, clock: &'static str| {
        let store = store.clone();
        async move {
            history(
                &store,
                &[symbol],
                Timeframe::Day,
                20,
                Adjustment::Raw,
                clock.parse().unwrap(),
            )
            .await
            .iter()
            .map(|row| (row.symbol.to_string(), row.close))
            .collect::<Vec<_>>()
        }
    };
    let series = |symbol: &str, closes: &[f64]| -> Vec<(String, f64)> {
        closes
            .iter()
            .map(|close| (symbol.to_owned(), *close))
            .collect()
    };

    assert_eq!(
        closes("FB", "2026-11-05T21:00:00Z").await,
        series("FB", &[100.0; 4]),
        "before its rename FB is the company that trades as META today"
    );
    assert_eq!(
        closes("META", "2026-11-03T21:00:00Z").await,
        series("META", &[15.0, 15.0]),
        "before the fund gave META up, META is the fund"
    );
    assert_eq!(
        closes("META", "2026-11-12T21:00:00Z").await,
        series(
            "META",
            &[
                100.0, 100.0, 100.0, 100.0, 110.0, 110.0, 110.0, 110.0, 110.0
            ]
        ),
        "the fund leaving the ticker does not cut the company's history"
    );
    assert_eq!(
        closes("FB", "2026-11-12T21:00:00Z").await,
        series("FB", &[25.0, 25.0]),
        "after the rename FB is the fund that took the ticker"
    );
    assert_eq!(
        closes("STI", "2026-11-04T21:00:00Z").await,
        series("STI", &[40.0, 40.0, 40.0])
    );
    assert_eq!(
        closes("STI", "2026-11-12T21:00:00Z").await,
        series("STI", &[4.0, 4.0])
    );

    let index = UniverseSpec::Index {
        index: StockIndex::Sp500,
    };
    let members = |date: &'static str| {
        let (store, index) = (store.clone(), index.clone());
        async move {
            store
                .universe(&index, date.parse().unwrap(), 3)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        members("2026-11-04").await,
        vec![Symbol::new("FB"), Symbol::new("STI")]
    );
    assert_eq!(members("2026-11-10").await, vec![Symbol::new("META")]);

    let class = |clock: &'static str| {
        let store = store.clone();
        async move {
            store
                .instrument_profiles(vec![Symbol::new("FB")], clock.parse().unwrap())
                .await
                .unwrap()
                .into_iter()
                .map(|profile| (profile.symbol, profile.asset_class))
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        class("2026-11-05T21:00:00Z").await,
        [(Symbol::new("FB"), AssetClass::Stock)]
    );
    assert_eq!(
        class("2026-11-12T21:00:00Z").await,
        [(Symbol::new("FB"), AssetClass::Etf)]
    );

    let day = |text: &str| -> NaiveDate { text.parse().unwrap() };
    let viewed = |title: &str, date: &str, views: u64| TitleViews {
        title: title.to_owned(),
        date: day(date),
        views,
        published: day(date) + TimeDelta::days(1),
    };
    store
        .append_page_views(
            DataSource::Wikipedia,
            BTreeMap::from([(
                "Meta Platforms".to_owned(),
                BTreeSet::from(["Facebook, Inc.".to_owned(), "Meta Platforms".to_owned()]),
            )]),
            vec![
                viewed("Facebook, Inc.", "2026-11-04", 900),
                viewed("Meta Platforms", "2026-11-04", 100),
                viewed("Meta Platforms", "2026-11-09", 700),
            ],
        )
        .await
        .unwrap();
    let views = |symbol: &'static str, clock: &'static str| {
        let store = store.clone();
        async move {
            store
                .page_views(
                    vec![Symbol::new(symbol)],
                    day("2026-11-02"),
                    clock.parse().unwrap(),
                )
                .await
                .unwrap()
                .into_iter()
                .map(|row| {
                    (
                        row.symbol.to_string(),
                        row.date.to_string(),
                        row.views,
                        row.available_at.to_string(),
                    )
                })
                .collect::<Vec<_>>()
        }
    };
    let row = |symbol: &str, date: &str, views: u64, at: &str| {
        (symbol.to_owned(), date.to_owned(), views, at.to_owned())
    };
    assert!(views("FB", "2026-11-05T14:29:59Z").await.is_empty());
    assert_eq!(
        views("FB", "2026-11-05T14:30:00Z").await,
        [row("FB", "2026-11-04", 1000, "2026-11-05T14:30:00Z")],
        "a day's views count every title the article had and show from the next morning's open"
    );
    assert_eq!(
        views("META", "2026-11-12T21:00:00Z").await,
        [
            row("META", "2026-11-04", 1000, "2026-11-05T14:30:00Z"),
            row("META", "2026-11-09", 700, "2026-11-10T14:30:00Z"),
        ]
    );
    assert!(
        views("FB", "2026-11-12T21:00:00Z").await.is_empty(),
        "the fund that took the FB ticker has no company article"
    );

    let failed =
        |symbol: &str, settlement: &str, quantity: u64, published: &str| FailToDeliverRecord {
            symbol: Symbol::new(symbol),
            settlement: day(settlement),
            cusip: "30303M102".to_owned(),
            quantity,
            price: None,
            published: day(published),
        };
    store
        .append_fails_to_deliver(
            DataSource::Edgar,
            "cnsfails202611a".to_owned(),
            day("2026-11-12"),
            vec![
                failed("FB", "2026-11-03", 500, "2026-11-06"),
                failed("META", "2026-11-09", 300, "2026-11-11"),
            ],
        )
        .await
        .unwrap();
    let fails = |clock: &'static str| {
        let store = store.clone();
        async move {
            store
                .fails_to_deliver(
                    vec![Symbol::new("META")],
                    day("2026-11-02"),
                    clock.parse().unwrap(),
                )
                .await
                .unwrap()
                .into_iter()
                .map(|row| {
                    (
                        row.symbol.to_string(),
                        row.quantity,
                        row.available_at.to_string(),
                    )
                })
                .collect::<Vec<_>>()
        }
    };
    assert!(fails("2026-11-09T14:29:59Z").await.is_empty());
    assert_eq!(
        fails("2026-11-12T21:00:00Z").await,
        [
            ("META".to_owned(), 500, "2026-11-09T14:30:00Z".to_owned()),
            ("META".to_owned(), 300, "2026-11-12T14:30:00Z".to_owned()),
        ],
        "fails filed under FB before the rename belong to the company, each from the open after its file was public"
    );

    let withdrawn = store
        .withdraw_shadowed_bars(
            DataSource::Fixture,
            "2026-11-02".parse().unwrap(),
            "2026-11-13".parse().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(withdrawn.rows, 4, "FB's copies of the company's bars");
    let stored_fb = store
        .bars_between(
            Timeframe::Day,
            vec![Symbol::new("FB")],
            "2026-11-01T00:00:00Z".parse().unwrap(),
            "2026-11-14T00:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stored_fb.len(), 2);
    assert_eq!(
        closes("FB", "2026-11-05T21:00:00Z").await,
        series("FB", &[100.0; 4])
    );
}

#[tokio::test]
async fn reading_a_filing_again_replaces_the_facts_the_store_held_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), READERS)
        .await
        .unwrap();
    let count = |period_end: NaiveDate, value: i64| FundamentalFact {
        concept: "CommonStockSharesOutstanding".to_owned(),
        unit: "shares".to_owned(),
        metric: Some(FundamentalMetric::SharesOutstanding),
        period_start: None,
        period_end,
        value: Decimal::from(value),
        ..future_filing()
    };
    let (prior_year, quarter) = (
        NaiveDate::from_ymd_opt(2023, 12, 31).unwrap(),
        NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
    );
    store
        .append_fundamental_facts(
            DataSource::Fixture,
            vec![
                count(prior_year, 492_025_443_000_000),
                count(quarter, 498_143_219_000_000),
            ],
        )
        .await
        .unwrap();
    store
        .append_fundamental_facts(DataSource::Fixture, vec![count(quarter, 498_143_219)])
        .await
        .unwrap();

    let held: Vec<(NaiveDate, Decimal)> = store
        .fundamentals(
            vec![Symbol::new("BBB")],
            vec![FundamentalMetric::SharesOutstanding],
            "2025-01-01T00:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|point| (point.period_end, point.value))
        .collect();
    assert_eq!(held, vec![(quarter, Decimal::from(498_143_219))]);
}

#[tokio::test]
async fn a_filer_tagging_error_or_an_old_reporting_currency_never_reaches_a_bot() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), READERS)
        .await
        .unwrap();
    let day = |month: u32, last: u32| NaiveDate::from_ymd_opt(2024, month, last).unwrap();
    let instant = |metric, unit: &str, period_end, value: i64| FundamentalFact {
        unit: unit.to_owned(),
        metric: Some(metric),
        period_start: None,
        period_end,
        value: Decimal::from(value),
        ..future_filing()
    };
    let quarter = |metric, unit: &str, period_end: NaiveDate, value: i64| FundamentalFact {
        period_start: Some(period_end - chrono::TimeDelta::days(90)),
        ..instant(metric, unit, period_end, value)
    };
    let shares = FundamentalMetric::SharesOutstanding;
    store
        .append_fundamental_facts(
            DataSource::Fixture,
            vec![
                instant(shares, "shares", day(3, 31), 500_000_000),
                instant(shares, "shares", day(6, 30), 53_000_000_000_000_000),
                instant(shares, "shares", day(9, 30), 502_000_000),
                quarter(
                    FundamentalMetric::EpsDiluted,
                    "USD/shares",
                    day(6, 30),
                    498_000_000,
                ),
                quarter(FundamentalMetric::Revenue, "CNY", day(3, 31), 700_000_000),
                quarter(FundamentalMetric::Revenue, "USD", day(6, 30), 100_000_000),
                instant(
                    FundamentalMetric::LongTermDebt,
                    "USD",
                    day(12, 31) + chrono::TimeDelta::days(3650),
                    1,
                ),
            ],
        )
        .await
        .unwrap();

    let read: Vec<(FundamentalMetric, NaiveDate, String)> = store
        .fundamentals(
            vec![Symbol::new("BBB")],
            vec![
                shares,
                FundamentalMetric::EpsDiluted,
                FundamentalMetric::Revenue,
                FundamentalMetric::LongTermDebt,
            ],
            "2025-01-01T00:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|point| (point.metric, point.period_end, point.unit))
        .collect();
    assert_eq!(
        read,
        vec![
            (FundamentalMetric::Revenue, day(6, 30), "USD".to_owned()),
            (shares, day(3, 31), "shares".to_owned()),
            (shares, day(9, 30), "shares".to_owned()),
        ],
        "a share count tagged as EPS, a count 10^8 times its neighbours, a debt maturity filed as a period and revenue in the currency the filer left are all held back"
    );
}

#[tokio::test]
async fn a_live_run_sees_a_fact_once_produced_and_a_backtest_from_its_filing() {
    let (_dir, store) = november_2026().await;
    let accepted_at: Timestamp = "2026-11-05T23:30:00Z".parse().unwrap();
    let accession = "0000320193-26-000101".to_owned();
    store
        .append_current_reports(
            DataSource::Edgar,
            vec![Filing {
                symbol: Symbol::new("AAPL"),
                cik: 320193,
                accession: accession.clone(),
                form: "8-K".to_owned(),
                filed: accepted_at.exchange_date(),
                report_date: None,
                items: vec!["3.01".to_owned()],
                accepted_at,
                available_at: accepted_at,
            }],
        )
        .await
        .unwrap();
    let version = RecipeVersion {
        recipe: "listing_notice".to_owned(),
        version: 1,
        schema: r#"{"type": "object", "properties": {"exchange": {"type": "string"}}}"#.to_owned(),
        producer: "code".to_owned(),
        prompt_hash: None,
        scope: RecipeScope {
            items: vec!["3.01".to_owned()],
            exhibits: Vec::new(),
            from: "2026-11-01".parse().unwrap(),
        },
        state: RecipeState::Filling,
        gold: None,
    };
    store
        .register_recipe(Dataset::ListingNotices, version)
        .await
        .unwrap();
    for state in [RecipeState::Complete, RecipeState::Current] {
        store
            .advance_recipe(
                Dataset::ListingNotices,
                "listing_notice".to_owned(),
                1,
                state,
                None,
            )
            .await
            .unwrap();
    }
    store
        .append_facts(
            Dataset::ListingNotices,
            vec![Fact {
                recipe: "listing_notice".to_owned(),
                version: 1,
                doc_id: format!("edgar:{accession}:1"),
                section_key: "8-K:3.01".to_owned(),
                ordinal: 0,
                accession: Some(accession),
                cik: Some(320193),
                status: FactStatus::Ok,
                fields: Some(r#"{"exchange": "nasdaq"}"#.to_owned()),
                evidence: Some("Nasdaq".to_owned()),
                evidence_start: Some(0),
                reason: None,
                published_at: accepted_at,
                produced_at: "2026-11-09T15:00:00Z".parse().unwrap(),
                input_hash: "hash".to_owned(),
            }],
            None,
            None,
        )
        .await
        .unwrap();
    let seen = |clock: &str, live: bool| {
        let store = store.clone();
        let as_of: Timestamp = clock.parse().unwrap();
        async move {
            store
                .facts(FactsRequest {
                    recipe: "listing_notice".to_owned(),
                    version: 1,
                    symbols: vec![Symbol::new("AAPL")],
                    since: "2026-11-01".parse().unwrap(),
                    as_of,
                    live,
                })
                .await
                .unwrap()
                .into_iter()
                .map(|fact| {
                    (
                        fact.row.available_at.to_string(),
                        fact.row.fields["exchange"].clone(),
                    )
                })
                .collect::<Vec<_>>()
        }
    };

    assert!(seen("2026-11-06T14:29:59Z", false).await.is_empty());
    assert_eq!(
        seen("2026-11-06T14:30:00Z", false).await,
        [("2026-11-06T14:30:00Z".to_owned(), Value::from("nasdaq"))],
        "a backtest reads the fact from when its filing was public"
    );
    assert!(
        seen("2026-11-09T14:59:59Z", true).await.is_empty(),
        "a live run cannot see a fact before it was produced"
    );
    assert_eq!(
        seen("2026-11-09T15:00:00Z", true).await,
        [("2026-11-09T15:00:00Z".to_owned(), Value::from("nasdaq"))]
    );
}
