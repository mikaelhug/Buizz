use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use buizz_core::market::{MacroObservation, MacroSeries};
use buizz_sources::fred::{FredClient, FredConfig};
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("../fixtures/fred/cpiaucns_vintages.json");
const PAGE: usize = 2;

type Requests = Arc<Mutex<Vec<HashMap<String, String>>>>;

async fn observations(
    State(requests): State<Requests>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let recorded: Value = serde_json::from_str(FIXTURE).unwrap();
    let offset: usize = params["offset"].parse().unwrap();
    requests.lock().unwrap().push(params);
    let all = recorded["observations"].as_array().unwrap();
    let page: Vec<Value> = all.iter().skip(offset).take(PAGE).cloned().collect();
    Json(json!({"count": all.len(), "offset": offset, "observations": page}))
}

#[tokio::test]
async fn every_vintage_is_fetched_across_pages_and_a_withdrawn_value_stays_missing() {
    let requests = Requests::default();
    let router = Router::new()
        .route("/fred/series/observations", get(observations))
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = FredClient::new(FredConfig {
        api_key: "fred-key".to_owned(),
        base_url,
    })
    .unwrap();

    let vintages = client.vintages(MacroSeries::Cpiaucns).await.unwrap();

    let day = |text: &str| text.parse().unwrap();
    let observation = |date: &str, published: &str, value: Option<&str>| MacroObservation {
        series: MacroSeries::Cpiaucns,
        date: day(date),
        published: day(published),
        value: value.map(|text| text.parse().unwrap()),
    };
    assert_eq!(
        vintages,
        [
            observation("2024-08-01", "2024-09-11", Some("314.796")),
            observation("2024-09-01", "2024-10-10", Some("315.301")),
            observation("2024-10-01", "2024-11-13", Some("315.664")),
            observation("2025-10-01", "2025-12-18", None),
            observation("2025-11-01", "2025-12-18", Some("324.122")),
        ]
    );
    let requests = requests.lock().unwrap();
    let offsets: Vec<&str> = requests
        .iter()
        .map(|query| query["offset"].as_str())
        .collect();
    assert_eq!(offsets, ["0", "2", "4"]);
    for query in requests.iter() {
        assert_eq!(query["series_id"], "CPIAUCNS");
        assert_eq!(query["api_key"], "fred-key");
        assert_eq!(query["realtime_start"], "1776-07-04");
        assert_eq!(query["realtime_end"], "9999-12-31");
    }
}
