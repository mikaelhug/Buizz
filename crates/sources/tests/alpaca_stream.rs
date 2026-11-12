use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use buizz_core::market::{Feed, Symbol};
use buizz_core::time::Timestamp;
use buizz_sources::alpaca::AlpacaConfig;
use buizz_sources::alpaca_stream::{SYMBOL_CAP, StreamConfig, StreamEvent, spawn, spawn_news};
use chrono::TimeDelta;
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

const MINUTE_NANOS: i64 = 60_000_000_000;
const BAR_FRAMES: [&str; 2] = ["bars_first.json", "bars_second.json"];

#[derive(Clone)]
struct FakeServer {
    connections: Arc<Mutex<usize>>,
    received: Arc<Mutex<Vec<Value>>>,
    authenticated: &'static str,
    frames: [&'static str; 2],
}

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/fixtures/alpaca_stream/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("fixture exists")
}

async fn serve(server: FakeServer) -> SocketAddr {
    let app = Router::new().route("/", get(upgrade)).with_state(server);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

async fn upgrade(State(server): State<FakeServer>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| converse(server, socket))
}

async fn read(socket: &mut WebSocket, server: &FakeServer) -> Value {
    let Some(Ok(Message::Text(text))) = socket.recv().await else {
        panic!("client sent no text frame");
    };
    let value: Value = serde_json::from_str(&text).unwrap();
    server.received.lock().unwrap().push(value.clone());
    value
}

async fn converse(server: FakeServer, mut socket: WebSocket) {
    let connection = {
        let mut connections = server.connections.lock().unwrap();
        *connections += 1;
        *connections
    };
    socket
        .send(Message::text(fixture("connected.json")))
        .await
        .unwrap();
    read(&mut socket, &server).await;
    socket
        .send(Message::text(fixture(server.authenticated)))
        .await
        .unwrap();
    if server.authenticated != "authenticated.json" {
        return;
    }
    read(&mut socket, &server).await;
    socket
        .send(Message::text(fixture("subscription.json")))
        .await
        .unwrap();
    if connection == 1 {
        socket
            .send(Message::text(fixture(server.frames[0])))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        return;
    }
    socket
        .send(Message::text(fixture(server.frames[1])))
        .await
        .unwrap();
    std::future::pending::<()>().await;
}

fn config(address: SocketAddr) -> StreamConfig {
    StreamConfig {
        url: format!("ws://{address}/"),
        reconnect_initial: Duration::from_millis(50),
        reconnect_max: Duration::from_millis(200),
        ..StreamConfig::new(AlpacaConfig::new("key".to_owned(), "secret".to_owned()))
    }
}

async fn next(events: &mut Receiver<StreamEvent>) -> StreamEvent {
    timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("event within deadline")
        .expect("stream still running")
}

fn now() -> i64 {
    Timestamp::now().nanos()
}

#[tokio::test]
async fn streams_bars_and_backfills_a_gap_after_reconnect() {
    let server = FakeServer {
        connections: Arc::default(),
        received: Arc::default(),
        authenticated: "authenticated.json",
        frames: BAR_FRAMES,
    };
    let address = serve(server.clone()).await;
    let (handle, mut events) = spawn(config(address));
    handle
        .subscribe(vec![Symbol::new("aapl"), Symbol::new("msft")])
        .await
        .unwrap();
    let started = now();

    assert_eq!(next(&mut events).await, StreamEvent::Connected);
    assert_eq!(
        next(&mut events).await,
        StreamEvent::Subscribed {
            symbols: vec![Symbol::new("AAPL"), Symbol::new("MSFT")]
        }
    );
    let event = next(&mut events).await;
    let StreamEvent::Bar(bar) = event else {
        panic!("expected a bar {event:?}");
    };
    assert_eq!(bar.symbol, Symbol::new("AAPL"));
    assert_eq!(bar.start, "2024-03-01T14:30:00Z".parse().unwrap());
    assert_eq!(bar.open, "187.15".parse::<Decimal>().unwrap());
    assert_eq!(bar.close, "187.25".parse::<Decimal>().unwrap());
    assert_eq!(bar.volume, 10492);
    assert_eq!(bar.feed, Feed::Iex);
    assert!(matches!(next(&mut events).await, StreamEvent::Bar(_)));

    assert!(matches!(
        next(&mut events).await,
        StreamEvent::Disconnected { .. }
    ));
    assert_eq!(next(&mut events).await, StreamEvent::Connected);
    let StreamEvent::Gap { from, to } = next(&mut events).await else {
        panic!("expected a gap");
    };
    let finished = now();
    assert_eq!(from.nanos() % MINUTE_NANOS, 0);
    assert_eq!(to.nanos() % MINUTE_NANOS, 0);
    assert!(from.nanos() >= started - started % MINUTE_NANOS - MINUTE_NANOS);
    assert!(to - from >= TimeDelta::minutes(1));
    assert!(to.nanos() <= finished);
    assert!(matches!(
        next(&mut events).await,
        StreamEvent::Subscribed { .. }
    ));
    let StreamEvent::Bar(bar) = next(&mut events).await else {
        panic!("expected a bar after reconnect");
    };
    assert_eq!(bar.start, "2024-03-01T14:35:00Z".parse().unwrap());

    let received = server.received.lock().unwrap().clone();
    let actions: Vec<_> = received
        .iter()
        .map(|message| message["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["auth", "subscribe", "auth", "subscribe"]);
    assert_eq!(received[0]["key"], "key");
    assert_eq!(received[1]["bars"], serde_json::json!(["AAPL", "MSFT"]));
    assert_eq!(received[3]["bars"], received[1]["bars"]);
}

#[tokio::test]
async fn refuses_symbols_beyond_the_cap() {
    let server = FakeServer {
        connections: Arc::default(),
        received: Arc::default(),
        authenticated: "authenticated.json",
        frames: BAR_FRAMES,
    };
    let address = serve(server).await;
    let (handle, _events) = spawn(config(address));
    let too_many = (0..=SYMBOL_CAP)
        .map(|index| Symbol::new(&format!("S{index}")))
        .collect();
    assert!(handle.subscribe(too_many).await.is_err());
}

#[tokio::test]
async fn auth_failure_is_reported_and_not_retried() {
    let server = FakeServer {
        connections: Arc::default(),
        received: Arc::default(),
        authenticated: "auth_failed.json",
        frames: BAR_FRAMES,
    };
    let address = serve(server.clone()).await;
    let (_handle, mut events) = spawn(config(address));

    let StreamEvent::Disconnected { reason } = next(&mut events).await else {
        panic!("expected a disconnect");
    };
    assert!(reason.contains("authentication failed"), "{reason}");
    assert!(next_closed(&mut events).await);
    assert_eq!(*server.connections.lock().unwrap(), 1);
}

#[tokio::test]
async fn records_every_edit_of_an_article_as_it_arrives_across_reconnects() {
    let server = FakeServer {
        connections: Arc::default(),
        received: Arc::default(),
        authenticated: "authenticated.json",
        frames: ["news_first.json", "news_second.json"],
    };
    let address = serve(server.clone()).await;
    let started = Timestamp::now();
    let mut articles = spawn_news(config(address));
    let mut arrivals = Vec::new();
    for _ in 0..2 {
        arrivals.push(
            timeout(Duration::from_secs(5), articles.recv())
                .await
                .expect("article within deadline")
                .expect("recorder still running"),
        );
    }
    let [original, edited] = &arrivals[..] else {
        panic!("two arrivals");
    };
    assert_eq!(original.id, edited.id);
    assert_eq!(original.created_at, edited.created_at);
    assert_eq!(original.updated_at, original.created_at);
    assert_eq!(edited.updated_at, "2026-10-30T20:47:55Z".parse().unwrap());
    assert_ne!(original.headline, edited.headline);
    assert_eq!(edited.symbols, [Symbol::new("AAPL"), Symbol::new("QQQ")]);
    assert!(!original.backfilled && !edited.backfilled);
    assert!(started <= original.received_at && original.received_at <= edited.received_at);

    let received = server.received.lock().unwrap().clone();
    assert_eq!(received[1]["action"], "subscribe");
    assert_eq!(received[1]["news"], serde_json::json!(["*"]));
    assert_eq!(*server.connections.lock().unwrap(), 2);
}

async fn next_closed(events: &mut Receiver<StreamEvent>) -> bool {
    timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("stream ends within deadline")
        .is_none()
}
