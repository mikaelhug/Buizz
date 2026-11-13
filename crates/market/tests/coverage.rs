use buizz_core::market::{
    AssetClass, Bar, CorporateAction, CorporateActionKind, DataSource, Feed, IndexHistory,
    IndexMember, Instrument, Session, StockIndex, Symbol, Timeframe,
};
use buizz_core::ops::CoverageGap;
use buizz_core::time::Timestamp;
use buizz_market::{ActionWindow, FILE_NAME, MarketStore};
use chrono::{NaiveDate, NaiveTime, TimeDelta};
use rust_decimal::Decimal;

fn session(date: NaiveDate, open: Timestamp) -> Session {
    Session {
        date,
        open,
        close: open + TimeDelta::minutes(390),
    }
}

fn bar(symbol: &Symbol, start: Timestamp, feed: Feed) -> Bar {
    Bar {
        symbol: symbol.clone(),
        start,
        open: Decimal::ONE,
        high: Decimal::ONE,
        low: Decimal::ONE,
        close: Decimal::ONE,
        volume: 100,
        feed,
    }
}

fn gap(symbol: &Symbol, timeframe: Timeframe, from: NaiveDate, to: NaiveDate) -> CoverageGap {
    CoverageGap {
        symbol: symbol.clone(),
        timeframe,
        from,
        to,
    }
}

#[tokio::test]
async fn gaps_skip_days_before_listing_and_count_only_settled_sip_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    let now = Timestamp::now();
    let today = now.exchange_date();
    let noon = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let days: Vec<NaiveDate> = (1..=4)
        .rev()
        .map(|back| today - TimeDelta::days(7 * back))
        .chain([today])
        .collect();
    let sessions: Vec<Session> = days
        .iter()
        .map(|day| {
            if *day == today {
                session(today, now)
            } else {
                session(*day, Timestamp::at_exchange_time(*day, noon).unwrap())
            }
        })
        .collect();
    store
        .upsert_sessions(DataSource::Fixture, sessions.clone())
        .await
        .unwrap();
    let ipo = Symbol::new("IPO");
    let daily = sessions[2..]
        .iter()
        .map(|session| bar(&ipo, session.open, Feed::Sip))
        .collect();
    store
        .upsert_bars(DataSource::Fixture, Timeframe::Day, daily)
        .await
        .unwrap();
    let minutes = vec![
        bar(&ipo, sessions[2].open, Feed::Iex),
        bar(&ipo, sessions[3].open, Feed::Sip),
        bar(&ipo, sessions[4].open, Feed::Sip),
    ];
    store
        .upsert_bars(DataSource::Fixture, Timeframe::Minute, minutes)
        .await
        .unwrap();

    let daily_gaps = store
        .coverage_gaps(vec![ipo.clone()], Timeframe::Day, days[0], today)
        .await
        .unwrap();
    assert_eq!(daily_gaps, vec![]);

    let minute_gaps = store
        .coverage_gaps(vec![ipo.clone()], Timeframe::Minute, days[0], today)
        .await
        .unwrap();
    assert_eq!(
        minute_gaps,
        vec![
            gap(&ipo, Timeframe::Minute, days[2], days[2]),
            gap(&ipo, Timeframe::Minute, today, today),
        ]
    );
}

#[tokio::test]
async fn history_that_starts_where_the_store_starts_is_a_gap_before_it_not_a_listing() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    let noon = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let days: Vec<NaiveDate> = ["2015-12-30", "2015-12-31", "2016-01-04", "2016-01-05"]
        .iter()
        .map(|day| day.parse().unwrap())
        .collect();
    let sessions: Vec<Session> = days
        .iter()
        .map(|day| session(*day, Timestamp::at_exchange_time(*day, noon).unwrap()))
        .collect();
    store
        .upsert_sessions(DataSource::Fixture, sessions.clone())
        .await
        .unwrap();
    let old = Symbol::new("OLD");
    let ipo = Symbol::new("IPO");
    let daily = sessions[2..]
        .iter()
        .map(|session| bar(&old, session.open, Feed::Sip))
        .chain([bar(&ipo, sessions[3].open, Feed::Sip)])
        .collect();
    store
        .upsert_bars(DataSource::Fixture, Timeframe::Day, daily)
        .await
        .unwrap();

    let gaps = store
        .coverage_gaps(vec![old.clone(), ipo], Timeframe::Day, days[0], days[3])
        .await
        .unwrap();
    assert_eq!(gaps, vec![gap(&old, Timeframe::Day, days[0], days[1])]);
}

#[tokio::test]
async fn a_member_is_priced_through_its_rename_and_not_missed_after_its_last_trade() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    let noon = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let day = |text: &str| -> NaiveDate { text.parse().unwrap() };
    let days: Vec<NaiveDate> = (0..10)
        .map(|offset| day("2022-12-12") + TimeDelta::days(offset))
        .filter(|date| chrono::Datelike::weekday(date).number_from_monday() <= 5)
        .collect();
    let sessions: Vec<Session> = days
        .iter()
        .map(|date| session(*date, Timestamp::at_exchange_time(*date, noon).unwrap()))
        .collect();
    store
        .upsert_sessions(DataSource::Fixture, sessions.clone())
        .await
        .unwrap();
    let (renamed, acquired) = (Symbol::new("FBIN"), Symbol::new("SPLS"));
    let acquired_listing = Symbol::ended(&acquired, day("2022-12-14"));
    let bars = sessions
        .iter()
        .map(|session| bar(&renamed, session.open, Feed::Sip))
        .chain(
            sessions
                .iter()
                .filter(|session| session.date <= day("2022-12-14"))
                .map(|session| bar(&acquired_listing, session.open, Feed::Sip)),
        )
        .collect();
    store
        .upsert_bars(DataSource::Fixture, Timeframe::Day, bars)
        .await
        .unwrap();
    store
        .upsert_instruments(
            DataSource::Fixture,
            vec![Instrument {
                symbol: acquired_listing,
                name: String::new(),
                exchange: String::new(),
                asset_class: AssetClass::Stock,
                active: false,
                shortable: false,
                first_session: None,
                last_session: Some(day("2022-12-14")),
            }],
        )
        .await
        .unwrap();
    store
        .replace_name_changes(
            DataSource::Fixture,
            vec![CorporateAction {
                symbol: Symbol::new("FBHS"),
                ex_date: day("2022-12-15"),
                kind: CorporateActionKind::NameChange {
                    new_symbol: renamed.clone(),
                },
            }],
            ActionWindow {
                symbols: Vec::new(),
                from: days[0],
                through: days[days.len() - 1],
                covered_to: None,
            },
        )
        .await
        .unwrap();
    let member = |symbol: &str| IndexMember {
        index: StockIndex::Sp500,
        symbol: Symbol::new(symbol),
        start: Some(day("2016-06-22")),
        end: Some(day("2022-12-19")),
    };
    store
        .replace_index_history(
            DataSource::Fixture,
            IndexHistory {
                index: StockIndex::Sp500,
                members: vec![member("FBHS"), member("SPLS")],
                changes: 2,
                reliable_from: None,
            },
            day("2022-12-23"),
        )
        .await
        .unwrap();

    let gaps = store
        .unpriced_members(StockIndex::Sp500, days[0], days[days.len() - 1])
        .await
        .unwrap();
    assert_eq!(
        gaps,
        vec![],
        "FBHS trades as FBIN from its rename to its removal, and SPLS stopped trading before its removal"
    );
}
