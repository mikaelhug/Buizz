use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Router, serve};
use buizz_core::market::{
    AssetClass, Calendar, CorporateActionKind, Feed, Session, Symbol, Timeframe,
};
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::{AlpacaClient, AlpacaConfig, BarsRequest, Tickers};
use buizz_sources::http::SourceError;
use chrono::{NaiveDate, NaiveTime};
use rust_decimal::Decimal;
use tokio::net::TcpListener;

type Params = HashMap<String, String>;
type Seen = Arc<Mutex<Vec<(String, Params)>>>;

fn json(body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn record(seen: &Seen, route: &str, headers: &HeaderMap, params: &Params) {
    assert_eq!(headers["apca-api-key-id"], "key");
    assert_eq!(headers["apca-api-secret-key"], "secret");
    seen.lock()
        .unwrap()
        .push((route.to_owned(), params.clone()));
}

async fn bars(
    State(seen): State<Seen>,
    headers: HeaderMap,
    Query(params): Query<Params>,
) -> Response {
    record(&seen, "bars", &headers, &params);
    if params["symbols"] == "LIMITED" {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "120")],
        )
            .into_response();
    }
    match (params["timeframe"].as_str(), params.get("page_token")) {
        ("1Day", None) => json(include_str!("../fixtures/alpaca/bars_1d_page1.json")),
        ("1Day", Some(_)) => json(include_str!("../fixtures/alpaca/bars_1d_page2.json")),
        (_, None) => json(include_str!("../fixtures/alpaca/bars_1m_page1.json")),
        (_, Some(_)) => json(include_str!("../fixtures/alpaca/bars_1m_page2.json")),
    }
}

async fn calendar() -> Response {
    json(include_str!("../fixtures/alpaca/calendar.json"))
}

async fn assets() -> Response {
    json(include_str!("../fixtures/alpaca/assets.json"))
}

async fn actions(
    State(seen): State<Seen>,
    headers: HeaderMap,
    Query(params): Query<Params>,
) -> Response {
    record(&seen, "actions", &headers, &params);
    if params.contains_key("page_token") {
        json(include_str!(
            "../fixtures/alpaca/corporate_actions_page2.json"
        ))
    } else {
        json(include_str!(
            "../fixtures/alpaca/corporate_actions_page1.json"
        ))
    }
}

async fn news(
    State(seen): State<Seen>,
    headers: HeaderMap,
    Query(params): Query<Params>,
) -> Response {
    record(&seen, "news", &headers, &params);
    if params.contains_key("page_token") {
        json(include_str!("../fixtures/alpaca/news_page2.json"))
    } else {
        json(include_str!("../fixtures/alpaca/news_page1.json"))
    }
}

async fn serve_fixtures() -> (String, Seen) {
    let seen = Seen::default();
    let router = Router::new()
        .route("/v2/stocks/bars", get(bars))
        .route("/v2/calendar", get(calendar))
        .route("/v2/assets", get(assets))
        .route("/v1/corporate-actions", get(actions))
        .route("/v1beta1/news", get(news))
        .with_state(seen.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { serve(listener, router).await.unwrap() });
    (url, seen)
}

fn date(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

fn at(day: &str, time: &str) -> Timestamp {
    Timestamp::at_exchange_time(date(day), NaiveTime::parse_from_str(time, "%H:%M").unwrap())
        .unwrap()
}

fn decimal(text: &str) -> Decimal {
    text.parse().unwrap()
}

fn request(timeframe: Timeframe, symbols: &[&str], to: Timestamp) -> BarsRequest {
    BarsRequest {
        symbols: symbols.iter().copied().map(Symbol::new).collect(),
        timeframe,
        from: at("2024-07-01", "00:00"),
        to,
        feed: Feed::Sip,
        tickers: Tickers::Today,
    }
}

#[tokio::test]
async fn alpaca_client_pages_filters_and_normalizes_against_fixtures() {
    let (url, seen) = serve_fixtures().await;
    let client = AlpacaClient::new(AlpacaConfig {
        data_url: url.clone(),
        trading_url: url,
        ..AlpacaConfig::new("key".to_owned(), "secret".to_owned())
    })
    .unwrap();

    let sessions = client
        .calendar(date("2024-07-01"), date("2024-07-08"))
        .await
        .unwrap();
    let half_day = sessions
        .iter()
        .find(|s| s.date == date("2024-07-03"))
        .unwrap();
    assert_eq!(
        *half_day,
        Session {
            date: date("2024-07-03"),
            open: at("2024-07-03", "09:30"),
            close: at("2024-07-03", "13:00"),
        }
    );
    let calendar = Calendar::new(sessions);

    let daily = client
        .bars(
            &request(Timeframe::Day, &["AAPL", "TSLA"], at("2024-07-08", "00:00")),
            &calendar,
        )
        .await
        .unwrap();
    assert_eq!(daily.len(), 6);
    assert_eq!(daily[0].symbol, Symbol::new("AAPL"));
    assert_eq!(daily[0].start, at("2024-07-02", "09:30"));
    assert_eq!(daily[0].close, decimal("216.27"));
    assert_eq!(daily[0].volume, 41_601_367);
    assert_eq!(daily[5].symbol, Symbol::new("TSLA"));
    assert_eq!(daily[5].start, at("2024-07-05", "09:30"));
    assert!(daily.iter().all(|bar| bar.feed == Feed::Sip));

    let minutes = client
        .bars(
            &request(
                Timeframe::Minute,
                &["AAPL", "TSLA"],
                at("2024-07-08", "00:00"),
            ),
            &calendar,
        )
        .await
        .unwrap();
    let starts: Vec<_> = minutes
        .iter()
        .map(|bar| (bar.symbol.as_str(), bar.start))
        .collect();
    assert_eq!(
        starts,
        [
            ("AAPL", at("2024-07-03", "09:30")),
            ("AAPL", at("2024-07-03", "09:31")),
            ("AAPL", at("2024-07-03", "12:59")),
            ("TSLA", at("2024-07-05", "09:30")),
            ("TSLA", at("2024-07-05", "15:59")),
        ]
    );

    let future = request(Timeframe::Minute, &["AAPL"], at("2100-01-01", "00:00"));
    client.bars(&future, &calendar).await.unwrap();
    let sip_end: Timestamp = seen
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(route, _)| route == "bars")
        .unwrap()
        .1["end"]
        .parse()
        .unwrap();
    assert!(Timestamp::now() - sip_end >= chrono::TimeDelta::minutes(15));

    let mid_session = request(Timeframe::Day, &["AAPL"], at("2024-07-05", "14:00"));
    client.bars(&mid_session, &calendar).await.unwrap();
    let daily_end = seen
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(route, _)| route == "bars")
        .unwrap()
        .1["end"]
        .clone();
    assert_eq!(daily_end, "2024-07-03T17:00:00Z");

    let seen_bars = seen.lock().unwrap().clone();
    assert!(
        seen_bars
            .iter()
            .filter(|(route, _)| route == "bars")
            .all(|(_, p)| p["adjustment"] == "raw")
    );

    let limited = client
        .bars(
            &request(Timeframe::Day, &["LIMITED"], at("2024-07-08", "00:00")),
            &calendar,
        )
        .await;
    assert!(matches!(
        limited,
        Err(SourceError::RateLimited { retry_after: Some(wait) }) if wait == Duration::from_secs(120)
    ));

    let instruments = client.assets().await.unwrap();
    assert_eq!(instruments.len(), 3);
    assert!(
        instruments
            .iter()
            .all(|instrument| instrument.asset_class == AssetClass::Stock)
    );
    assert_eq!(instruments[1].exchange, "ARCA");
    assert!(instruments[0].active && !instruments[2].active);

    let actions = client
        .corporate_actions(
            &[Symbol::new("NVDA")],
            date("2024-05-01"),
            date("2024-07-31"),
        )
        .await
        .unwrap();
    assert_eq!(actions.len(), 10);
    let kinds: Vec<_> = actions
        .iter()
        .map(|a| (a.symbol.as_str(), a.ex_date, a.kind.clone()))
        .collect();
    for expected in [
        (
            "UA",
            "2024-05-20",
            CorporateActionKind::StockDividend { rate: decimal("1") },
        ),
        (
            "UTX",
            "2024-06-03",
            CorporateActionKind::SpinOff {
                new_symbol: Symbol::new("OTIS"),
                old_rate: decimal("2"),
                new_rate: decimal("1"),
            },
        ),
        (
            "APC",
            "2024-06-05",
            CorporateActionKind::CashMerger {
                rate: decimal("59"),
            },
        ),
        (
            "CRC",
            "2024-06-06",
            CorporateActionKind::StockMerger {
                acquirer: Symbol::new("OXY"),
                old_rate: decimal("1"),
                new_rate: decimal("0.2934"),
                cash_rate: decimal("36.5"),
            },
        ),
        ("SIVB", "2024-06-07", CorporateActionKind::WorthlessRemoval),
        (
            "FB",
            "2024-06-10",
            CorporateActionKind::NameChange {
                new_symbol: Symbol::new("META"),
            },
        ),
    ] {
        let (symbol, ex_date, kind) = expected;
        assert!(
            kinds.contains(&(symbol, date(ex_date), kind.clone())),
            "{symbol} {kind:?}"
        );
    }
    assert!(kinds.contains(&(
        "NVDA",
        date("2024-06-10"),
        CorporateActionKind::Split {
            old_rate: decimal("1"),
            new_rate: decimal("10")
        }
    )));
    assert!(kinds.contains(&(
        "MNTS",
        date("2024-06-24"),
        CorporateActionKind::Split {
            old_rate: decimal("50"),
            new_rate: decimal("1")
        }
    )));
    assert!(kinds.contains(&(
        "AAPL",
        date("2024-05-10"),
        CorporateActionKind::CashDividend {
            rate: decimal("0.35")
        }
    )));
    assert!(kinds.contains(&(
        "SPY",
        date("2024-06-21"),
        CorporateActionKind::CashDividend {
            rate: decimal("1.5395")
        }
    )));
}

#[tokio::test]
async fn news_history_pages_through_a_window_and_is_marked_backfilled() {
    let (url, seen) = serve_fixtures().await;
    let client = AlpacaClient::new(AlpacaConfig {
        data_url: url.clone(),
        trading_url: url,
        ..AlpacaConfig::new("key".to_owned(), "secret".to_owned())
    })
    .unwrap();
    let (from, to) = (at("2026-10-30", "00:00"), at("2026-10-31", "00:00"));
    let before = Timestamp::now();

    let first = client.news_page(from, to, None).await.unwrap();
    let second = client.news_page(from, to, first.next).await.unwrap();
    assert!(second.next.is_none());
    let articles: Vec<_> = first.articles.iter().chain(&second.articles).collect();
    assert_eq!(
        articles
            .iter()
            .map(|article| (article.id, article.symbols.len()))
            .collect::<Vec<_>>(),
        [(24843171, 2), (24843302, 1)]
    );
    assert!(
        articles
            .iter()
            .all(|article| article.backfilled && article.received_at >= before)
    );
    let requests = seen.lock().unwrap().clone();
    let (_, params) = &requests[0];
    assert_eq!(params["start"], from.to_string());
    assert_eq!(params["end"], to.to_string());
    assert_eq!(params["sort"], "asc");
    assert!(requests[1].1.contains_key("page_token"));
}
