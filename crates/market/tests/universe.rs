use buizz_core::market::{
    AssetClass, Bar, DataSource, DelistingCheck, DelistingOutcome, Feed, Instrument, SecFiling,
    Session, Symbol, Timeframe,
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

#[tokio::test]
async fn an_exchange_delisting_notice_belongs_to_the_listed_company_not_the_exchange() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    let filed: NaiveDate = "2016-04-08".parse().unwrap();
    let notice = |cik, accession: &str, form: &str| SecFiling {
        cik,
        accession: accession.to_owned(),
        form: form.to_owned(),
        filed,
    };
    let by_exchange = "0000876661-16-000101";
    let by_company = "0000990001-16-000003";
    store
        .append_delisting_notices(
            DataSource::Fixture,
            vec![
                notice(876_661, by_exchange, "25-NSE"),
                notice(750_556, by_exchange, "25-NSE"),
                notice(990_001, by_company, "25"),
            ],
            (filed, filed),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .unresolved_delistings(filed)
            .await
            .unwrap()
            .iter()
            .map(|notice| (notice.cik, notice.accession.as_str()))
            .collect::<Vec<_>>(),
        vec![(750_556, by_exchange), (990_001, by_company)],
        "the exchange that filed the 25-NSE is not the company it delists"
    );

    let listing = Symbol::ended(&Symbol::new("STI"), "2016-03-31".parse().unwrap());
    store
        .record_delistings(
            DataSource::Fixture,
            vec![DelistingCheck {
                accession: by_exchange.to_owned(),
                ticker: Some(Symbol::new("STI")),
                listing: Some(listing.clone()),
                outcome: DelistingOutcome::Stored,
            }],
        )
        .await
        .unwrap();
    assert_eq!(
        store.delisted_companies().await.unwrap(),
        vec![(listing, 750_556)]
    );
}
