use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use buizz_core::market::{MacroObservation, MacroSeries};
use buizz_sources::fred::{FredClient, FredConfig};
use chrono::{NaiveDate, TimeDelta};
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("../fixtures/fred/cpiaucns_vintages.json");
const ALFRED_MAX_VINTAGES: usize = 2000;
const FOREVER: &str = "9999-12-31";

#[derive(Clone)]
struct Row {
    date: NaiveDate,
    start: NaiveDate,
    end: NaiveDate,
    value: String,
}

#[derive(Clone)]
struct Alfred {
    rows: Arc<Vec<Row>>,
    page: usize,
    requests: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

fn day(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

fn param(params: &HashMap<String, String>, name: &str, default: &str) -> NaiveDate {
    day(params.get(name).map_or(default, String::as_str))
}

async fn vintage_dates(
    State(alfred): State<Alfred>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let since = param(&params, "realtime_start", "1776-07-04");
    let offset: usize = params["offset"].parse().unwrap();
    let limit: usize = params["limit"].parse().unwrap();
    let dates: BTreeSet<NaiveDate> = alfred
        .rows
        .iter()
        .map(|row| row.start)
        .filter(|start| *start >= since)
        .collect();
    let page: Vec<String> = dates
        .iter()
        .skip(offset)
        .take(limit)
        .map(ToString::to_string)
        .collect();
    Json(json!({"count": dates.len(), "offset": offset, "vintage_dates": page}))
}

async fn observations(
    State(alfred): State<Alfred>,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    alfred.requests.lock().unwrap().push(params.clone());
    let start = param(&params, "realtime_start", "1776-07-04");
    let end = param(&params, "realtime_end", FOREVER);
    let vintages: BTreeSet<NaiveDate> = alfred
        .rows
        .iter()
        .map(|row| row.start)
        .filter(|vintage| (start..=end).contains(vintage))
        .collect();
    if vintages.len() > ALFRED_MAX_VINTAGES {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error_code": 400, "error_message": "too many vintage dates"})),
        );
    }
    let mut visible: Vec<Row> = alfred
        .rows
        .iter()
        .filter(|row| row.start <= end && row.end >= start)
        .map(|row| Row {
            start: row.start.max(start),
            end: row.end.min(end),
            ..row.clone()
        })
        .collect();
    visible.sort_by_key(|row| (row.date, row.start));
    let offset: usize = params["offset"].parse().unwrap();
    let limit: usize = params["limit"].parse().unwrap();
    let page: Vec<Value> = visible
        .iter()
        .skip(offset)
        .take(limit.min(alfred.page))
        .map(|row| {
            json!({
                "realtime_start": row.start.to_string(),
                "realtime_end": row.end.to_string(),
                "date": row.date.to_string(),
                "value": row.value,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({"count": visible.len(), "offset": offset, "observations": page})),
    )
}

async fn serve(rows: Vec<Row>, page: usize) -> (FredClient, Alfred) {
    let alfred = Alfred {
        rows: Arc::new(rows),
        page,
        requests: Arc::default(),
    };
    let router = Router::new()
        .route("/fred/series/vintagedates", get(vintage_dates))
        .route("/fred/series/observations", get(observations))
        .with_state(alfred.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = FredClient::new(FredConfig {
        api_key: "fred-key".to_owned(),
        base_url,
    })
    .unwrap();
    (client, alfred)
}

fn observation(
    series: MacroSeries,
    date: NaiveDate,
    published: NaiveDate,
    value: Option<&str>,
) -> MacroObservation {
    MacroObservation {
        series,
        date,
        published,
        value: value.map(|text| text.parse().unwrap()),
    }
}

#[tokio::test]
async fn every_vintage_is_fetched_across_pages_and_a_withdrawn_value_stays_missing() {
    let recorded: Value = serde_json::from_str(FIXTURE).unwrap();
    let rows = recorded["observations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| Row {
            date: day(row["date"].as_str().unwrap()),
            start: day(row["realtime_start"].as_str().unwrap()),
            end: day(row["realtime_end"].as_str().unwrap()),
            value: row["value"].as_str().unwrap().to_owned(),
        })
        .collect();
    let (client, alfred) = serve(rows, 2).await;

    let vintages = client.vintages(MacroSeries::Cpiaucns).await.unwrap();

    let cpi = |date: &str, published: &str, value| {
        observation(MacroSeries::Cpiaucns, day(date), day(published), value)
    };
    assert_eq!(
        vintages,
        [
            cpi("2024-08-01", "2024-09-11", Some("314.796")),
            cpi("2024-09-01", "2024-10-10", Some("315.301")),
            cpi("2024-10-01", "2024-11-13", Some("315.664")),
            cpi("2025-10-01", "2025-12-18", None),
            cpi("2025-11-01", "2025-12-18", Some("324.122")),
        ]
    );
    let requests = alfred.requests.lock().unwrap();
    let offsets: Vec<&str> = requests
        .iter()
        .map(|query| query["offset"].as_str())
        .collect();
    assert_eq!(offsets, ["0", "2", "4"]);
    for query in requests.iter() {
        assert_eq!(query["series_id"], "CPIAUCNS");
        assert_eq!(query["api_key"], "fred-key");
        assert_eq!(query["realtime_end"], FOREVER);
    }
}

#[tokio::test]
async fn a_daily_series_with_more_vintages_than_one_request_allows_keeps_each_first_publication() {
    let first = day("2005-06-28");
    let vintages: Vec<NaiveDate> = (0..2500).map(|index| first + TimeDelta::days(index)).collect();
    let revised = vintages[2200];
    let mut rows: Vec<Row> = vintages
        .windows(2)
        .map(|pair| Row {
            date: pair[0],
            start: pair[1],
            end: day(FOREVER),
            value: "4.00".to_owned(),
        })
        .collect();
    rows[10].end = revised.pred_opt().unwrap();
    rows.push(Row {
        date: rows[10].date,
        start: revised,
        end: day(FOREVER),
        value: "4.25".to_owned(),
    });
    let (client, alfred) = serve(rows.clone(), 100_000).await;

    let fetched = client.vintages(MacroSeries::Dgs10).await.unwrap();

    let mut expected: Vec<MacroObservation> = rows
        .iter()
        .map(|row| observation(MacroSeries::Dgs10, row.date, row.start, Some(&row.value)))
        .collect();
    expected.sort_by_key(|observation| (observation.date, observation.published));
    assert_eq!(fetched, expected);
    assert!(alfred.requests.lock().unwrap().len() > 1);
}
