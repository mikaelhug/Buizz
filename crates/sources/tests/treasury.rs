use axum::Router;
use axum::routing::get;
use buizz_sources::treasury::{TreasuryClient, TreasuryConfig};
use chrono::NaiveDate;
use rust_decimal::Decimal;

const FIXTURE: &str = include_str!("../fixtures/treasury/par_yield_2025.csv");

#[tokio::test]
async fn par_yield_csv_with_gaps_yields_ascending_three_month_rates() {
    let router = Router::new().route(
        "/resource-center/data-chart-center/interest-rates/daily-treasury-rates.csv/{year}/all",
        get(|| async { FIXTURE }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = TreasuryClient::new(TreasuryConfig { base_url }).unwrap();

    let rates = client.three_month_yields(2025).await.unwrap();

    let observed: Vec<(NaiveDate, Decimal)> =
        rates.iter().map(|rate| (rate.date, rate.percent)).collect();
    let day = |text: &str| text.parse::<NaiveDate>().unwrap();
    let percent = |text: &str| text.parse::<Decimal>().unwrap();
    assert_eq!(
        observed,
        [
            (day("2025-03-03"), percent("4.35")),
            (day("2025-03-04"), percent("4.34")),
            (day("2025-03-06"), percent("4.33")),
            (day("2025-03-07"), percent("4.33")),
        ]
    );
}
