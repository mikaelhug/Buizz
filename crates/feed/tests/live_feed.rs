use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use buizz_core::market::{DataSource, Session, Symbol, Timeframe};
use buizz_core::run::RunId;
use buizz_core::time::Timestamp;
use buizz_feed::{FeedConfig, FeedError, FeedManager};
use buizz_market::{FILE_NAME, MarketStore};
use buizz_sources::alpaca::{AlpacaClient, AlpacaConfig};
use buizz_sources::alpaca_stream::StreamConfig;
use chrono::{NaiveTime, TimeDelta, Utc};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

const DEADLINE: Duration = Duration::from_secs(10);

enum Command {
    Send(String),
    Disconnect,
}

#[derive(Clone)]
struct FakeAlpaca {
    commands: Arc<tokio::sync::Mutex<UnboundedReceiver<Command>>>,
    stream_frames: Arc<Mutex<Vec<String>>>,
    bar_requests: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

async fn serve(fake: FakeAlpaca) -> SocketAddr {
    let app = Router::new()
        .route("/stream", get(upgrade))
        .route("/v2/stocks/bars", get(bars))
        .with_state(fake);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

async fn upgrade(State(fake): State<FakeAlpaca>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| converse(fake, socket))
}

async fn converse(fake: FakeAlpaca, mut socket: WebSocket) {
    let mut commands = fake.commands.lock().await;
    let connected = r#"[{"T":"success","msg":"connected"}]"#;
    socket.send(Message::text(connected)).await.unwrap();
    socket.recv().await;
    let authenticated = r#"[{"T":"success","msg":"authenticated"}]"#;
    socket.send(Message::text(authenticated)).await.unwrap();
    let mut subscribed = BTreeSet::new();
    loop {
        tokio::select! {
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    fake.stream_frames.lock().unwrap().push(text.to_string());
                    let listed = listed_bars(&text);
                    if text.contains(r#""action":"unsubscribe""#) {
                        subscribed.retain(|symbol| !listed.contains(symbol));
                    } else {
                        subscribed.extend(listed);
                    }
                    let bars: Vec<String> = subscribed.iter().map(|symbol| format!(r#""{symbol}""#)).collect();
                    let ack = format!(
                        r#"[{{"T":"subscription","trades":[],"quotes":[],"bars":[{}]}}]"#,
                        bars.join(",")
                    );
                    socket.send(Message::text(ack)).await.unwrap();
                }
                Some(Ok(_)) => {}
                _ => return,
            },
            command = commands.recv() => match command {
                Some(Command::Send(frame)) => socket.send(Message::text(frame)).await.unwrap(),
                Some(Command::Disconnect) | None => return,
            },
        }
    }
}

fn listed_bars(frame: &str) -> Vec<String> {
    let list = frame
        .split(r#""bars":["#)
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_default();
    list.split(',')
        .map(|symbol| symbol.trim_matches('"').to_owned())
        .filter(|symbol| !symbol.is_empty())
        .collect()
}

async fn bars(
    State(fake): State<FakeAlpaca>,
    Query(query): Query<HashMap<String, String>>,
) -> String {
    fake.bar_requests.lock().unwrap().push(query.clone());
    let bars: Vec<String> = query["symbols"]
        .split(',')
        .map(|symbol| {
            format!(
                r#""{symbol}":[{{"t":"{}","o":99,"h":99,"l":99,"c":99,"v":7}}]"#,
                query["start"]
            )
        })
        .collect();
    format!(
        r#"{{"bars":{{{}}},"next_page_token":null}}"#,
        bars.join(",")
    )
}

fn bar_frame(symbol: &str, start: Timestamp) -> String {
    format!(
        r#"[{{"T":"b","S":"{symbol}","o":10.5,"h":10.6,"l":10.4,"c":10.55,"v":100,"t":"{start}"}}]"#
    )
}

fn symbols(names: &[&str]) -> Vec<Symbol> {
    names.iter().map(|name| Symbol::new(name)).collect()
}

fn whole_days_around_now() -> Vec<Session> {
    let today = Utc::now().date_naive();
    let midnight = |date| Timestamp::at_exchange_time(date, NaiveTime::MIN).unwrap();
    (-2..=2)
        .map(|offset| today + TimeDelta::days(offset))
        .map(|date| Session {
            date,
            open: midnight(date),
            close: midnight(date + TimeDelta::days(1)),
        })
        .collect()
}

async fn eventually<T, F: Future<Output = Option<T>>>(mut probe: impl FnMut() -> F) -> T {
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let Some(value) = probe().await {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition holds within the deadline")
}

fn frames(fake: &FakeAlpaca, action: &str) -> Vec<String> {
    let action = format!(r#""action":"{action}""#);
    fake.stream_frames
        .lock()
        .unwrap()
        .iter()
        .filter(|frame| frame.contains(&action))
        .cloned()
        .collect()
}

async fn closes_at(market: &MarketStore, start: Timestamp) -> Vec<(String, String)> {
    market
        .bars_between(
            Timeframe::Minute,
            symbols(&["AAA", "BBB", "CCC", "DDD"]),
            start,
            start + TimeDelta::minutes(1),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|bar| (bar.symbol.to_string(), bar.close.normalize().to_string()))
        .collect()
}

#[tokio::test]
async fn leases_share_stream_slots_and_live_bars_land_with_gaps_backfilled() {
    let (commands, receiver): (UnboundedSender<Command>, _) = unbounded_channel();
    let fake = FakeAlpaca {
        commands: Arc::new(tokio::sync::Mutex::new(receiver)),
        stream_frames: Arc::default(),
        bar_requests: Arc::default(),
    };
    let address = serve(fake.clone()).await;
    let alpaca = AlpacaConfig {
        data_url: format!("http://{address}"),
        ..AlpacaConfig::new("key".to_owned(), "secret".to_owned())
    };
    let stream = StreamConfig {
        url: format!("ws://{address}/stream"),
        reconnect_initial: Duration::from_millis(50),
        reconnect_max: Duration::from_millis(200),
        ..StreamConfig::new(alpaca.clone())
    };
    let dir = tempfile::tempdir().unwrap();
    let market = MarketStore::open(&dir.path().join(FILE_NAME), 2)
        .await
        .unwrap();
    market
        .upsert_sessions(DataSource::Fixture, whole_days_around_now())
        .await
        .unwrap();
    let feed = FeedManager::new(
        FeedConfig {
            cap: 3,
            stream: Some(stream),
        },
        market.clone(),
        Some(AlpacaClient::new(alpaca).unwrap()),
    );
    let task = feed.spawn();

    let alpha = feed
        .acquire(RunId(1), "alpha", symbols(&["AAA", "BBB"]))
        .unwrap();
    let mut beta = feed
        .acquire(RunId(2), "beta", symbols(&["BBB", "CCC"]))
        .unwrap();
    let refused = feed.acquire(RunId(3), "gamma", symbols(&["DDD"]));
    let Err(FeedError::CapExceeded {
        requested,
        available,
        holders,
    }) = refused
    else {
        panic!("a fourth symbol must exceed the cap of three");
    };
    assert_eq!((requested, available), (1, 0));
    let named: Vec<_> = holders
        .iter()
        .map(|holder| (holder.run_id, holder.bot.as_str()))
        .collect();
    assert_eq!(named, [(RunId(1), "alpha"), (RunId(2), "beta")]);
    assert_eq!(feed.slots().borrow().used, 3);

    eventually(|| async {
        let subscribed = frames(&fake, "subscribe").concat();
        ["AAA", "BBB", "CCC"]
            .iter()
            .all(|symbol| subscribed.contains(symbol))
            .then_some(())
    })
    .await;
    drop(alpha);
    let released = eventually(|| async { frames(&fake, "unsubscribe").pop() }).await;
    assert!(released.contains("AAA"), "{released}");
    assert!(
        !released.contains("BBB") && !released.contains("CCC"),
        "{released}"
    );
    assert_eq!(feed.slots().borrow().used, 2);

    assert!(matches!(
        beta.cover(symbols(&["BBB", "CCC", "DDD", "EEE"])),
        Err(FeedError::CapExceeded {
            requested: 4,
            available: 3,
            ..
        })
    ));
    assert_eq!(beta.symbols(), symbols(&["BBB", "CCC"]));
    let _gamma = feed.acquire(RunId(3), "gamma", symbols(&["DDD"])).unwrap();
    eventually(|| async {
        frames(&fake, "subscribe")
            .concat()
            .contains("DDD")
            .then_some(())
    })
    .await;
    let slots = feed.slots().borrow().clone();
    assert_eq!((slots.used, slots.connected, slots.error), (3, true, None));

    let watermarks = market.watermarks();
    let minute = eventually(|| async { watermarks.borrow().minute }).await;
    for symbol in ["BBB", "CCC", "DDD"] {
        commands
            .send(Command::Send(bar_frame(symbol, minute)))
            .unwrap();
    }
    eventually(|| async { (watermarks.borrow().minute > Some(minute)).then_some(()) }).await;
    let streamed = closes_at(&market, minute).await;
    let expected: Vec<_> = ["BBB", "CCC", "DDD"]
        .map(|symbol| (symbol.to_owned(), "10.55".to_owned()))
        .to_vec();
    assert_eq!(streamed, expected);

    commands.send(Command::Disconnect).unwrap();
    let backfilled = eventually(|| async {
        let request = fake.bar_requests.lock().unwrap().last().cloned()?;
        let start: Timestamp = request["start"].parse().unwrap();
        let closes = closes_at(&market, start).await;
        (closes.len() == 3 && closes.iter().all(|(_, close)| close == "99")).then_some(request)
    })
    .await;
    assert_eq!(backfilled["symbols"], "BBB,CCC,DDD");
    assert_eq!(backfilled["feed"], "iex");
    let slots = eventually(|| async {
        let slots = feed.slots().borrow().clone();
        (slots.connected && slots.error.is_none()).then_some(slots)
    })
    .await;
    assert_eq!(slots.used, 3);
    assert!(watermarks.borrow().minute > Some(minute));

    task.abort();
}
