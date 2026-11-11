mod pump;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use buizz_core::api::{FeedHolder, FeedSlots};
use buizz_core::market::Symbol;
use buizz_core::run::RunId;
use buizz_market::{MarketError, MarketStore};
use buizz_sources::alpaca::AlpacaClient;
use buizz_sources::alpaca_stream::{SYMBOL_CAP, StreamConfig};
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

#[derive(Default)]
struct Leases {
    next_id: u64,
    held: BTreeMap<u64, FeedHolder>,
}

impl Leases {
    fn symbols(&self) -> BTreeSet<&Symbol> {
        self.held
            .values()
            .flat_map(|holder| &holder.symbols)
            .collect()
    }
}

struct Inner {
    cap: usize,
    stream: Option<StreamConfig>,
    market: MarketStore,
    alpaca: Option<AlpacaClient>,
    configured: bool,
    leases: Mutex<Leases>,
    slots: watch::Sender<FeedSlots>,
}

impl Inner {
    fn leases(&self) -> std::sync::MutexGuard<'_, Leases> {
        self.leases.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn publish_leases(&self, leases: &Leases) {
        let used = u32::try_from(leases.symbols().len()).expect("cap fits in u32");
        let holders: Vec<FeedHolder> = leases.held.values().cloned().collect();
        self.slots.send_if_modified(|slots| {
            let changed = slots.used != used || slots.holders != holders;
            slots.used = used;
            slots.holders = holders;
            changed
        });
    }

    fn release(&self, id: u64) {
        let mut leases = self.leases();
        if leases.held.remove(&id).is_some() {
            self.publish_leases(&leases);
        }
    }
}

pub struct FeedLease {
    id: u64,
    run_id: RunId,
    symbols: Vec<Symbol>,
    inner: Arc<Inner>,
}

impl FeedLease {
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }
}

impl Drop for FeedLease {
    fn drop(&mut self) {
        self.inner.release(self.id);
    }
}

#[derive(Clone)]
pub struct FeedManager {
    inner: Arc<Inner>,
}

impl FeedManager {
    pub fn new(config: FeedConfig, market: MarketStore, alpaca: Option<AlpacaClient>) -> Self {
        let cap = config.cap.min(SYMBOL_CAP);
        let configured = config.stream.is_some() && alpaca.is_some();
        let slots = FeedSlots {
            cap: u32::try_from(cap).expect("cap fits in u32"),
            used: 0,
            connected: false,
            error: (!configured).then(|| FeedError::Unavailable.to_string()),
            holders: Vec::new(),
        };
        Self {
            inner: Arc::new(Inner {
                cap,
                stream: config.stream,
                market,
                alpaca,
                configured,
                leases: Mutex::default(),
                slots: watch::Sender::new(slots),
            }),
        }
    }

    pub async fn acquire(
        &self,
        run_id: RunId,
        bot: &str,
        symbols: Vec<Symbol>,
    ) -> Result<FeedLease, FeedError> {
        if !self.inner.configured {
            return Err(FeedError::Unavailable);
        }
        let symbols: Vec<Symbol> = symbols
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut leases = self.inner.leases();
        let held = leases.symbols();
        let requested = symbols
            .iter()
            .filter(|symbol| !held.contains(symbol))
            .count();
        let available = self.inner.cap.saturating_sub(held.len());
        if requested > available {
            return Err(FeedError::CapExceeded {
                requested,
                available,
                holders: leases.held.values().cloned().collect(),
            });
        }
        let id = leases.next_id;
        leases.next_id += 1;
        leases.held.insert(
            id,
            FeedHolder {
                run_id,
                bot: bot.to_owned(),
                symbols: symbols.clone(),
            },
        );
        self.inner.publish_leases(&leases);
        Ok(FeedLease {
            id,
            run_id,
            symbols,
            inner: Arc::clone(&self.inner),
        })
    }

    pub fn slots(&self) -> watch::Receiver<FeedSlots> {
        self.inner.slots.subscribe()
    }

    pub fn spawn(&self) -> JoinHandle<()> {
        let (Some(stream), Some(alpaca)) = (self.inner.stream.clone(), self.inner.alpaca.clone())
        else {
            return tokio::spawn(async {});
        };
        tokio::spawn(pump::run(Arc::clone(&self.inner), stream, alpaca))
    }
}
