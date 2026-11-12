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
use chrono::{NaiveTime, TimeDelta};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

const DEADLINE: Duration = Duration::from_secs(10);

enum Command {
    Send(String),
    Disconnect,
}

#[derive(Clone, Default)]
struct FakeAlpaca {
    connections: Arc<Mutex<HashMap<String, UnboundedSender<Command>>>>,
    stream_frames: Arc<Mutex<Vec<(String, String)>>>,
    bar_requests: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

impl FakeAlpaca {
    fn command(&self, account: &str, command: Command) {
        self.connections.lock().unwrap()[account]
            .send(command)
            .unwrap();
    }
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
    let connected = r#"[{"T":"success","msg":"connected"}]"#;
    socket.send(Message::text(connected)).await.unwrap();
    let Some(Ok(Message::Text(auth))) = socket.recv().await else {
        return;
    };
    let account = auth
        .split(r#""key":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_owned();
    let authenticated = r#"[{"T":"success","msg":"authenticated"}]"#;
    socket.send(Message::text(authenticated)).await.unwrap();
    let (sender, mut commands) = unbounded_channel();
    fake.connections
        .lock()
        .unwrap()
        .insert(account.clone(), sender);
    let mut subscribed = BTreeSet::new();
    loop {
        tokio::select! {
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    fake.stream_frames.lock().unwrap().push((account.clone(), text.to_string()));
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
    let today = Timestamp::now().exchange_date();
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

fn frames(fake: &FakeAlpaca, account: &str, action: &str) -> Vec<String> {
    let action = format!(r#""action":"{action}""#);
    fake.stream_frames
        .lock()
        .unwrap()
        .iter()
        .filter(|(sent_on, frame)| sent_on == account && frame.contains(&action))
        .map(|(_, frame)| frame.clone())
        .collect()
}

async fn closes_at(market: &MarketStore, start: Timestamp) -> Vec<(String, String)> {
    market
        .bars_between(
            Timeframe::Minute,
            symbols(&["AAA", "BBB", "CCC", "DDD", "EEE"]),
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
async fn leases_share_two_accounts_and_a_dropped_connection_backfills_only_its_symbols() {
    let fake = FakeAlpaca::default();
    let address = serve(fake.clone()).await;
    let account = |key: &str| AlpacaConfig {
        data_url: format!("http://{address}"),
        ..AlpacaConfig::new(key.to_owned(), "secret".to_owned())
    };
    let stream = |key: &str| StreamConfig {
        url: format!("ws://{address}/stream"),
        reconnect_initial: Duration::from_millis(50),
        reconnect_max: Duration::from_millis(200),
        ..StreamConfig::new(account(key))
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
            symbols_per_connection: 2,
            streams: vec![stream("one"), stream("two")],
        },
        market.clone(),
        Some(AlpacaClient::new(account("one")).unwrap()),
    );
    let task = feed.spawn();
    assert_eq!(feed.slots().borrow().cap, 4);

    let alpha = feed
        .acquire(RunId(1), "alpha", symbols(&["AAA", "BBB"]))
        .unwrap();
    let mut beta = feed
        .acquire(RunId(2), "beta", symbols(&["BBB", "CCC", "DDD"]))
        .unwrap();
    let refused = feed.acquire(RunId(3), "gamma", symbols(&["EEE"]));
    let Err(FeedError::CapExceeded {
        requested,
        available,
        holders,
    }) = refused
    else {
        panic!("a fifth symbol must exceed two accounts of two");
    };
    assert_eq!((requested, available), (1, 0));
    let named: Vec<_> = holders
        .iter()
        .map(|holder| (holder.run_id, holder.bot.as_str()))
        .collect();
    assert_eq!(named, [(RunId(1), "alpha"), (RunId(2), "beta")]);
    let used: Vec<u32> = feed
        .slots()
        .borrow()
        .connections
        .iter()
        .map(|connection| connection.used)
        .collect();
    assert_eq!(used, [2, 2]);

    let subscribed = |account: &'static str, wanted: &'static [&'static str]| {
        let fake = fake.clone();
        async move {
            let sent = frames(&fake, account, "subscribe").concat();
            wanted
                .iter()
                .all(|symbol| sent.contains(symbol))
                .then_some(sent)
        }
    };
    let one = eventually(|| subscribed("one", &["AAA", "CCC"])).await;
    let two = eventually(|| subscribed("two", &["BBB", "DDD"])).await;
    assert!(!one.contains("BBB") && !one.contains("DDD"), "{one}");
    assert!(!two.contains("AAA") && !two.contains("CCC"), "{two}");

    drop(alpha);
    let released = eventually(|| async { frames(&fake, "one", "unsubscribe").pop() }).await;
    assert!(released.contains("AAA"), "{released}");
    assert!(frames(&fake, "two", "unsubscribe").is_empty());
    assert_eq!(feed.slots().borrow().used, 3);

    assert!(matches!(
        beta.cover(symbols(&["BBB", "CCC", "DDD", "EEE", "FFF"])),
        Err(FeedError::CapExceeded {
            requested: 5,
            available: 4,
            ..
        })
    ));
    assert_eq!(beta.symbols(), symbols(&["BBB", "CCC", "DDD"]));
    let _gamma = feed.acquire(RunId(3), "gamma", symbols(&["EEE"])).unwrap();
    eventually(|| subscribed("one", &["EEE"])).await;
    let slots = eventually(|| async {
        let slots = feed.slots().borrow().clone();
        slots
            .connections
            .iter()
            .all(|connection| connection.connected && connection.error.is_none())
            .then_some(slots)
    })
    .await;
    assert_eq!((slots.used, slots.error), (4, None));
    let now = Timestamp::now();
    let session_open = whole_days_around_now()
        .into_iter()
        .find(|session| session.open <= now && now < session.close)
        .unwrap()
        .open;
    eventually(|| async {
        closes_at(&market, session_open)
            .await
            .contains(&("EEE".to_owned(), "99".to_owned()))
            .then_some(())
    })
    .await;

    let watermarks = market.watermarks();
    let minute = eventually(|| async { watermarks.borrow().minute }).await;
    for (account, symbol) in [
        ("one", "CCC"),
        ("one", "EEE"),
        ("two", "BBB"),
        ("two", "DDD"),
    ] {
        fake.command(account, Command::Send(bar_frame(symbol, minute)));
    }
    eventually(|| async { (watermarks.borrow().minute > Some(minute)).then_some(()) }).await;
    let streamed = closes_at(&market, minute).await;
    let expected: Vec<_> = ["BBB", "CCC", "DDD", "EEE"]
        .map(|symbol| (symbol.to_owned(), "10.55".to_owned()))
        .to_vec();
    assert_eq!(streamed, expected);

    let requests_before = fake.bar_requests.lock().unwrap().len();
    fake.command("two", Command::Disconnect);
    let backfilled = eventually(|| async {
        let request = fake.bar_requests.lock().unwrap()[requests_before..]
            .last()
            .cloned()?;
        let start: Timestamp = request["start"].parse().unwrap();
        let closes = closes_at(&market, start).await;
        let replaced = ["BBB", "DDD"]
            .iter()
            .all(|symbol| closes.contains(&((*symbol).to_owned(), "99".to_owned())));
        replaced.then_some(request)
    })
    .await;
    assert_eq!(backfilled["symbols"], "BBB,DDD");
    assert_eq!(backfilled["feed"], "iex");
    assert!(
        fake.bar_requests.lock().unwrap()[requests_before..]
            .iter()
            .all(|request| request["symbols"] == "BBB,DDD"),
        "only the dropped account's symbols are backfilled"
    );
    let slots = eventually(|| async {
        let slots = feed.slots().borrow().clone();
        slots
            .connections
            .iter()
            .all(|connection| connection.connected && connection.error.is_none())
            .then_some(slots)
    })
    .await;
    assert_eq!(slots.used, 4);
    assert!(watermarks.borrow().minute > Some(minute));

    task.abort();
}
