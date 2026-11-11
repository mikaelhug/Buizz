use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Router, serve};
use buizz_core::api::SpineEvent;
use buizz_core::market::{DataSource, Dataset, Symbol, Timeframe};
use buizz_core::ops::{FetchStatus, JobOutcome};
use buizz_core::time::Timestamp;
use buizz_ingest::{Ingest, IngestConfig};
use buizz_market::MarketStore;
use buizz_sources::Sources;
use buizz_sources::alpaca::{AlpacaClient, AlpacaConfig};
use buizz_sources::edgar::{EdgarClient, EdgarConfig};
use buizz_sources::tiingo::{TiingoClient, TiingoConfig};
use buizz_sources::treasury::{TreasuryClient, TreasuryConfig};
use buizz_state::StateStore;
use chrono::{Datelike, NaiveDate, TimeDelta, Weekday};
use tokio::net::TcpListener;

const ASSETS: &str = include_str!("../../sources/fixtures/alpaca/assets.json");
const ACTIONS_PAGE_1: &str =
    include_str!("../../sources/fixtures/alpaca/corporate_actions_page1.json");
const ACTIONS_PAGE_2: &str =
    include_str!("../../sources/fixtures/alpaca/corporate_actions_page2.json");
const TICKERS: &str = include_str!("../../sources/fixtures/edgar/company_tickers.json");
const COMPANY_FACTS: &str = include_str!("../../sources/fixtures/edgar/companyfacts_sample.json");
const YIELDS: &str = include_str!("../../sources/fixtures/treasury/par_yield_2025.csv");

const HISTORY_START: &str = "2016-01-01";
const ALPACA_FIRST_BAR: &str = "2016-03-01";
const CALENDAR_END: &str = "2025-06-30";
const ALPACA_LAST_BAR: &str = "2016-04-29";

#[derive(Default)]
struct Fake {
    rate_limit_bars: AtomicBool,
    hits: Mutex<HashMap<&'static str, u32>>,
}

impl Fake {
    fn hit(&self, route: &'static str) {
        *self.hits.lock().unwrap().entry(route).or_default() += 1;
    }

    fn hits(&self, route: &'static str) -> u32 {
        self.hits.lock().unwrap().get(route).copied().unwrap_or(0)
    }
}

type Params = HashMap<String, String>;

fn date(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

fn stamp(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn weekdays(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    from.iter_days()
        .take_while(|day| *day <= to)
        .filter(|day| !matches!(day.weekday(), Weekday::Sat | Weekday::Sun))
        .collect()
}

fn alpaca_close(day: NaiveDate) -> i64 {
    100 + (day - date(HISTORY_START)).num_days()
}

fn json(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

async fn calendar(Query(params): Query<Params>) -> Response {
    let days: Vec<String> = weekdays(
        date(&params["start"]),
        date(&params["end"]).min(date(CALENDAR_END)),
    )
    .into_iter()
    .map(|day| format!(r#"{{"date":"{day}","open":"09:30","close":"16:00"}}"#))
    .collect();
    json(format!("[{}]", days.join(",")))
}

async fn bars(State(fake): State<Arc<Fake>>, Query(params): Query<Params>) -> Response {
    fake.hit("bars");
    if fake.rate_limit_bars.load(Ordering::SeqCst) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "7200")],
        )
            .into_response();
    }
    let from = date(&params["start"]).max(date(ALPACA_FIRST_BAR));
    let to = date(&params["end"]).min(date(ALPACA_LAST_BAR));
    let rows: Vec<String> = weekdays(from, to)
        .into_iter()
        .map(|day| {
            let close = alpaca_close(day);
            format!(
                r#"{{"t":"{day}T12:00:00Z","o":{close},"h":{close},"l":{close},"c":{close},"v":1000}}"#
            )
        })
        .collect();
    let aapl = if params["symbols"].contains("AAPL") {
        rows.join(",")
    } else {
        String::new()
    };
    json(format!(
        r#"{{"bars":{{"AAPL":[{aapl}]}},"next_page_token":null}}"#
    ))
}

async fn actions(Query(params): Query<Params>) -> Response {
    json(
        if params.contains_key("page_token") {
            ACTIONS_PAGE_2
        } else {
            ACTIONS_PAGE_1
        }
        .to_owned(),
    )
}

async fn tiingo(
    State(fake): State<Arc<Fake>>,
    Path(symbol): Path<String>,
    Query(params): Query<Params>,
) -> Response {
    fake.hit("tiingo");
    if symbol != "AAPL" {
        return StatusCode::NOT_FOUND.into_response();
    }
    let rows: Vec<String> = weekdays(date(&params["startDate"]), date(&params["endDate"]))
        .into_iter()
        .map(|day| {
            let close = alpaca_close(day) * 2;
            format!(
                r#"{{"date":"{day}T00:00:00.000Z","open":{close},"high":{close},"low":{close},"close":{close},"volume":1000,"divCash":0.0,"splitFactor":1.0}}"#
            )
        })
        .collect();
    json(format!("[{}]", rows.join(",")))
}

async fn tickers() -> Response {
    json(TICKERS.to_owned())
}

async fn company_facts(Path(file): Path<String>) -> Response {
    if file == "CIK0000320193.json" {
        json(COMPANY_FACTS.to_owned())
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn yields() -> Response {
    YIELDS.to_owned().into_response()
}

async fn serve_fakes(fake: Arc<Fake>) -> String {
    let router = Router::new()
        .route("/v2/calendar", get(calendar))
        .route("/v2/assets", get(|| async { json(ASSETS.to_owned()) }))
        .route("/v2/stocks/bars", get(bars))
        .route("/v1/corporate-actions", get(actions))
        .route("/tiingo/daily/{symbol}/prices", get(tiingo))
        .route("/files/company_tickers.json", get(tickers))
        .route("/api/xbrl/companyfacts/{file}", get(company_facts))
        .route(
            "/resource-center/data-chart-center/interest-rates/daily-treasury-rates.csv/{year}/all",
            get(yields),
        )
        .with_state(fake);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { serve(listener, router).await.unwrap() });
    url
}

fn sources(url: &str) -> Sources {
    let mut alpaca = AlpacaConfig::new("key".to_owned(), "secret".to_owned());
    alpaca.data_url = url.to_owned();
    alpaca.trading_url = url.to_owned();
    let mut tiingo = TiingoConfig::new("key".to_owned());
    tiingo.base_url = url.to_owned();
    let mut edgar = EdgarConfig::new("buizz-test contact@example.test".to_owned());
    edgar.data_url = url.to_owned();
    edgar.www_url = url.to_owned();
    Sources {
        alpaca: Some(AlpacaClient::new(alpaca).unwrap()),
        tiingo: Some(TiingoClient::new(tiingo).unwrap()),
        edgar: Some(EdgarClient::new(edgar).unwrap()),
        treasury: TreasuryClient::new(TreasuryConfig {
            base_url: url.to_owned(),
        })
        .unwrap(),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Rows {
    sessions: usize,
    instruments: usize,
    bars: usize,
    actions: usize,
    rates: usize,
    coverage: Vec<String>,
}

async fn rows(market: &MarketStore) -> Rows {
    let aapl = Symbol::new("AAPL");
    let symbols = vec![aapl.clone(), Symbol::new("SPY")];
    let mut coverage: Vec<String> = market
        .coverage()
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            format!(
                "{:?} {:?} {:?} {} {}",
                row.dataset, row.symbol, row.source, row.first, row.last
            )
        })
        .collect();
    coverage.sort();
    Rows {
        sessions: market
            .calendar(date(HISTORY_START), date("2030-01-01"))
            .await
            .unwrap()
            .sessions()
            .len(),
        instruments: market.instruments().await.unwrap().len(),
        bars: market
            .bars_between(
                Timeframe::Day,
                vec![aapl],
                stamp("2015-01-01T00:00:00Z"),
                stamp("2030-01-01T00:00:00Z"),
            )
            .await
            .unwrap()
            .len(),
        actions: market
            .corporate_actions(symbols, date(HISTORY_START), date("2030-01-01"))
            .await
            .unwrap()
            .len(),
        rates: market
            .risk_free_rates(date("2025-01-01"), date("2025-12-31"))
            .await
            .unwrap()
            .len(),
        coverage,
    }
}

#[tokio::test]
async fn ticks_backfill_back_off_and_settle_into_noops() {
    let fake = Arc::new(Fake::default());
    let url = serve_fakes(fake.clone()).await;
    let directory = tempfile::tempdir().unwrap();
    let market = MarketStore::open(&directory.path().join("market.duckdb"), 2)
        .await
        .unwrap();
    let state = StateStore::open(&directory.path().join("state.db"))
        .await
        .unwrap();
    let ingest = Ingest::new(
        IngestConfig {
            tick: Duration::from_secs(60),
            snapshot_dir: directory.path().join("snapshots"),
            snapshot_keep: 1,
            history_start: date(HISTORY_START),
        },
        market.clone(),
        state.clone(),
        sources(&url),
    );
    let mut events = ingest.events();

    let first_at = stamp("2024-07-05T21:00:00Z");
    let first = ingest.tick(first_at).await.unwrap();
    assert_eq!(
        first.ran,
        vec![
            ("calendar", JobOutcome::Ok),
            ("instruments", JobOutcome::Ok),
            ("bars_1d", JobOutcome::Ok),
            ("corporate_actions", JobOutcome::Ok),
            ("fundamentals", JobOutcome::Ok),
            ("rates", JobOutcome::Ok),
            ("state_snapshot", JobOutcome::Ok),
            ("tiingo_history", JobOutcome::Failed),
        ]
    );
    let backfilled = rows(&market).await;
    assert!(backfilled.sessions > 2_000);
    assert_eq!(backfilled.instruments, 3);
    assert_eq!(
        backfilled.bars,
        weekdays(date(ALPACA_FIRST_BAR), date(ALPACA_LAST_BAR)).len()
    );
    assert_eq!(backfilled.actions, 2);
    assert_eq!(backfilled.rates, 4);
    assert!(market.watermarks().borrow().day.is_some());
    let facts = state.latest_jobs().await.unwrap();
    assert!(
        facts
            .iter()
            .find(|run| run.job == "fundamentals")
            .is_some_and(|run| run.rows > 0)
    );

    let views = ingest.job_views(first_at).await.unwrap();
    let tiingo = views
        .iter()
        .find(|view| view.name == "tiingo_history")
        .unwrap();
    assert_eq!(tiingo.last_outcome, Some(JobOutcome::Failed));
    let message = tiingo.last_message.as_deref().unwrap();
    assert!(message.contains("AAPL") && message.contains("rejected"));
    assert!(tiingo.stale);
    assert!(
        views
            .iter()
            .filter(|view| view.name != "tiingo_history")
            .all(|view| !view.stale)
    );
    let mut failed_events = 0;
    while let Ok(event) = events.try_recv() {
        if let SpineEvent::Job { job } = event {
            failed_events += usize::from(job.last_outcome == Some(JobOutcome::Failed));
        }
    }
    assert_eq!(failed_events, 1);

    let second = ingest.tick(first_at + TimeDelta::hours(48)).await.unwrap();
    assert_eq!(
        second.ran,
        vec![
            ("calendar", JobOutcome::Noop),
            ("instruments", JobOutcome::Noop),
            ("bars_1d", JobOutcome::Noop),
            ("corporate_actions", JobOutcome::Noop),
            ("fundamentals", JobOutcome::Noop),
            ("rates", JobOutcome::Noop),
            ("state_snapshot", JobOutcome::Ok),
            ("tiingo_history", JobOutcome::Noop),
        ]
    );
    assert_eq!(rows(&market).await, backfilled);
    assert_eq!(fake.hits("tiingo"), 1);
    let snapshots: Vec<_> = std::fs::read_dir(directory.path().join("snapshots"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(snapshots, vec!["state-2024-07-07.db".to_owned()]);

    fake.rate_limit_bars.store(true, Ordering::SeqCst);
    let limited_at = first_at + TimeDelta::hours(55);
    let limited = ingest.tick(limited_at).await.unwrap();
    assert!(limited.ran.contains(&("bars_1d", JobOutcome::Backoff)));
    let fetch = state
        .recent_fetches(DataSource::Alpaca, Dataset::Bars1d, None, 1)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(fetch.status, FetchStatus::RateLimited);
    assert!(fetch.retry_after.unwrap() >= limited_at + TimeDelta::hours(2));

    let hits = fake.hits("bars");
    let retried = ingest
        .tick(limited_at + TimeDelta::minutes(61))
        .await
        .unwrap();
    assert!(retried.ran.contains(&("bars_1d", JobOutcome::Backoff)));
    assert_eq!(fake.hits("bars"), hits);
}
