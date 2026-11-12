use buizz_core::market::{
    AssetClass, Bar, DataSource, Feed, Instrument, Session, Symbol, Timeframe,
};
use buizz_core::run::UniverseSpec;
use buizz_core::time::Timestamp;
use buizz_market::{FILE_NAME, MarketStore};
use chrono::{Datelike, NaiveDate, NaiveTime, TimeDelta};
use rust_decimal::Decimal;

fn session(date: NaiveDate) -> Session {
    let open =
        Timestamp::at_exchange_time(date, NaiveTime::from_hms_opt(9, 30, 0).unwrap()).unwrap();
    Session {
        date,
        open,
        close: open + TimeDelta::minutes(390),
    }
}

fn instrument(symbol: &str, last_session: Option<NaiveDate>) -> Instrument {
    Instrument {
        symbol: Symbol::new(symbol),
        name: symbol.to_owned(),
        exchange: "NYSE".to_owned(),
        asset_class: AssetClass::Stock,
        active: last_session.is_none(),
        shortable: true,
        first_session: None,
        last_session,
    }
}

fn bar(symbol: &str, session: &Session, volume: u64) -> Bar {
    Bar {
        symbol: Symbol::new(symbol),
        start: session.open,
        open: Decimal::ONE,
        high: Decimal::ONE,
        low: Decimal::ONE,
        close: Decimal::ONE,
        volume,
        feed: Feed::Sip,
    }
}

#[tokio::test]
async fn top_dollar_volume_ranks_by_average_and_fills_past_delisted_symbols() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    let sessions: Vec<Session> = ["2024-07-01", "2024-07-02", "2024-07-03"]
        .iter()
        .map(|day| session(day.parse().unwrap()))
        .collect();
    store
        .upsert_sessions(DataSource::Fixture, sessions.clone())
        .await
        .unwrap();
    store
        .upsert_instruments(
            DataSource::Fixture,
            vec![
                instrument("GONE", Some(sessions[1].date)),
                instrument("STEADY", None),
                instrument("NEW", None),
            ],
        )
        .await
        .unwrap();
    let bars = vec![
        bar("GONE", &sessions[0], 1_000),
        bar("GONE", &sessions[1], 1_000),
        bar("STEADY", &sessions[0], 100),
        bar("STEADY", &sessions[1], 100),
        bar("NEW", &sessions[1], 150),
    ];
    store
        .upsert_bars(DataSource::Fixture, Timeframe::Day, bars)
        .await
        .unwrap();

    let top = |count| UniverseSpec::TopDollarVolume {
        top_dollar_volume: count,
        asset_class: None,
    };
    let on = sessions[2].date;
    assert_eq!(
        store.universe(&top(1), on, 20).await.unwrap(),
        vec![Symbol::new("NEW")]
    );
    assert_eq!(
        store.universe(&top(2), on, 20).await.unwrap(),
        vec![Symbol::new("NEW"), Symbol::new("STEADY")]
    );
}

#[tokio::test]
async fn bars_of_a_wide_universe_come_back_in_time_order_on_a_limited_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    store.limit("1GB".to_owned(), 2).await.unwrap();
    let sessions: Vec<Session> = NaiveDate::from_ymd_opt(2015, 1, 2)
        .unwrap()
        .iter_days()
        .take_while(|day| day.year() < 2017)
        .filter(|day| day.weekday().number_from_monday() <= 5)
        .map(session)
        .collect();
    store
        .upsert_sessions(DataSource::Fixture, sessions.clone())
        .await
        .unwrap();
    let symbols: Vec<Symbol> = (0..500)
        .map(|index| Symbol::new(&format!("S{index:03}")))
        .collect();
    store
        .upsert_bars(
            DataSource::Fixture,
            Timeframe::Day,
            symbols
                .iter()
                .flat_map(|symbol| sessions.iter().map(|day| bar(symbol.as_str(), day, 1)))
                .collect(),
        )
        .await
        .unwrap();

    let bars = store
        .bars_between(
            Timeframe::Day,
            symbols,
            sessions[270].open,
            sessions[520].close,
        )
        .await
        .unwrap();

    assert!(bars.is_sorted_by_key(|bar| (bar.start, bar.symbol.clone())));
}
