use axum::Router;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use buizz_core::market::{
    Bar, Calendar, CorporateAction, CorporateActionKind, Feed, Session, Symbol, split_rates,
};
use buizz_core::time::Timestamp;
use buizz_sources::http::SourceError;
use buizz_sources::tiingo::{
    OverlapVerdict, TiingoClient, TiingoConfig, normalize, validate_overlap,
};
use chrono::{Datelike, NaiveDate, NaiveTime, Weekday};
use rust_decimal::Decimal;

const FIXTURE: &str = include_str!("../fixtures/tiingo/aapl_daily.json");

async fn prices(Path(symbol): Path<String>, headers: HeaderMap) -> impl IntoResponse {
    let authorized = headers
        .get("authorization")
        .is_some_and(|value| value == "Token test-key");
    match (symbol.as_str(), authorized) {
        (_, false) => (StatusCode::UNAUTHORIZED, String::new()),
        ("AAPL", true) => (StatusCode::OK, FIXTURE.to_owned()),
        ("SLOW", true) => (StatusCode::TOO_MANY_REQUESTS, String::new()),
        _ => (StatusCode::NOT_FOUND, "not found".to_owned()),
    }
}

async fn client() -> TiingoClient {
    let router = Router::new().route("/tiingo/daily/{symbol}/prices", get(prices));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut config = TiingoConfig::new("test-key".to_owned());
    config.base_url = base_url;
    TiingoClient::new(config).unwrap()
}

fn date(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

fn weekday_calendar(from: NaiveDate, to: NaiveDate) -> Calendar {
    let open = NaiveTime::from_hms_opt(9, 30, 0).unwrap();
    let close = NaiveTime::from_hms_opt(16, 0, 0).unwrap();
    let sessions = from
        .iter_days()
        .take_while(|day| *day <= to)
        .filter(|day| !matches!(day.weekday(), Weekday::Sat | Weekday::Sun))
        .map(|day| Session {
            date: day,
            open: Timestamp::at_exchange_time(day, open).unwrap(),
            close: Timestamp::at_exchange_time(day, close).unwrap(),
        })
        .collect();
    Calendar::new(sessions)
}

async fn aapl() -> (Vec<Bar>, Vec<CorporateAction>) {
    let symbol = Symbol::new("aapl");
    let rows = client()
        .await
        .daily(&symbol, date("2020-08-03"), date("2020-09-04"))
        .await
        .unwrap();
    let calendar = weekday_calendar(date("2020-08-03"), date("2020-09-04"));
    normalize(&symbol, &rows, &calendar)
}

#[tokio::test]
async fn daily_rows_normalize_into_raw_bars_and_corporate_actions() {
    let (bars, actions) = aapl().await;

    assert_eq!(bars.len(), 25);
    let before_split = &bars[bars.len() - 8];
    assert_eq!(before_split.close, "499.23".parse::<Decimal>().unwrap());
    assert_eq!(before_split.feed, Feed::Sip);
    assert_eq!(bars[0].start.exchange_date(), date("2020-08-03"));
    assert_eq!(
        actions
            .iter()
            .map(|action| action.ex_date)
            .collect::<Vec<_>>(),
        [date("2020-08-07"), date("2020-08-31")]
    );
    assert_eq!(
        actions[0].kind,
        CorporateActionKind::CashDividend {
            rate: "0.82".parse().unwrap()
        }
    );
    assert_eq!(
        actions[1].kind,
        CorporateActionKind::Split {
            old_rate: Decimal::ONE,
            new_rate: Decimal::from(4)
        }
    );
}

#[test]
fn a_split_factor_off_by_float_noise_is_the_ratio_it_stands_for() {
    let rates = |factor: &str| split_rates(factor.parse().unwrap());
    assert_eq!(rates("7.000007"), (Decimal::ONE, Decimal::from(7)));
    assert_eq!(rates("1.5"), (Decimal::TWO, Decimal::from(3)));
    assert_eq!(rates("0.05"), (Decimal::from(20), Decimal::ONE));
    assert_eq!(rates("0.142857"), (Decimal::from(7), Decimal::ONE));
}

#[tokio::test]
async fn overlap_gate_trusts_a_matching_tail_and_rejects_wrong_scale_or_split_basis() {
    let (tiingo, _) = aapl().await;

    assert_eq!(
        validate_overlap(&tiingo, &tiingo),
        OverlapVerdict::Trusted { overlap: 25 }
    );

    let wrong_scale: Vec<Bar> = tiingo
        .iter()
        .map(|bar| Bar {
            close: bar.close * Decimal::from(1000),
            ..bar.clone()
        })
        .collect();
    assert!(matches!(
        validate_overlap(&tiingo, &wrong_scale),
        OverlapVerdict::Rejected { .. }
    ));

    let split_adjusted: Vec<Bar> = tiingo
        .iter()
        .map(|bar| {
            let pre_split = bar.start.exchange_date() < date("2020-08-31");
            Bar {
                close: if pre_split {
                    bar.close / Decimal::from(4)
                } else {
                    bar.close
                },
                ..bar.clone()
            }
        })
        .collect();
    assert!(matches!(
        validate_overlap(&tiingo, &split_adjusted),
        OverlapVerdict::Rejected { .. }
    ));

    assert_eq!(
        validate_overlap(&tiingo, &tiingo[..10]),
        OverlapVerdict::Insufficient { overlap: 10 }
    );
}

#[tokio::test]
async fn throttling_and_unknown_symbols_surface_as_distinct_errors() {
    let (from, to) = (date("2020-08-03"), date("2020-09-04"));

    assert!(matches!(
        client().await.daily(&Symbol::new("slow"), from, to).await,
        Err(SourceError::RateLimited { .. })
    ));
    assert!(matches!(
        client().await.daily(&Symbol::new("nope"), from, to).await,
        Err(SourceError::Status { status: 404, .. })
    ));
}
