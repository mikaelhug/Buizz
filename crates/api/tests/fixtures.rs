use axum::body::Body;
use axum::http::{Method, Request, header};
use tower::ServiceExt;

#[tokio::test]
async fn every_stub_route_serves_a_fixture_that_matches_its_type() {
    let routes = [
        (Method::GET, "/api/bots", None),
        (Method::GET, "/api/bots/red_green", None),
        (
            Method::POST,
            "/api/bots/red_green/backtests",
            Some(r#"{"preset":"1y"}"#),
        ),
        (Method::POST, "/api/bots/red_green/live", None),
        (Method::POST, "/api/bots/red_green/pause", None),
        (Method::GET, "/api/runs?bot=red_green&mode=backtest", None),
        (Method::GET, "/api/runs/41", None),
        (Method::POST, "/api/runs/41/cancel", None),
        (Method::GET, "/api/runs/41/equity", None),
        (Method::GET, "/api/runs/41/orders", None),
        (Method::GET, "/api/runs/41/positions", None),
        (Method::GET, "/api/runs/41/events?after=2", None),
        (Method::GET, "/api/leaderboard?preset=1y", None),
        (Method::GET, "/api/data/health", None),
        (Method::GET, "/api/data/jobs", None),
        (Method::GET, "/api/data/feed", None),
        (Method::POST, "/api/auth/login", Some(r#"{"password":"x"}"#)),
        (Method::POST, "/api/auth/logout", None),
        (Method::GET, "/api/auth/session", None),
        (Method::GET, "/api/events", None),
    ];
    for (method, uri, body) in routes {
        let request = Request::builder()
            .method(method.clone())
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, Body::from))
            .expect("valid request");
        let response = buizz_api::fixture_router()
            .oneshot(request)
            .await
            .expect("router answers");
        assert!(
            response.status().is_success(),
            "{method} {uri} -> {}",
            response.status()
        );
    }
}
