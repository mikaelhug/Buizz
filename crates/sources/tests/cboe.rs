use axum::Router;
use axum::extract::Path;
use axum::routing::get;
use buizz_core::market::{MacroObservation, MacroSeries};
use buizz_sources::cboe::{CboeClient, CboeConfig};

const VIX: &str = include_str!("../fixtures/cboe/VIX_History.csv");
const VIX3M: &str = include_str!("../fixtures/cboe/VIX3M_History.csv");
const SKEW: &str = include_str!("../fixtures/cboe/SKEW_History.csv");
const TOTAL_PUT_CALL: &str = include_str!("../fixtures/cboe/totalpc.csv");
const DAILY_OPTIONS: &str = include_str!("../fixtures/cboe/2019-10-07_daily_options.json");

#[tokio::test]
async fn each_index_starts_at_its_launch_and_every_close_is_published_on_its_date() {
    let router = Router::new()
        .route(
            "/api/global/us_indices/daily_prices/{file}",
            get(|Path(file): Path<String>| async move {
                match file.as_str() {
                    "VIX_History.csv" => VIX,
                    "VIX3M_History.csv" => VIX3M,
                    "SKEW_History.csv" => SKEW,
                    other => panic!("unexpected {other}"),
                }
            }),
        )
        .route(
            "/resources/options/volume_and_call_put_ratios/totalpc.csv",
            get(|| async { TOTAL_PUT_CALL }),
        )
        .route(
            "/data/us/options/market_statistics/daily/{file}",
            get(|Path(file): Path<String>| async move {
                match file.as_str() {
                    "2019-10-07_daily_options" => Ok(DAILY_OPTIONS),
                    _ => Err(axum::http::StatusCode::FORBIDDEN),
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
    assert_eq!(
        client.daily_closes(MacroSeries::Skew).await.unwrap(),
        [
            close(MacroSeries::Skew, "2011-02-23", "124.53"),
            close(MacroSeries::Skew, "2011-02-24", "124.68"),
            close(MacroSeries::Skew, "2026-11-12", "150.16"),
        ]
    );
    assert_eq!(
        client
            .archived_put_call(MacroSeries::PutCallTotal)
            .await
            .unwrap(),
        [
            close(MacroSeries::PutCallTotal, "2006-11-01", "0.91"),
            close(MacroSeries::PutCallTotal, "2019-10-04", "1.05"),
        ]
    );
    assert_eq!(
        client
            .daily_put_call("2019-10-07".parse().unwrap())
            .await
            .unwrap(),
        Some(vec![
            close(MacroSeries::PutCallTotal, "2019-10-07", "1.05"),
            close(MacroSeries::PutCallEquity, "2019-10-07", "0.70"),
            close(MacroSeries::PutCallIndex, "2019-10-07", "1.05"),
        ])
    );
    assert_eq!(
        client
            .daily_put_call("2019-10-08".parse().unwrap())
            .await
            .unwrap(),
        None
    );
}
