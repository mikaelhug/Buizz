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
use buizz_core::api::{JobView, SpineEvent};
use buizz_core::market::{AssetClass, DataSource, Dataset, Symbol, Timeframe};
use buizz_core::ops::{FetchStatus, JobOutcome};
use buizz_core::time::Timestamp;
use buizz_ingest::{Ingest, IngestConfig, TickReport};
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
const FUND_TICKERS: &str = include_str!("../../sources/fixtures/edgar/company_tickers_mf.json");
const SUBMISSIONS: &str = include_str!("../../sources/fixtures/edgar/submissions_sample.json");
const COMPANY_FACTS: &str = include_str!("../../sources/fixtures/edgar/companyfacts_sample.json");
const YIELDS: &str = include_str!("../../sources/fixtures/treasury/par_yield_2025.csv");

const HISTORY_START: &str = "2016-01-01";
const ALPACA_FIRST_BAR: &str = "2016-03-01";
const CALENDAR_END: &str = "2025-06-30";
const ALPACA_LAST_BAR: &str = "2016-04-29";
const SPY_REVISED: &str = "2024-07-05";
const HOLIDAY: &str = "2024-07-04";
const UNKNOWN: &str = "BADX";

#[derive(Default)]
struct Fake {
    rate_limit_bars: AtomicBool,
    fail_bars: AtomicBool,
    closed: Mutex<Option<NaiveDate>>,
    revised: AtomicBool,
    last_bars: Mutex<Params>,
    listed_later: AtomicBool,
    action_requests: Mutex<Vec<Params>>,
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

async fn calendar(State(fake): State<Arc<Fake>>, Query(params): Query<Params>) -> Response {
    let closed = *fake.closed.lock().unwrap();
    let days: Vec<String> = weekdays(
        date(&params["start"]),
        date(&params["end"]).min(date(CALENDAR_END)),
    )
    .into_iter()
    .filter(|day| Some(*day) != closed)
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
    if fake.fail_bars.load(Ordering::SeqCst) {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if params["symbols"].split(',').any(|symbol| symbol == UNKNOWN) {
        return (StatusCode::UNPROCESSABLE_ENTITY, "invalid symbol").into_response();
    }
    *fake.last_bars.lock().unwrap() = params.clone();
    let requested = |symbol: &str, first: NaiveDate, last: NaiveDate| {
        if !params["symbols"].split(',').any(|listed| listed == symbol) {
            return String::new();
        }
        let from = date(&params["start"]).max(first);
        let to = date(&params["end"][..10]).min(last);
        let rows: Vec<String> = weekdays(from, to)
            .into_iter()
            .map(|day| {
                let revision = i64::from(
                    symbol == "SPY" && day == date(SPY_REVISED) && fake.revised.load(Ordering::SeqCst),
                );
                let close = alpaca_close(day) + revision;
                format!(
                    r#"{{"t":"{day}T12:00:00Z","o":{close},"h":{close},"l":{close},"c":{close},"v":1000}}"#
                )
            })
            .collect();
        rows.join(",")
    };
    let aapl = requested("AAPL", date(ALPACA_FIRST_BAR), date(ALPACA_LAST_BAR));
    let spy = requested(
        "SPY",
        date(ALPACA_FIRST_BAR) + TimeDelta::days(1),
        date(CALENDAR_END),
    );
    json(format!(
        r#"{{"bars":{{"AAPL":[{aapl}],"SPY":[{spy}]}},"next_page_token":null}}"#
    ))
}

async fn assets(State(fake): State<Arc<Fake>>) -> Response {
    let later = r#",{"class":"us_equity","exchange":"NASDAQ","symbol":"BADX","name":"Bad Ticker","status":"active","shortable":false},{"class":"us_equity","exchange":"NASDAQ","symbol":"MSFT","name":"Microsoft Corporation Common Stock","status":"active","shortable":true}]"#;
    if fake.listed_later.load(Ordering::SeqCst) {
        json(ASSETS.trim_end().trim_end_matches(']').to_owned() + later)
    } else {
        json(ASSETS.to_owned())
    }
}

async fn actions(State(fake): State<Arc<Fake>>, Query(params): Query<Params>) -> Response {
    fake.action_requests.lock().unwrap().push(params.clone());
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

async fn daily_index(Path((_, _, file)): Path<(String, String, String)>) -> Response {
    if file == format!("master.{}.idx", HOLIDAY.replace('-', "")) {
        StatusCode::NOT_FOUND.into_response()
    } else {
        String::new().into_response()
    }
}

async fn yields() -> Response {
    YIELDS.to_owned().into_response()
}

async fn serve_fakes(fake: Arc<Fake>) -> String {
    let router = Router::new()
        .route("/v2/calendar", get(calendar))
        .route("/v2/assets", get(assets))
        .route("/v2/stocks/bars", get(bars))
        .route("/v1/corporate-actions", get(actions))
        .route("/tiingo/daily/{symbol}/prices", get(tiingo))
        .route("/files/company_tickers.json", get(tickers))
        .route(
            "/files/company_tickers_mf.json",
            get(|| async { json(FUND_TICKERS.to_owned()) }),
        )
        .route("/api/xbrl/companyfacts/{file}", get(company_facts))
        .route(
            "/Archives/edgar/daily-index/{year}/{quarter}/{file}",
            get(daily_index),
        )
        .route(
            "/submissions/CIK0000320193.json",
            get(|| async { json(SUBMISSIONS.to_owned()) }),
        )
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
        fred: None,
        treasury: TreasuryClient::new(TreasuryConfig {
            base_url: url.to_owned(),
        })
        .unwrap(),
        wikipedia: None,
        ibkr: None,
        nasdaq: None,
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
        .filter(|row| row.dataset != Dataset::SecFilings)
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
            enabled: true,
            only: None,
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
            ("index_membership", JobOutcome::Noop),
            ("bars_1d", JobOutcome::Ok),
            ("corporate_actions", JobOutcome::Ok),
            ("sec_index", JobOutcome::Noop),
            ("fundamentals", JobOutcome::Ok),
            ("filings", JobOutcome::Ok),
            ("rates", JobOutcome::Ok),
            ("borrow", JobOutcome::Noop),
            ("macro", JobOutcome::Noop),
            ("state_snapshot", JobOutcome::Ok),
            ("market_snapshot", JobOutcome::Ok),
            ("tiingo_history", JobOutcome::Failed),
        ]
    );
    let backfilled = rows(&market).await;
    assert!(backfilled.sessions > 2_000);
    assert_eq!(backfilled.instruments, 3);
    let classes: Vec<(String, AssetClass)> = market
        .instruments()
        .await
        .unwrap()
        .into_iter()
        .map(|instrument| {
            (
                instrument.symbol.as_str().to_owned(),
                instrument.asset_class,
            )
        })
        .collect();
    assert_eq!(
        classes,
        vec![
            ("AAPL".to_owned(), AssetClass::Stock),
            ("SIVB".to_owned(), AssetClass::Stock),
            ("SPY".to_owned(), AssetClass::Etf),
        ]
    );
    assert_eq!(
        backfilled.bars,
        weekdays(date(ALPACA_FIRST_BAR), date(ALPACA_LAST_BAR)).len()
    );
    assert_eq!(backfilled.actions, 2);
    assert_eq!(backfilled.rates, 4);
    assert!(market.watermarks().borrow().day.is_some());
    let filings_read_through = market
        .coverage()
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.dataset == Dataset::SecFilings)
        .map(|row| row.last);
    assert_eq!(filings_read_through, Some(date(SPY_REVISED)));
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

    std::fs::create_dir_all(directory.path().join("snapshots/stale.partial")).unwrap();
    fake.revised.store(true, Ordering::SeqCst);
    let revised = ingest.tick(first_at + TimeDelta::hours(6)).await.unwrap();
    assert!(revised.ran.contains(&("bars_1d", JobOutcome::Ok)));
    let spy = market
        .bars_between(
            Timeframe::Day,
            vec![Symbol::new("SPY")],
            stamp("2024-07-05T00:00:00Z"),
            stamp("2024-07-06T00:00:00Z"),
        )
        .await
        .unwrap();
    assert_eq!(spy[0].close, (alpaca_close(date(SPY_REVISED)) + 1).into());

    let second_at = first_at + TimeDelta::hours(25);
    let bar_requests = fake.hits("bars");
    let second = ingest.tick(second_at).await.unwrap();
    assert_eq!(fake.hits("bars"), bar_requests);
    assert_eq!(
        second.ran,
        vec![
            ("calendar", JobOutcome::Noop),
            ("instruments", JobOutcome::Noop),
            ("index_membership", JobOutcome::Noop),
            ("bars_1d", JobOutcome::Noop),
            ("corporate_actions", JobOutcome::Noop),
            ("sec_index", JobOutcome::Noop),
            ("fundamentals", JobOutcome::Noop),
            ("filings", JobOutcome::Noop),
            ("rates", JobOutcome::Noop),
            ("borrow", JobOutcome::Noop),
            ("macro", JobOutcome::Noop),
            ("state_snapshot", JobOutcome::Ok),
            ("market_snapshot", JobOutcome::Ok),
            ("tiingo_history", JobOutcome::Noop),
        ]
    );
    assert_eq!(rows(&market).await, backfilled);
    let calendar = ingest
        .job_views(second_at)
        .await
        .unwrap()
        .into_iter()
        .find(|view| view.name == "calendar")
        .unwrap();
    assert_eq!(calendar.next_due, Some(second_at + TimeDelta::days(1)));
    assert_eq!(fake.last_bars.lock().unwrap()["symbols"], "SPY");
    assert_eq!(fake.hits("tiingo"), 1);
    let listing = |path: &str| {
        let mut names: Vec<String> = std::fs::read_dir(directory.path().join(path))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    };
    assert_eq!(listing("snapshots"), ["market", "state-2024-07-06.db"]);
    let parts = listing("snapshots/market");
    assert_eq!(parts.len(), 2);
    let (first_from, first_through) = parts[0].split_once('-').unwrap();
    let (second_from, _) = parts[1].split_once('-').unwrap();
    assert_eq!(first_from.parse::<i64>().unwrap(), 1);
    assert_eq!(
        second_from.parse::<i64>().unwrap(),
        first_through.parse::<i64>().unwrap() + 1
    );
    assert!(
        directory
            .path()
            .join("snapshots/market")
            .join(&parts[1])
            .join("bar_1d_revision.parquet")
            .exists()
    );
    assert!(!directory.path().join("snapshots/stale.partial").exists());

    *fake.closed.lock().unwrap() = Some(date("2025-01-09"));
    fake.fail_bars.store(true, Ordering::SeqCst);
    let failed_at = first_at + TimeDelta::hours(72);
    let failed = ingest.tick(failed_at).await.unwrap();
    assert!(failed.ran.contains(&("calendar", JobOutcome::Ok)));
    assert_eq!(rows(&market).await.sessions, backfilled.sessions - 1);
    assert!(failed.ran.contains(&("bars_1d", JobOutcome::Failed)));
    fake.fail_bars.store(false, Ordering::SeqCst);
    let recovered = ingest
        .tick(failed_at + TimeDelta::minutes(6))
        .await
        .unwrap();
    assert_eq!(recovered.ran, vec![("bars_1d", JobOutcome::Ok)]);

    fake.rate_limit_bars.store(true, Ordering::SeqCst);
    let limited_at = failed_at + TimeDelta::hours(6) + TimeDelta::minutes(10);
    let limited = ingest.tick(limited_at).await.unwrap();
    assert!(limited.ran.contains(&("bars_1d", JobOutcome::Backoff)));
    let fetch = state
        .recent_fetches(DataSource::Alpaca, Dataset::Bars1d, None, 1)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(fetch.status, FetchStatus::RateLimited);
    assert!(fetch.retry_after.unwrap() >= limited_at + TimeDelta::hours(2));

    let bars = |views: Vec<JobView>| views.into_iter().find(|view| view.name == "bars_1d");
    let paused = bars(ingest.job_views(limited_at).await.unwrap()).unwrap();
    assert_eq!(paused.next_due, fetch.retry_after);
    let hits = fake.hits("bars");
    let retried = ingest
        .tick(limited_at + TimeDelta::minutes(61))
        .await
        .unwrap();
    assert!(!retried.ran.iter().any(|(job, _)| *job == "bars_1d"));
    assert_eq!(fake.hits("bars"), hits);

    fake.rate_limit_bars.store(false, Ordering::SeqCst);
    fake.listed_later.store(true, Ordering::SeqCst);
    fake.action_requests.lock().unwrap().clear();
    let inside_sip_delay = stamp("2024-07-10T20:10:00Z");
    let later = ingest.tick(inside_sip_delay).await.unwrap();
    assert!(later.ran.contains(&("bars_1d", JobOutcome::Ok)));
    assert!(fake.last_bars.lock().unwrap()["end"].starts_with("2024-07-09"));
    assert_eq!(
        market.watermarks().borrow().day,
        Some(stamp("2024-07-09T20:00:00Z"))
    );
    let latest = |symbol: &str| {
        let state = state.clone();
        let symbol = Symbol::new(symbol);
        async move {
            state
                .recent_fetches(DataSource::Alpaca, Dataset::Bars1d, Some(symbol), 1)
                .await
                .unwrap()
                .remove(0)
                .status
        }
    };
    assert_eq!(latest(UNKNOWN).await, FetchStatus::Rejected);
    assert_eq!(latest("MSFT").await, FetchStatus::Empty);
    let action_starts: Vec<(String, String)> = fake
        .action_requests
        .lock()
        .unwrap()
        .iter()
        .filter(|params| !params.contains_key("page_token"))
        .map(|params| {
            let symbols = params.get("symbols").cloned().unwrap_or_default();
            (symbols, params["start"].clone())
        })
        .collect();
    assert_eq!(
        action_starts,
        vec![
            (String::new(), "2024-03-12".to_owned()),
            ("BADX,MSFT".to_owned(), "2015-12-02".to_owned()),
        ]
    );

    let killed_at = inside_sip_delay + TimeDelta::hours(2);
    state.start_job("tiingo_history", killed_at).await.unwrap();
    let ran_tiingo =
        |report: TickReport| report.ran.iter().any(|(job, _)| *job == "tiingo_history");
    let crashed = ingest
        .tick(killed_at + TimeDelta::minutes(1))
        .await
        .unwrap();
    assert!(!ran_tiingo(crashed));
    let backed_off = ingest
        .tick(killed_at + TimeDelta::minutes(6))
        .await
        .unwrap();
    assert!(ran_tiingo(backed_off));

    let month_later = ingest.tick(first_at + TimeDelta::days(31)).await.unwrap();
    assert!(
        month_later
            .ran
            .contains(&("tiingo_history", JobOutcome::Noop))
    );
    assert_eq!(fake.hits("tiingo"), 1);
}
