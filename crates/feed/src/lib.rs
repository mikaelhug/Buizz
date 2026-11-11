use buizz_core::api::{FeedHolder, FeedSlots};
use buizz_core::market::Symbol;
use buizz_core::run::RunId;
use buizz_market::{MarketError, MarketStore};
use buizz_sources::alpaca::AlpacaClient;
use buizz_sources::alpaca_stream::StreamConfig;
use buizz_sources::http::SourceError;
use tokio::sync::watch;
use tokio::task::JoinHandle;

#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("stream symbol cap exceeded: {requested} requested, {available} free")]
    CapExceeded {
        requested: usize,
        available: usize,
        holders: Vec<FeedHolder>,
    },
    #[error("live feed is not configured")]
    Unavailable,
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Market(#[from] MarketError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedConfig {
    pub cap: usize,
    pub stream: Option<StreamConfig>,
}

pub struct FeedLease {}

impl FeedLease {
    pub fn run_id(&self) -> RunId {
        unimplemented!()
    }

    pub fn symbols(&self) -> &[Symbol] {
        unimplemented!()
    }
}

#[derive(Clone)]
pub struct FeedManager {}

impl FeedManager {
    pub fn new(_config: FeedConfig, _market: MarketStore, _alpaca: Option<AlpacaClient>) -> Self {
        unimplemented!()
    }

    pub async fn acquire(
        &self,
        _run_id: RunId,
        _bot: &str,
        _symbols: Vec<Symbol>,
    ) -> Result<FeedLease, FeedError> {
        unimplemented!()
    }

    pub fn slots(&self) -> watch::Receiver<FeedSlots> {
        unimplemented!()
    }

    pub fn spawn(&self) -> JoinHandle<()> {
        unimplemented!()
    }
}
