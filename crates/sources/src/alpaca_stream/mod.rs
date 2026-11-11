mod wire;

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use buizz_core::market::{Bar, Symbol};
use buizz_core::time::Timestamp;
use chrono::{TimeDelta, Utc};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use wire::{AUTH_FAILED_CODES, Frame, SYMBOL_LIMIT_CODE};

use crate::alpaca::AlpacaConfig;
use crate::http::SourceError;

pub const STREAM_URL: &str = "wss://stream.data.alpaca.markets/v2/iex";
pub const STREAM_URL_ENV: &str = "ALPACA_STREAM_URL";
pub const SYMBOL_CAP: usize = 30;
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const EVENT_BUFFER: usize = 1024;
const MINUTE_NANOS: i64 = 60_000_000_000;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Subscriptions = BTreeSet<Symbol>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamConfig {
    pub alpaca: AlpacaConfig,
    pub url: String,
    pub reconnect_initial: Duration,
    pub reconnect_max: Duration,
}

impl StreamConfig {
    pub fn new(alpaca: AlpacaConfig) -> Self {
        Self {
            alpaca,
            url: env::var(STREAM_URL_ENV).unwrap_or_else(|_| STREAM_URL.to_owned()),
            reconnect_initial: Duration::from_secs(1),
            reconnect_max: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    Connected,
    Subscribed { symbols: Vec<Symbol> },
    Bar(Bar),
    Gap { from: Timestamp, to: Timestamp },
    SymbolLimitExceeded,
    Disconnected { reason: String },
}

#[derive(Clone)]
pub struct StreamHandle {
    desired: Arc<watch::Sender<Subscriptions>>,
}

impl StreamHandle {
    pub async fn subscribe(&self, symbols: Vec<Symbol>) -> Result<(), SourceError> {
        let mut over_cap = false;
        self.desired.send_if_modified(|current| {
            let mut next = current.clone();
            next.extend(symbols);
            if next.len() > SYMBOL_CAP {
                over_cap = true;
                return false;
            }
            let changed = next != *current;
            *current = next;
            changed
        });
        if over_cap {
            return Err(SourceError::Stream(format!(
                "subscribing would exceed the {SYMBOL_CAP} symbol cap"
            )));
        }
        Ok(())
    }

    pub async fn unsubscribe(&self, symbols: Vec<Symbol>) -> Result<(), SourceError> {
        self.desired.send_if_modified(|current| {
            let before = current.len();
            current.retain(|symbol| !symbols.contains(symbol));
            current.len() != before
        });
        Ok(())
    }
}

pub fn spawn(config: StreamConfig) -> (StreamHandle, mpsc::Receiver<StreamEvent>) {
    let (desired, watcher) = watch::channel(Subscriptions::new());
    let (events, receiver) = mpsc::channel(EVENT_BUFFER);
    tokio::spawn(run(config, watcher, events));
    (
        StreamHandle {
            desired: Arc::new(desired),
        },
        receiver,
    )
}

enum Exit {
    Stop,
    Fatal(String),
    Failed(String),
    Dropped(String),
}

async fn run(
    config: StreamConfig,
    mut desired: watch::Receiver<Subscriptions>,
    events: mpsc::Sender<StreamEvent>,
) {
    let mut delay = config.reconnect_initial;
    let mut dropped_at = None;
    loop {
        let reason = match session(&config, &mut desired, &events, &mut dropped_at).await {
            Exit::Stop => return,
            Exit::Fatal(reason) => {
                let _ = events.send(StreamEvent::Disconnected { reason }).await;
                return;
            }
            Exit::Dropped(reason) => {
                dropped_at = Some(now());
                delay = config.reconnect_initial;
                reason
            }
            Exit::Failed(reason) => reason,
        };
        tracing::warn!(%reason, ?delay, "alpaca stream down, reconnecting");
        if events
            .send(StreamEvent::Disconnected { reason })
            .await
            .is_err()
        {
            return;
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(config.reconnect_max);
    }
}

async fn session(
    config: &StreamConfig,
    desired: &mut watch::Receiver<Subscriptions>,
    events: &mpsc::Sender<StreamEvent>,
    dropped_at: &mut Option<Timestamp>,
) -> Exit {
    let mut socket = match establish(config).await {
        Ok(socket) => socket,
        Err(exit) => return exit,
    };
    let Err(exit) = serve(&mut socket, desired, events, dropped_at).await;
    exit
}

async fn establish(config: &StreamConfig) -> Result<Socket, Exit> {
    let handshake = async {
        let (mut socket, _) = connect_async(config.url.as_str())
            .await
            .map_err(|error| Exit::Failed(format!("connect: {error}")))?;
        expect_success(&mut socket, "connected").await?;
        send(
            &mut socket,
            json!({
                "action": "auth",
                "key": config.alpaca.key_id,
                "secret": config.alpaca.secret_key,
            }),
        )
        .await
        .map_err(Exit::Failed)?;
        expect_success(&mut socket, "authenticated").await?;
        Ok(socket)
    };
    timeout(AUTH_TIMEOUT, handshake)
        .await
        .unwrap_or_else(|_| Err(Exit::Failed("authentication timed out".to_owned())))
}

async fn expect_success(socket: &mut Socket, expected: &str) -> Result<(), Exit> {
    loop {
        for frame in next_frames(socket).await.map_err(Exit::Failed)? {
            match frame {
                Frame::Success { msg } if msg == expected => return Ok(()),
                Frame::Error { code, msg } if AUTH_FAILED_CODES.contains(&code) => {
                    return Err(Exit::Fatal(format!(
                        "authentication failed ({code}): {msg}"
                    )));
                }
                Frame::Error { code, msg } => {
                    return Err(Exit::Failed(format!("rejected ({code}): {msg}")));
                }
                _ => {}
            }
        }
    }
}

async fn serve(
    socket: &mut Socket,
    desired: &mut watch::Receiver<Subscriptions>,
    events: &mpsc::Sender<StreamEvent>,
    dropped_at: &mut Option<Timestamp>,
) -> Result<Infallible, Exit> {
    emit(events, StreamEvent::Connected).await?;
    desired.mark_changed();
    if let Some(dropped) = dropped_at.take()
        && !desired.borrow().is_empty()
    {
        let gap = StreamEvent::Gap {
            from: minute_floor(dropped) - TimeDelta::minutes(1),
            to: minute_floor(now()),
        };
        emit(events, gap).await?;
    }
    let mut acknowledged = Subscriptions::new();
    loop {
        tokio::select! {
            changed = desired.changed() => {
                if changed.is_err() {
                    return Err(Exit::Stop);
                }
                let wanted = desired.borrow_and_update().clone();
                reconcile(socket, &wanted, &acknowledged).await.map_err(Exit::Dropped)?;
            }
            frames = next_frames(socket) => {
                for frame in frames.map_err(Exit::Dropped)? {
                    match frame {
                        Frame::Bar(bar) => emit(events, StreamEvent::Bar(bar.into())).await?,
                        Frame::Subscription { bars } => {
                            acknowledged = bars.iter().cloned().collect();
                            emit(events, StreamEvent::Subscribed { symbols: bars }).await?;
                        }
                        Frame::Error { code: SYMBOL_LIMIT_CODE, .. } => {
                            emit(events, StreamEvent::SymbolLimitExceeded).await?;
                        }
                        Frame::Error { code, msg } => {
                            return Err(Exit::Dropped(format!("error ({code}): {msg}")));
                        }
                        Frame::Success { .. } | Frame::Other => {}
                    }
                }
            }
        }
    }
}

async fn reconcile(
    socket: &mut Socket,
    wanted: &Subscriptions,
    acknowledged: &Subscriptions,
) -> Result<(), String> {
    let additions: Vec<_> = wanted.difference(acknowledged).collect();
    let removals: Vec<_> = acknowledged.difference(wanted).collect();
    if !additions.is_empty() {
        send(socket, json!({"action": "subscribe", "bars": additions})).await?;
    }
    if !removals.is_empty() {
        send(socket, json!({"action": "unsubscribe", "bars": removals})).await?;
    }
    Ok(())
}

async fn send(socket: &mut Socket, payload: serde_json::Value) -> Result<(), String> {
    socket
        .send(Message::text(payload.to_string()))
        .await
        .map_err(|error| format!("send: {error}"))
}

async fn next_frames(socket: &mut Socket) -> Result<Vec<Frame>, String> {
    loop {
        match socket.next().await {
            None => return Err("connection closed".to_owned()),
            Some(Err(error)) => return Err(format!("read: {error}")),
            Some(Ok(Message::Close(_))) => return Err("closed by server".to_owned()),
            Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                Ok(frames) => return Ok(frames),
                Err(error) => tracing::warn!(%error, "undecodable alpaca stream frame"),
            },
            Some(Ok(_)) => {}
        }
    }
}

async fn emit(events: &mpsc::Sender<StreamEvent>, event: StreamEvent) -> Result<(), Exit> {
    events.send(event).await.map_err(|_| Exit::Stop)
}

fn now() -> Timestamp {
    Timestamp::from_datetime(Utc::now()).expect("current time fits in i64 nanoseconds")
}

fn minute_floor(at: Timestamp) -> Timestamp {
    Timestamp::from_nanos(at.nanos() - at.nanos().rem_euclid(MINUTE_NANOS))
}
