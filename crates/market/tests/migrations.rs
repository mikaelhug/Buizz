use buizz_core::market::{Symbol, Timeframe};
use buizz_core::time::Timestamp;
use buizz_market::{FILE_NAME, MarketStore};

const VERSION_1: &str = include_str!("fixtures/schema_v1.sql");

#[tokio::test]
async fn a_version_1_store_opens_in_place_with_its_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    let old = duckdb::Connection::open(&path).unwrap();
    old.execute_batch(VERSION_1).unwrap();
    old.execute_batch(
        "INSERT INTO instrument VALUES ('AAPL', 'Apple Inc.', 'NASDAQ', 'stock', true, true, NULL, NULL, 1);
         INSERT INTO calendar VALUES ('2024-07-05', '2024-07-05 13:30:00', '2024-07-05 20:00:00', 1);
         INSERT INTO bar_1d VALUES ('AAPL', '2024-07-05', 221, 226.45, 221.65, 226.34, 60412408, 'alpaca', 1);",
    )
    .unwrap();
    drop(old);

    let market = MarketStore::open(&path, 1).await.unwrap();
    let instruments = market.instruments().await.unwrap();
    assert_eq!(instruments.len(), 1);
    assert_eq!(instruments[0].name, "Apple Inc.");
    let bars = market
        .bars_between(
            Timeframe::Day,
            vec![Symbol::new("AAPL")],
            Timestamp::MIN,
            Timestamp::MAX,
        )
        .await
        .unwrap();
    assert_eq!(bars.len(), 1);
    assert_eq!(bars[0].close.to_string(), "226.340000");
}
