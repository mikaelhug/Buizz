use buizz_core::market::{Bar, DataSource, Feed, Session, Symbol, Timeframe};
use buizz_core::ops::CoverageGap;
use buizz_core::time::Timestamp;
use buizz_market::{FILE_NAME, MarketStore};
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
