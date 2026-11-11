use std::time::Duration;

use buizz_core::market::{Bar, Symbol};
use buizz_core::time::Timestamp;
use tokio::sync::mpsc;

use crate::alpaca::AlpacaConfig;
use crate::http::SourceError;

pub const STREAM_URL: &str = "wss://stream.data.alpaca.markets/v2/iex";
pub const SYMBOL_CAP: usize = 30;
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamConfig {
    pub alpaca: AlpacaConfig,
    pub url: String,
    pub reconnect_initial: Duration,
    pub reconnect_max: Duration,
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
pub struct StreamHandle {}

impl StreamHandle {
    pub async fn subscribe(&self, _symbols: Vec<Symbol>) -> Result<(), SourceError> {
        unimplemented!()
    }

    pub async fn unsubscribe(&self, _symbols: Vec<Symbol>) -> Result<(), SourceError> {
        unimplemented!()
    }
}

pub fn spawn(_config: StreamConfig) -> (StreamHandle, mpsc::Receiver<StreamEvent>) {
    unimplemented!()
}
