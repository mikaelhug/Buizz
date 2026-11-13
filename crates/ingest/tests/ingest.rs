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
use buizz_core::market::{
    Adjustment, AssetClass, BorrowAvailability, DataSource, Dataset, IndexHistory, IndexMember,
    MacroSeries, StockIndex, Symbol, Timeframe,
};
use buizz_core::ops::{FetchStatus, JobOutcome};
use buizz_core::time::Timestamp;
use buizz_ingest::{Ingest, IngestConfig, TickReport};
use buizz_market::{HistoryRequest, MarketStore};
use buizz_sources::Sources;
use buizz_sources::alpaca::{AlpacaClient, AlpacaConfig};
use buizz_sources::cboe::{CboeClient, CboeConfig};
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
const FORM_4: &str = include_str!("../../sources/fixtures/edgar/form4_sample.xml");
const FILING_INDEX: &str = include_str!("../../sources/fixtures/edgar/filing_index_sample.htm");
const EX_99: &str = include_str!("../../sources/fixtures/edgar/ex99_sample.htm");
const NO_NEWS: &str = r#"{"news":[],"next_page_token":null}"#;
const YIELDS: &str = include_str!("../../sources/fixtures/treasury/par_yield_2025.csv");
const VIX: &str = include_str!("../../sources/fixtures/cboe/VIX_History.csv");
const VIX3M: &str = include_str!("../../sources/fixtures/cboe/VIX3M_History.csv");
const VIX_REVISED_ROW: (&str, &str) = (
    "09/22/2003,19.080000,23.900000,19.030000,19.650000",
    "09/22/2003,19.080000,23.900000,19.030000,19.700000",
);

const HISTORY_START: &str = "2016-01-01";
const ALPACA_FIRST_BAR: &str = "2016-03-01";
const CALENDAR_END: &str = "2025-06-30";
const ALPACA_LAST_BAR: &str = "2016-04-29";
const SPY_REVISED: &str = "2024-07-05";
const HOLIDAY: &str = "2024-07-04";
const UNKNOWN: &str = "BADX";
const DELISTED_MEMBER: &str = "K";
const REUSED: &str = "STI";
const ENDED_LAST_BAR: &str = "2016-03-31";
const REUSED_FIRST_BAR: &str = "2016-04-20";
const RENAMED: &str = "ANDV";
const RENAMED_FIRST_BAR: &str = "2016-04-01";
const SILENT: &str = "DOW";
const HALTED: &str = "2016-03-03";
const SILENT_FROM: &str = "2016-03-11";
const SILENT_TO: &str = "2016-06-10";
const SILENT_LAST_BAR: &str = "2016-07-29";

#[derive(Default)]
struct Fake {
    rate_limit_bars: AtomicBool,
    fail_bars: AtomicBool,
    closed: Mutex<Option<NaiveDate>>,
    revised: AtomicBool,
    last_bars: Mutex<Params>,
    listed_later: AtomicBool,
    filed_late: AtomicBool,
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
    let alone = params["symbols"] == DELISTED_MEMBER;
    let delisted = match alone {
        true => requested(
            DELISTED_MEMBER,
            date(ALPACA_FIRST_BAR),
            date(ALPACA_LAST_BAR),
        ),
        false => String::new(),
    };
    let spy = requested(
        "SPY",
        date(ALPACA_FIRST_BAR) + TimeDelta::days(1),
        date(CALENDAR_END),
    );
    let ended = || requested(REUSED, date(ALPACA_FIRST_BAR), date(ENDED_LAST_BAR));
    let today = || requested(REUSED, date(REUSED_FIRST_BAR), date(ALPACA_LAST_BAR));
    let reused = match params.get("asof").map(String::as_str) {
        None => today(),
        Some("-") => [ended(), today()]
            .into_iter()
            .filter(|rows| !rows.is_empty())
            .collect::<Vec<_>>()
            .join(","),
        Some(_) => ended(),
    };
    let renamed = match params.get("asof").map(String::as_str) {
        Some(asof) if asof != "-" && date(asof) >= date(RENAMED_FIRST_BAR) => {
            requested(RENAMED, date(ALPACA_FIRST_BAR), date(ALPACA_LAST_BAR))
        }
        Some(asof) if asof != "-" => String::new(),
        _ => requested(RENAMED, date(RENAMED_FIRST_BAR), date(ALPACA_LAST_BAR)),
    };
    let silent = match params["symbols"].split(',').any(|listed| listed == SILENT) {
        true => weekdays(
            date(&params["start"]).max(date(ALPACA_FIRST_BAR)),
            date(&params["end"][..10]).min(date(SILENT_LAST_BAR)),
        )
        .into_iter()
        .map(|day| {
            let quiet = day == date(HALTED) || (date(SILENT_FROM)..=date(SILENT_TO)).contains(&day);
            let (close, volume, trades) = match quiet {
                true => (50, 0, 0),
                false => (alpaca_close(day), 1000, 10),
            };
            format!(
                r#"{{"t":"{day}T12:00:00Z","o":{close},"h":{close},"l":{close},"c":{close},"v":{volume},"n":{trades}}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(","),
        false => String::new(),
    };
    json(format!(
        r#"{{"bars":{{"AAPL":[{aapl}],"SPY":[{spy}],"{DELISTED_MEMBER}":[{delisted}],"{REUSED}":[{reused}],"{RENAMED}":[{renamed}],"{SILENT}":[{silent}]}},"next_page_token":null}}"#
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
    if params.contains_key("page_token") {
        return json(ACTIONS_PAGE_2.to_owned());
    }
    let market_wide = !params.contains_key("symbols") && params["types"].contains(',');
    if fake.filed_late.load(Ordering::SeqCst) && market_wide {
        let late = r#""cash_dividends": [
      {"symbol": "SPY", "rate": 1.4, "ex_date": "2020-03-20", "process_date": "2024-07-08"},"#;
        return json(ACTIONS_PAGE_1.replacen(r#""cash_dividends": ["#, late, 1));
    }
    json(ACTIONS_PAGE_1.to_owned())
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

async fn cboe(State(fake): State<Arc<Fake>>, Path(file): Path<String>) -> Response {
    match file.as_str() {
        "VIX_History.csv" if fake.revised.load(Ordering::SeqCst) => {
            VIX.replace(VIX_REVISED_ROW.0, VIX_REVISED_ROW.1)
        }
        "VIX_History.csv" => VIX.to_owned(),
        _ => VIX3M.to_owned(),
    }
    .into_response()
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
            "/Archives/edgar/data/320193/000114036126042149/form4.xml",
            get(|State(fake): State<Arc<Fake>>| async move {
                fake.hit("form4");
                FORM_4.into_response()
            }),
        )
        .route(
            "/Archives/edgar/data/320193/000032019326000024/0000320193-26-000024-index.htm",
            get(|| async { FILING_INDEX }),
        )
        .route(
            "/Archives/edgar/data/320193/000032019326000024/a8-kex991q4202609262026.htm",
            get(|| async { EX_99 }),
        )
        .route(
            "/v1beta1/news",
            get(|State(fake): State<Arc<Fake>>| async move {
                fake.hit("news");
                json(NO_NEWS.to_owned())
            }),
        )
        .route(
            "/resource-center/data-chart-center/interest-rates/daily-treasury-rates.csv/{year}/all",
            get(yields),
        )
        .route("/api/global/us_indices/daily_prices/{file}", get(cboe))
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
        cboe: Some(
            CboeClient::new(CboeConfig {
                base_url: url.to_owned(),
            })
            .unwrap(),
        ),
        fed: None,
        factset: None,
        finra: None,
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
        .filter(|row| !matches!(row.dataset, Dataset::SecFilings | Dataset::News))
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

async fn spine(directory: &std::path::Path, url: &str) -> (MarketStore, StateStore, Ingest) {
    spine_with(directory, sources(url)).await
}

async fn spine_with(
    directory: &std::path::Path,
    sources: Sources,
) -> (MarketStore, StateStore, Ingest) {
    let market = MarketStore::open(&directory.join("market.duckdb"), 2)
        .await
        .unwrap();
    let state = StateStore::open(&directory.join("state.db")).await.unwrap();
    let ingest = Ingest::new(
        IngestConfig {
            tick: Duration::from_secs(60),
            snapshot_dir: directory.join("snapshots"),
            snapshot_keep: 1,
            history_start: date(HISTORY_START),
            enabled: true,
            only: None,
        },
        market.clone(),
        state.clone(),
        sources,
    );
    (market, state, ingest)
}

fn sp500(members: &[&str]) -> IndexHistory {
    IndexHistory {
        index: StockIndex::Sp500,
        members: members
            .iter()
            .map(|symbol| IndexMember {
                index: StockIndex::Sp500,
                symbol: Symbol::new(symbol),
                start: Some(date("2010-01-04")),
                end: None,
            })
            .collect(),
        changes: u32::try_from(members.len()).unwrap(),
        reliable_from: Some(date("2011-01-03")),
    }
}

fn borrow(at: Timestamp) -> Vec<BorrowAvailability> {
    vec![BorrowAvailability {
        symbol: Symbol::new("SPY"),
        at,
        con_id: 756733,
        rebate_rate: None,
        fee_rate: None,
        available: 10_000_000,
        available_or_more: true,
    }]
}

#[tokio::test]
async fn a_market_backup_restores_into_an_empty_store_and_ingest_starts_over() {
    let fake = Arc::new(Fake::default());
    let url = serve_fakes(fake.clone()).await;
    let lost = tempfile::tempdir().unwrap();
    let (market, state, ingest) = spine(lost.path(), &url).await;
    let first_at = stamp("2024-07-05T21:00:00Z");
    let source = DataSource::Wikipedia;
    market
        .replace_index_history(source, sp500(&["SPY"]), first_at.exchange_date())
        .await
        .unwrap();
    market
        .append_borrow_availability(DataSource::InteractiveBrokers, borrow(first_at))
        .await
        .unwrap();
    ingest.tick(first_at).await.unwrap();
    let second_at = first_at + TimeDelta::hours(25);
    market
        .replace_index_history(source, sp500(&["SPY", "AAPL"]), second_at.exchange_date())
        .await
        .unwrap();
    market
        .append_borrow_availability(DataSource::InteractiveBrokers, borrow(second_at))
        .await
        .unwrap();
    fake.revised.store(true, Ordering::SeqCst);
    let second = ingest.tick(second_at).await.unwrap();
    assert!(second.ran.contains(&("market_snapshot", JobOutcome::Ok)));
    let backup = lost.path().join("snapshots/market");
    assert_eq!(std::fs::read_dir(&backup).unwrap().count(), 2);
    let full = |market: MarketStore, name: &'static str| {
        let into = lost.path().join(name);
        async move {
            std::fs::create_dir_all(&into).unwrap();
            market.export_unrecoverable(into, 0).await.unwrap().unwrap()
        }
    };
    let original = full(market.clone(), "original").await;
    let replacement = tempfile::tempdir().unwrap();
    state
        .snapshot(&replacement.path().join("state.db"))
        .await
        .unwrap();
    drop((ingest, market, state));

    let (market, state, ingest) = spine(replacement.path(), &url).await;
    let reset_at = second_at + TimeDelta::hours(1);
    let restored = buizz_ingest::restore(&market, &state, backup.clone(), reset_at)
        .await
        .unwrap();
    assert_eq!((restored.parts, restored.batches), (2, original.through));
    assert_eq!(full(market.clone(), "restored").await, original);
    let members = market
        .index_history(StockIndex::Sp500)
        .await
        .unwrap()
        .unwrap()
        .members;
    assert_eq!(members.len(), 2);
    assert_eq!(market.latest_borrow().await.unwrap(), Some(second_at));
    let borrow_coverage = market
        .coverage()
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.dataset == Dataset::Borrow)
        .map(|row| (row.first, row.last));
    assert_eq!(
        borrow_coverage,
        Some((first_at.exchange_date(), second_at.exchange_date()))
    );
    assert!(
        buizz_ingest::restore(&market, &state, backup, reset_at)
            .await
            .is_err()
    );

    let tiingo_hits = fake.hits("tiingo");
    let started_over = ingest.tick(reset_at + TimeDelta::minutes(1)).await.unwrap();
    assert!(started_over.ran.contains(&("calendar", JobOutcome::Ok)));
    assert!(started_over.ran.contains(&("bars_1d", JobOutcome::Ok)));
    assert_eq!(
        rows(&market).await.bars,
        weekdays(date(ALPACA_FIRST_BAR), date(ALPACA_LAST_BAR)).len()
    );
    assert_eq!(fake.hits("tiingo"), tiingo_hits);
    let next = market
        .upsert_instruments(DataSource::Fixture, Vec::new())
        .await
        .unwrap();
    assert!(next.batch_id > original.through);
}

#[tokio::test]
async fn a_start_that_adds_a_macro_series_fetches_it_without_waiting_a_day() {
    let fake = Arc::new(Fake::default());
    let url = serve_fakes(fake.clone()).await;
    let directory = tempfile::tempdir().unwrap();
    let without_cboe = Sources {
        cboe: None,
        ..sources(&url)
    };
    let (market, state, ingest) = spine_with(directory.path(), without_cboe).await;
    let first_at = stamp("2024-07-05T21:00:00Z");
    let ran_vix = |report: TickReport| report.ran.iter().any(|(job, _)| *job == "vix");
    assert!(ran_vix(ingest.tick(first_at).await.unwrap()));
    drop((ingest, market, state));

    let (market, _state, restarted) = spine(directory.path(), &url).await;
    let hour_later = first_at + TimeDelta::hours(1);
    assert!(ran_vix(restarted.tick(hour_later).await.unwrap()));
    assert!(
        !market
            .macro_observations(MacroSeries::Vix3m)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!ran_vix(
        restarted
            .tick(hour_later + TimeDelta::minutes(1))
            .await
            .unwrap()
    ));
}

#[tokio::test]
async fn a_delisted_member_alpaca_prices_only_when_asked_alone_stays_tradable_until_its_last_bar() {
    let fake = Arc::new(Fake::default());
    let url = serve_fakes(fake.clone()).await;
    let directory = tempfile::tempdir().unwrap();
    let (market, _state, ingest) = spine(directory.path(), &url).await;
    let first_at = stamp("2024-07-05T21:00:00Z");
    market
        .replace_index_history(
            DataSource::Wikipedia,
            sp500(&["SPY", DELISTED_MEMBER]),
            first_at.exchange_date(),
        )
        .await
        .unwrap();
    ingest.tick(first_at).await.unwrap();
    ingest.tick(first_at + TimeDelta::hours(25)).await.unwrap();
    let instruments = market.instruments().await.unwrap();
    let listing = |symbol: &str| {
        instruments
            .iter()
            .find(|instrument| instrument.symbol.as_str() == symbol)
            .map(|instrument| (instrument.active, instrument.last_session))
    };
    assert_eq!(
        listing(DELISTED_MEMBER),
        Some((false, Some(date(ALPACA_LAST_BAR))))
    );
    assert_eq!(listing("SPY"), Some((true, None)));
}

#[tokio::test]
async fn a_member_whose_ticker_a_later_company_took_is_priced_as_alpaca_mapped_it_then() {
    let fake = Arc::new(Fake::default());
    let url = serve_fakes(fake.clone()).await;
    let directory = tempfile::tempdir().unwrap();
    let (market, _state, ingest) = spine(directory.path(), &url).await;
    let first_at = stamp("2024-07-05T21:00:00Z");
    let mut members = sp500(&["SPY"]);
    members.members.push(IndexMember {
        index: StockIndex::Sp500,
        symbol: Symbol::new(REUSED),
        start: Some(date("2010-01-04")),
        end: Some(date("2016-04-01")),
    });
    market
        .replace_index_history(DataSource::Wikipedia, members, first_at.exchange_date())
        .await
        .unwrap();
    ingest.tick(first_at).await.unwrap();

    let ended = Symbol::ended(&Symbol::new(REUSED), date(ENDED_LAST_BAR));
    let listing = market
        .instruments()
        .await
        .unwrap()
        .into_iter()
        .find(|instrument| instrument.symbol == ended)
        .map(|instrument| {
            (
                instrument.asset_class,
                instrument.active,
                instrument.last_session,
            )
        });
    assert_eq!(
        listing,
        Some((AssetClass::Stock, false, Some(date(ENDED_LAST_BAR))))
    );
    let closes = |as_of: &'static str| {
        let market = market.clone();
        async move {
            market
                .history(HistoryRequest {
                    symbols: vec![Symbol::new(REUSED)],
                    timeframe: Timeframe::Day,
                    bars: 100,
                    adjust: Adjustment::Raw,
                    as_of: stamp(as_of),
                })
                .await
                .unwrap()
                .iter()
                .map(|row| row.start.exchange_date())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        closes("2016-03-31T21:00:00Z").await,
        weekdays(date(ALPACA_FIRST_BAR), date(ENDED_LAST_BAR)),
        "the company that traded as STI in March 2016"
    );
    assert_eq!(
        closes("2016-04-29T21:00:00Z").await,
        weekdays(date(REUSED_FIRST_BAR), date(ALPACA_LAST_BAR)),
        "the company that took STI later"
    );
}

#[tokio::test]
async fn a_member_alpaca_lists_only_under_its_later_ticker_is_priced_under_its_earlier_one() {
    let fake = Arc::new(Fake::default());
    let url = serve_fakes(fake.clone()).await;
    let directory = tempfile::tempdir().unwrap();
    let (market, _state, ingest) = spine(directory.path(), &url).await;
    let first_at = stamp("2024-07-05T21:00:00Z");
    market
        .replace_index_history(
            DataSource::Wikipedia,
            sp500(&["SPY", RENAMED]),
            first_at.exchange_date(),
        )
        .await
        .unwrap();
    ingest.tick(first_at).await.unwrap();

    let history = market
        .history(HistoryRequest {
            symbols: vec![Symbol::new(RENAMED)],
            timeframe: Timeframe::Day,
            bars: 100,
            adjust: Adjustment::Raw,
            as_of: stamp("2016-04-29T21:00:00Z"),
        })
        .await
        .unwrap();
    assert_eq!(
        history
            .iter()
            .map(|row| row.start.exchange_date())
            .collect::<Vec<_>>(),
        weekdays(date(ALPACA_FIRST_BAR), date(ALPACA_LAST_BAR)),
        "the March sessions it traded under its earlier ticker come from Alpaca's mapping as of its first day as ANDV"
    );
}

#[tokio::test]
async fn sessions_without_a_trade_are_not_prices_and_a_long_silence_ends_the_listing() {
    let fake = Arc::new(Fake::default());
    let url = serve_fakes(fake.clone()).await;
    let directory = tempfile::tempdir().unwrap();
    let (market, _state, ingest) = spine(directory.path(), &url).await;
    let first_at = stamp("2024-07-05T21:00:00Z");
    let mut members = sp500(&["SPY"]);
    members.members.push(IndexMember {
        index: StockIndex::Sp500,
        symbol: Symbol::new(SILENT),
        start: Some(date("2010-01-04")),
        end: Some(date("2016-03-15")),
    });
    market
        .replace_index_history(DataSource::Wikipedia, members, first_at.exchange_date())
        .await
        .unwrap();
    ingest.tick(first_at).await.unwrap();

    let ended = Symbol::ended(&Symbol::new(SILENT), date("2016-03-10"));
    assert!(
        market
            .instruments()
            .await
            .unwrap()
            .iter()
            .any(|instrument| instrument.symbol == ended
                && instrument.last_session == Some(date("2016-03-10")))
    );
    let sessions = |as_of: &'static str| {
        let market = market.clone();
        async move {
            market
                .history(HistoryRequest {
                    symbols: vec![Symbol::new(SILENT)],
                    timeframe: Timeframe::Day,
                    bars: 100,
                    adjust: Adjustment::Raw,
                    as_of: stamp(as_of),
                })
                .await
                .unwrap()
                .iter()
                .map(|row| row.start.exchange_date())
                .collect::<Vec<_>>()
        }
    };
    let traded_before = weekdays(date(ALPACA_FIRST_BAR), date("2016-03-10"))
        .into_iter()
        .filter(|day| *day != date(HALTED))
        .collect::<Vec<_>>();
    assert_eq!(
        sessions("2016-03-10T21:00:00Z").await,
        traded_before,
        "the halted session has no price and the listing ends at its last trade"
    );
    assert_eq!(
        sessions("2016-07-29T21:00:00Z").await,
        weekdays(date("2016-06-13"), date(SILENT_LAST_BAR)),
        "trading after the long silence is another listing"
    );
    assert_eq!(
        market
            .unpriced_members(
                StockIndex::Sp500,
                date(ALPACA_FIRST_BAR),
                date("2016-03-14")
            )
            .await
            .unwrap()
            .into_iter()
            .filter(|gap| gap.symbol.as_str() == SILENT)
            .map(|gap| (gap.from, gap.to))
            .collect::<Vec<_>>(),
        vec![],
        "a halted session and the sessions after the last trade are not missing"
    );
}

#[tokio::test]
async fn ticks_backfill_back_off_and_settle_into_noops() {
    let fake = Arc::new(Fake::default());
    let url = serve_fakes(fake.clone()).await;
    let directory = tempfile::tempdir().unwrap();
    let (market, state, ingest) = spine(directory.path(), &url).await;
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
            ("insider_trades", JobOutcome::Ok),
            ("press_releases", JobOutcome::Ok),
            ("filing_documents", JobOutcome::Noop),
            ("rates", JobOutcome::Ok),
            ("borrow", JobOutcome::Noop),
            ("short_interest", JobOutcome::Noop),
            ("short_volume", JobOutcome::Noop),
            ("macro", JobOutcome::Noop),
            ("news", JobOutcome::Noop),
            ("vix", JobOutcome::Ok),
            ("fomc", JobOutcome::Noop),
            ("earnings_insight", JobOutcome::Noop),
            ("earnings_insight_archive", JobOutcome::Noop),
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
            ("insider_trades", JobOutcome::Noop),
            ("press_releases", JobOutcome::Noop),
            ("filing_documents", JobOutcome::Noop),
            ("rates", JobOutcome::Noop),
            ("borrow", JobOutcome::Noop),
            ("short_interest", JobOutcome::Noop),
            ("short_volume", JobOutcome::Noop),
            ("macro", JobOutcome::Noop),
            ("news", JobOutcome::Noop),
            ("vix", JobOutcome::Ok),
            ("fomc", JobOutcome::Noop),
            ("earnings_insight", JobOutcome::Noop),
            ("state_snapshot", JobOutcome::Ok),
            ("market_snapshot", JobOutcome::Ok),
            ("tiingo_history", JobOutcome::Noop),
        ]
    );
    assert_eq!(rows(&market).await, backfilled);
    let vintages: Vec<(NaiveDate, NaiveDate, String)> = market
        .macro_observations(MacroSeries::Vix)
        .await
        .unwrap()
        .into_iter()
        .filter(|close| close.date == date("2003-09-22"))
        .map(|close| {
            let value = close.value.unwrap().normalize().to_string();
            (close.date, close.published, value)
        })
        .collect();
    assert_eq!(
        vintages,
        [
            (date("2003-09-22"), date("2003-09-22"), "19.65".to_owned()),
            (
                date("2003-09-22"),
                second_at.exchange_date(),
                "19.7".to_owned()
            ),
        ]
    );
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
    fake.filed_late.store(true, Ordering::SeqCst);
    fake.action_requests.lock().unwrap().clear();
    let inside_sip_delay = stamp("2024-07-10T20:10:00Z");
    let later = ingest.tick(inside_sip_delay).await.unwrap();
    assert!(later.ran.contains(&("bars_1d", JobOutcome::Ok)));
    let filed_late = market
        .corporate_actions(
            vec![Symbol::new("SPY")],
            date("2020-03-20"),
            date("2020-03-20"),
        )
        .await
        .unwrap();
    assert_eq!(
        filed_late.len(),
        1,
        "an action Alpaca files years after its ex-date reaches a symbol already backfilled"
    );
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
    let action_starts: Vec<(String, String, bool)> = fake
        .action_requests
        .lock()
        .unwrap()
        .iter()
        .filter(|params| !params.contains_key("page_token"))
        .map(|params| {
            let symbols = params.get("symbols").cloned().unwrap_or_default();
            let renames = params["types"] == "name_change";
            (symbols, params["start"].clone(), renames)
        })
        .collect();
    assert_eq!(
        action_starts,
        vec![
            (String::new(), "2024-03-12".to_owned(), true),
            (String::new(), "2015-12-02".to_owned(), false),
            (String::new(), "2024-03-12".to_owned(), false),
            ("BADX,MSFT".to_owned(), "2015-12-02".to_owned(), false),
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
