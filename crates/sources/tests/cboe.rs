use axum::Router;
use axum::extract::Path;
use axum::routing::get;
use buizz_core::market::{MacroObservation, MacroSeries};
use buizz_sources::cboe::{CboeClient, CboeConfig};

const VIX: &str = include_str!("../fixtures/cboe/VIX_History.csv");
const VIX3M: &str = include_str!("../fixtures/cboe/VIX3M_History.csv");

#[tokio::test]
async fn vix_starts_at_its_2003_methodology_and_every_close_is_published_on_its_date() {
    let router = Router::new().route(
        "/api/global/us_indices/daily_prices/{file}",
        get(|Path(file): Path<String>| async move {
            match file.as_str() {
                "VIX_History.csv" => VIX,
                "VIX3M_History.csv" => VIX3M,
                other => panic!("unexpected {other}"),
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = CboeClient::new(CboeConfig { base_url }).unwrap();

    let close = |series, date: &str, value: &str| MacroObservation {
        series,
        date: date.parse().unwrap(),
        published: date.parse().unwrap(),
        value: Some(value.parse().unwrap()),
    };
    assert_eq!(
        client.daily_closes(MacroSeries::Vix).await.unwrap(),
        [
            close(MacroSeries::Vix, "2003-09-22", "19.65"),
            close(MacroSeries::Vix, "2003-09-23", "19.47"),
            close(MacroSeries::Vix, "2026-11-10", "19.46"),
            close(MacroSeries::Vix, "2026-11-11", "17.49"),
        ]
    );
    assert_eq!(
        client.daily_closes(MacroSeries::Vix3m).await.unwrap(),
        [
            close(MacroSeries::Vix3m, "2009-09-18", "26.54"),
            close(MacroSeries::Vix3m, "2009-09-21", "26.23"),
            close(MacroSeries::Vix3m, "2026-11-10", "20.64"),
            close(MacroSeries::Vix3m, "2026-11-11", "19.24"),
        ]
    );
}
