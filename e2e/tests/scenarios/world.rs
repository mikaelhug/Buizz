use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use buizz_core::market::{
    AssetClass, Bar, CorporateAction, Feed, FundamentalFact, Instrument, RiskFreeRate, Session,
    Symbol, Timeframe,
};
use buizz_core::time::Timestamp;
use buizz_market::MarketStore;
use buizz_sources::treasury::parse_par_yield_csv;
use buizz_sources::wikipedia::{SP500_CHANGES, SP500_CONSTITUENTS};
use chrono::{Datelike, NaiveDate, NaiveTime, TimeDelta, Weekday};
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};

pub const LIVE_SYMBOLS: [&str; 3] = ["AAA", "BBB", "SPY"];
const LIVE_HISTORY_SESSIONS: i64 = 25;
const LIVE_OPENED_MINUTES_AGO: i64 = 10;
const LIVE_SESSION_MINUTES: i64 = 40;
const LIVE_RELEASED_AHEAD_MINUTES: i64 = 5;

pub fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

pub fn stamp(text: &str) -> Timestamp {
    text.parse()
        .unwrap_or_else(|_| panic!("{text} is a timestamp"))
}

pub fn date(text: &str) -> NaiveDate {
    text.parse().unwrap_or_else(|_| panic!("{text} is a date"))
}

pub fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap_or_else(|_| panic!("{text} is a decimal"))
}

pub fn minute_floor(at: Timestamp) -> Timestamp {
    Timestamp::from_nanos(at.nanos() - at.nanos().rem_euclid(60_000_000_000))
}

#[derive(Clone)]
pub struct Company {
    pub cik: u32,
    pub ticker: Symbol,
    pub facts: Value,
    pub submissions: Value,
}

pub struct Filing<'a> {
    pub accession: &'a str,
    pub form: &'a str,
    pub items: &'a str,
    pub accepted: Timestamp,
}

pub fn submissions(cik: u32, sic: &str, filings: &[Filing<'_>]) -> Value {
    let column = |value: fn(&Filing<'_>) -> Value| filings.iter().map(value).collect::<Vec<_>>();
    json!({
        "cik": format!("{cik:010}"),
        "sic": sic,
        "sicDescription": if sic.is_empty() { "" } else { "Fixture industry" },
        "filings": {
            "recent": {
                "accessionNumber": column(|filing| json!(filing.accession)),
                "filingDate": column(|filing| json!(filing.accepted.exchange_date())),
                "reportDate": column(|filing| json!(filing.accepted.exchange_date())),
                "acceptanceDateTime": column(|filing| json!(filing.accepted.to_string())),
                "form": column(|filing| json!(filing.form)),
                "items": column(|filing| json!(filing.items)),
            },
            "files": [],
        },
    })
}

#[derive(Clone, Default)]
pub struct World {
    pub sessions: Vec<Session>,
    pub instruments: Vec<Instrument>,
    pub daily: Vec<Bar>,
    pub minute: Vec<Bar>,
    pub actions: Vec<CorporateAction>,
    pub companies: Vec<Company>,
    pub rates: Vec<RiskFreeRate>,
    pub tiingo: Vec<Bar>,
    pub wikipedia: BTreeMap<String, String>,
    pub borrow: Option<Vec<u8>>,
    pub released: Option<Timestamp>,
    pub streamed_from: Option<Timestamp>,
}

impl World {
    pub async fn fixture_market() -> Self {
        let dir = repository().join("fixtures/market");
        let scratch = tempfile::tempdir().expect("scratch directory");
        let store = MarketStore::open(&scratch.path().join(buizz_market::FILE_NAME), 1)
            .await
            .expect("scratch market store opens");
        store
            .load_fixture(&dir)
            .await
            .expect("the market fixture loads");
        let (from, to) = (date("2000-01-01"), date("2100-01-01"));
        let instruments = store.instruments().await.expect("instruments");
        let symbols: Vec<Symbol> = instruments.iter().map(|i| i.symbol.clone()).collect();
        let bars = |timeframe| {
            store.bars_between(
                timeframe,
                symbols.clone(),
                stamp("2000-01-01T00:00:00Z"),
                stamp("2100-01-01T00:00:00Z"),
            )
        };
        let facts: Vec<FundamentalFact> = csv::Reader::from_path(dir.join("fundamental_facts.csv"))
            .expect("fundamental facts fixture")
            .deserialize()
            .collect::<Result<_, _>>()
            .expect("fundamental facts parse");
        Self {
            sessions: store
                .calendar(from, to)
                .await
                .expect("calendar")
                .sessions()
                .to_vec(),
            daily: bars(Timeframe::Day).await.expect("daily bars"),
            minute: bars(Timeframe::Minute).await.expect("minute bars"),
            actions: store
                .corporate_actions(symbols.clone(), from, to)
                .await
                .expect("corporate actions"),
            rates: store.risk_free_rates(from, to).await.expect("rates"),
            companies: companies(&facts),
            instruments,
            ..Self::default()
        }
    }

    pub fn live(now: Timestamp) -> Self {
        let today = now.exchange_date();
        let open = minute_floor(now) - TimeDelta::minutes(LIVE_OPENED_MINUTES_AGO);
        let close = open + TimeDelta::minutes(LIVE_SESSION_MINUTES);
        assert_eq!(
            close.exchange_date(),
            today,
            "the live session must close before midnight in New York"
        );
        let session_on = |date: NaiveDate| Session {
            date,
            open: Timestamp::at_exchange_time(date, open.exchange_time()).expect("open exists"),
            close: Timestamp::at_exchange_time(date, close.exchange_time()).expect("close exists"),
        };
        let mut sessions: Vec<Session> = (1..=LIVE_HISTORY_SESSIONS)
            .rev()
            .map(|days| session_on(today - TimeDelta::days(days)))
            .collect();
        let live = session_on(today);
        let instruments = LIVE_SYMBOLS
            .iter()
            .map(|symbol| instrument(symbol, *symbol == "SPY", true))
            .collect();
        let mut daily = Vec::new();
        for (index, symbol) in LIVE_SYMBOLS.iter().enumerate() {
            for (day, session) in sessions.iter().enumerate() {
                let base = Decimal::from(100 + 50 * index) + Decimal::new(day as i64 % 5, 1);
                daily.push(bar(
                    symbol,
                    session.open,
                    [base, base + Decimal::ONE, base - Decimal::ONE, base],
                    50_000_000,
                ));
            }
        }
        let minute = LIVE_SYMBOLS
            .iter()
            .enumerate()
            .flat_map(|(index, symbol)| live_tape(symbol, index, &live))
            .collect();
        sessions.push(live);
        Self {
            sessions,
            instruments,
            daily,
            minute,
            released: Some(minute_floor(now) + TimeDelta::minutes(LIVE_RELEASED_AHEAD_MINUTES)),
            streamed_from: Some(live.open),
            ..Self::default()
        }
    }

    pub fn recorded_sources() -> Self {
        let sources = repository().join("crates/sources/fixtures");
        let read = |name: &str| {
            std::fs::read_to_string(sources.join(name))
                .unwrap_or_else(|error| panic!("{name}: {error}"))
        };
        let recorded: Vec<Value> =
            serde_json::from_str(&read("alpaca/calendar.json")).expect("recorded calendar");
        let june = date("2024-06-03")
            .iter_days()
            .take_while(|day| *day < date("2024-07-01"))
            .filter(|day| !matches!(day.weekday(), Weekday::Sat | Weekday::Sun));
        let sessions: Vec<Session> = june
            .map(|day| (day, "09:30", "16:00"))
            .chain(recorded.iter().map(|day| {
                (
                    date(day["date"].as_str().expect("date")),
                    day["open"].as_str().expect("open"),
                    day["close"].as_str().expect("close"),
                )
            }))
            .map(|(day, open, close)| {
                let at = |time: &str| {
                    Timestamp::at_exchange_time(
                        day,
                        NaiveTime::parse_from_str(time, "%H:%M").expect("calendar time"),
                    )
                    .expect("time exists")
                };
                Session {
                    date: day,
                    open: at(open),
                    close: at(close),
                }
            })
            .collect();
        let assets: Vec<Value> =
            serde_json::from_str(&read("alpaca/assets.json")).expect("recorded assets");
        let instruments = assets
            .iter()
            .map(|asset| {
                let symbol = asset["symbol"].as_str().expect("symbol");
                let mut instrument = instrument(
                    symbol,
                    symbol == "SPY",
                    asset["status"].as_str() == Some("active"),
                );
                instrument.name = asset["name"].as_str().expect("name").to_owned();
                instrument.exchange = asset["exchange"].as_str().expect("exchange").to_owned();
                instrument
            })
            .collect();
        let mut daily = Vec::new();
        let mut tiingo = Vec::new();
        for (step, session) in sessions.iter().enumerate() {
            let close = Decimal::from(200) + Decimal::from(step);
            let prices = [close, close + Decimal::ONE, close - Decimal::ONE, close];
            daily.push(bar(
                "SPY",
                session.open,
                prices.map(|p| p * Decimal::TWO),
                1_000,
            ));
            if step > 0 {
                daily.push(bar("AAPL", session.open, prices, 1_000));
            }
            let scaled = prices.map(|price| price * Decimal::new(110, 2));
            tiingo.push(bar("AAPL", session.open, scaled, 1_000));
        }
        let tickers: Value =
            serde_json::from_str(&read("edgar/company_tickers.json")).expect("recorded tickers");
        let apple = &tickers["0"];
        Self {
            sessions,
            instruments,
            daily,
            tiingo,
            companies: vec![Company {
                cik: u32::try_from(apple["cik_str"].as_u64().expect("cik")).expect("cik fits"),
                ticker: Symbol::new(apple["ticker"].as_str().expect("ticker")),
                facts: serde_json::from_str(&read("edgar/companyfacts_sample.json"))
                    .expect("recorded company facts"),
                submissions: serde_json::from_str(&read("edgar/submissions_sample.json"))
                    .expect("recorded submissions"),
            }],
            wikipedia: [
                (SP500_CONSTITUENTS, "wikipedia/sp500_constituents.json"),
                (SP500_CHANGES, "wikipedia/sp500_changes.json"),
            ]
            .into_iter()
            .map(|(title, file)| {
                let page: Value = serde_json::from_str(&read(file)).expect("recorded page");
                let wikitext = page["parse"]["wikitext"].as_str().expect("wikitext");
                (title.to_owned(), wikitext.to_owned())
            })
            .collect(),
            borrow: Some(read("ibkr/usa.txt").into_bytes()),
            rates: parse_par_yield_csv(&read("treasury/par_yield_2025.csv"))
                .expect("recorded yields"),
            ..Self::default()
        }
    }

    pub fn session(&self, date: NaiveDate) -> Session {
        *self
            .sessions
            .iter()
            .find(|session| session.date == date)
            .unwrap_or_else(|| panic!("the world has a session on {date}"))
    }

    pub fn live_session(&self) -> Session {
        *self.sessions.last().expect("the live session")
    }

    pub fn mirror_live_minutes(&mut self, from: Timestamp, onto: NaiveDate) {
        let live = self.live_session();
        let target = self.session(onto);
        let mirrored: Vec<Bar> = self
            .minute
            .iter()
            .filter(|bar| (from..live.close).contains(&bar.start))
            .map(|bar| Bar {
                start: target.open + (bar.start - live.open),
                ..bar.clone()
            })
            .collect();
        self.minute.extend(mirrored);
    }
}

fn instrument(symbol: &str, etf: bool, active: bool) -> Instrument {
    Instrument {
        symbol: Symbol::new(symbol),
        name: format!("{symbol} {}", if etf { "ETF" } else { "Corp" }),
        exchange: "NYSE".to_owned(),
        asset_class: if etf {
            AssetClass::Etf
        } else {
            AssetClass::Stock
        },
        active,
        shortable: true,
        first_session: None,
        last_session: None,
    }
}

fn bar(symbol: &str, start: Timestamp, [open, high, low, close]: [Decimal; 4], volume: u64) -> Bar {
    Bar {
        symbol: Symbol::new(symbol),
        start,
        open,
        high,
        low,
        close,
        volume,
        feed: Feed::Sip,
    }
}

fn live_tape(symbol: &str, index: usize, session: &Session) -> Vec<Bar> {
    let cents = |value: i64| Decimal::new(value, 2);
    let mut price = 10_000 + 5_000 * i64::try_from(index).expect("few symbols");
    (0..session.minutes())
        .map(|minute| {
            let open = price;
            let close = if (minute + i64::try_from(index).expect("few symbols")) % 3 == 0 {
                open - 7
            } else {
                open + 4
            };
            price = close;
            bar(
                symbol,
                session.open + TimeDelta::minutes(minute),
                [
                    cents(open),
                    cents(open.max(close) + 2),
                    cents(open.min(close) - 2),
                    cents(close),
                ],
                10_000,
            )
        })
        .collect()
}

fn companies(facts: &[FundamentalFact]) -> Vec<Company> {
    let mut companies: Vec<Company> = Vec::new();
    for fact in facts {
        let index = match companies.iter().position(|company| company.cik == fact.cik) {
            Some(index) => index,
            None => {
                companies.push(Company {
                    cik: fact.cik,
                    ticker: fact.symbol.clone(),
                    facts: json!({"cik": fact.cik, "entityName": fact.symbol, "facts": {}}),
                    submissions: submissions(fact.cik, "", &[]),
                });
                companies.len() - 1
            }
        };
        let mut observation = Map::new();
        if let Some(start) = fact.period_start {
            observation.insert("start".to_owned(), json!(start));
        }
        observation.insert("end".to_owned(), json!(fact.period_end));
        observation.insert(
            "val".to_owned(),
            serde_json::from_str(&fact.value.to_string()).expect("fact values are numbers"),
        );
        observation.insert("fy".to_owned(), json!(fact.fiscal_year));
        observation.insert("fp".to_owned(), json!(fact.fiscal_period));
        observation.insert("form".to_owned(), json!(fact.form));
        observation.insert("filed".to_owned(), json!(fact.filed));
        observation.insert("accn".to_owned(), json!(fact.accession));
        if let Some(frame) = &fact.frame {
            observation.insert("frame".to_owned(), json!(frame));
        }
        let list = &mut companies[index].facts["facts"][&fact.taxonomy][&fact.concept]["units"]
            [&fact.unit];
        if list.is_null() {
            *list = json!([]);
        }
        list.as_array_mut()
            .expect("unit lists are arrays")
            .push(Value::Object(observation));
    }
    companies
}
