mod pump;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use buizz_core::api::{FeedConnection, FeedHolder, FeedSlots};
use buizz_core::market::{DataSource, Session, Symbol, Timeframe};
use buizz_core::run::RunId;
use buizz_core::time::Timestamp;
use buizz_market::{MarketError, MarketStore};
use buizz_sources::alpaca::AlpacaClient;
use buizz_sources::alpaca_stream::{SYMBOL_CAP, StreamConfig};
use buizz_sources::http::SourceError;
use chrono::NaiveDate;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use pump::LATE_BAR_GRACE;

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
    pub symbols_per_connection: usize,
    pub streams: Vec<StreamConfig>,
}

struct Leases {
    per_connection: usize,
    connections: usize,
    next_id: u64,
    held: BTreeMap<u64, FeedHolder>,
    placed: BTreeMap<Symbol, usize>,
    closed: BTreeSet<usize>,
}

impl Leases {
    fn new(per_connection: usize, connections: usize) -> Self {
        Self {
            per_connection,
            connections,
            next_id: 0,
            held: BTreeMap::new(),
            placed: BTreeMap::new(),
            closed: BTreeSet::new(),
        }
    }

    fn symbols(&self) -> BTreeSet<&Symbol> {
        self.held
            .values()
            .flat_map(|holder| &holder.symbols)
            .collect()
    }

    fn open_cap(&self) -> usize {
        self.per_connection * (self.connections - self.closed.len())
    }

    fn admit(&self, except: Option<u64>, symbols: &[Symbol]) -> Result<(), FeedError> {
        let others: BTreeSet<&Symbol> = self
            .held
            .iter()
            .filter(|(id, _)| Some(**id) != except)
            .flat_map(|(_, holder)| &holder.symbols)
            .collect();
        let requested = symbols
            .iter()
            .filter(|symbol| !others.contains(symbol))
            .count();
        let available = self.open_cap().saturating_sub(others.len());
        if requested > available {
            return Err(FeedError::CapExceeded {
                requested,
                available,
                holders: self.held.values().cloned().collect(),
            });
        }
        Ok(())
    }

    fn place(&mut self) {
        let wanted: BTreeSet<Symbol> = self.symbols().into_iter().cloned().collect();
        let closed = &self.closed;
        self.placed
            .retain(|symbol, connection| wanted.contains(symbol) && !closed.contains(connection));
        let mut used = self.used();
        for symbol in wanted {
            if self.placed.contains_key(&symbol) {
                continue;
            }
            let roomiest = (0..self.connections)
                .filter(|connection| !self.closed.contains(connection))
                .filter(|connection| used[*connection] < self.per_connection)
                .min_by_key(|connection| used[*connection]);
            if let Some(connection) = roomiest {
                used[connection] += 1;
                self.placed.insert(symbol, connection);
            }
        }
    }

    fn used(&self) -> Vec<usize> {
        let mut used = vec![0; self.connections];
        for connection in self.placed.values() {
            used[*connection] += 1;
        }
        used
    }

    fn placement(&self) -> Vec<BTreeSet<Symbol>> {
        let mut placement = vec![BTreeSet::new(); self.connections];
        for (symbol, connection) in &self.placed {
            placement[*connection].insert(symbol.clone());
        }
        placement
    }
}

fn distinct(symbols: impl IntoIterator<Item = Symbol>) -> Vec<Symbol> {
    symbols
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn count(value: usize) -> u32 {
    u32::try_from(value).expect("stream symbol counts fit in u32")
}

type SessionMinutes = BTreeMap<Symbol, (NaiveDate, Timestamp)>;

struct Inner {
    streams: Vec<StreamConfig>,
    market: MarketStore,
    alpaca: Option<AlpacaClient>,
    configured: bool,
    leases: Mutex<Leases>,
    session_minutes: tokio::sync::Mutex<SessionMinutes>,
    slots: watch::Sender<FeedSlots>,
}

impl Inner {
    fn leases(&self) -> std::sync::MutexGuard<'_, Leases> {
        self.leases.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn publish_leases(&self, leases: &mut Leases) {
        leases.place();
        let used = count(leases.symbols().len());
        let per_connection = leases.used();
        let holders: Vec<FeedHolder> = leases.held.values().cloned().collect();
        self.slots.send_if_modified(|slots| {
            let mut changed = slots.used != used || slots.holders != holders;
            for (view, used) in slots.connections.iter_mut().zip(&per_connection) {
                let used = count(*used);
                changed |= view.used != used;
                view.used = used;
            }
            slots.used = used;
            slots.holders = holders;
            changed
        });
    }

    fn placement(&self) -> Vec<BTreeSet<Symbol>> {
        self.leases().placement()
    }

    fn close_connection(&self, connection: usize) {
        let mut leases = self.leases();
        if leases.closed.insert(connection) {
            self.publish_leases(&mut leases);
        }
    }

    fn release(&self, id: u64) {
        let mut leases = self.leases();
        if leases.held.remove(&id).is_some() {
            self.publish_leases(&mut leases);
        }
    }
}

pub struct FeedLease {
    id: u64,
    symbols: Vec<Symbol>,
    inner: Arc<Inner>,
}

impl FeedLease {
    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    pub fn cover(&mut self, symbols: impl IntoIterator<Item = Symbol>) -> Result<(), FeedError> {
        let symbols = distinct(symbols);
        if symbols == self.symbols {
            return Ok(());
        }
        let mut leases = self.inner.leases();
        leases.admit(Some(self.id), &symbols)?;
        if let Some(holder) = leases.held.get_mut(&self.id) {
            holder.symbols = symbols.clone();
        }
        self.inner.publish_leases(&mut leases);
        self.symbols = symbols;
        Ok(())
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
        let per_connection = config.symbols_per_connection.min(SYMBOL_CAP);
        let connections = config.streams.len();
        let configured = connections > 0 && alpaca.is_some();
        let slots = FeedSlots {
            cap: count(per_connection * connections),
            used: 0,
            error: (!configured).then(|| FeedError::Unavailable.to_string()),
            connections: vec![
                FeedConnection {
                    cap: count(per_connection),
                    used: 0,
                    connected: false,
                    error: None,
                };
                connections
            ],
            holders: Vec::new(),
        };
        Self {
            inner: Arc::new(Inner {
                streams: config.streams,
                market,
                alpaca,
                configured,
                leases: Mutex::new(Leases::new(per_connection, connections)),
                session_minutes: tokio::sync::Mutex::default(),
                slots: watch::Sender::new(slots),
            }),
        }
    }

    pub fn admits(&self, symbols: impl IntoIterator<Item = Symbol>) -> Result<(), FeedError> {
        if !self.inner.configured {
            return Err(FeedError::Unavailable);
        }
        self.inner.leases().admit(None, &distinct(symbols))
    }

    pub fn acquire(
        &self,
        run_id: RunId,
        bot: &str,
        symbols: impl IntoIterator<Item = Symbol>,
    ) -> Result<FeedLease, FeedError> {
        if !self.inner.configured {
            return Err(FeedError::Unavailable);
        }
        let symbols = distinct(symbols);
        let mut leases = self.inner.leases();
        leases.admit(None, &symbols)?;
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
        self.inner.publish_leases(&mut leases);
        Ok(FeedLease {
            id,
            symbols,
            inner: Arc::clone(&self.inner),
        })
    }

    pub fn slots(&self) -> watch::Receiver<FeedSlots> {
        self.inner.slots.subscribe()
    }

    pub fn fetches_minutes(&self) -> Result<(), FeedError> {
        self.inner
            .alpaca
            .as_ref()
            .map(drop)
            .ok_or(FeedError::Unavailable)
    }

    pub async fn session_minutes(
        &self,
        symbols: impl IntoIterator<Item = Symbol>,
        session: &Session,
        through: Timestamp,
    ) -> Result<(), FeedError> {
        let alpaca = self.inner.alpaca.clone().ok_or(FeedError::Unavailable)?;
        let mut fetched = self.inner.session_minutes.lock().await;
        let covered = |symbol: &Symbol| {
            fetched
                .get(symbol)
                .is_some_and(|(date, until)| *date == session.date && *until >= through)
        };
        let missing: Vec<Symbol> = distinct(symbols)
            .into_iter()
            .filter(|symbol| !covered(symbol))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let market = &self.inner.market;
        let bars = pump::fetch_minutes(
            alpaca,
            market.clone(),
            missing.clone(),
            session.open,
            through,
        )
        .await?;
        market
            .upsert_bars(DataSource::Alpaca, Timeframe::Minute, bars)
            .await?;
        fetched.retain(|_, (date, _)| *date == session.date);
        fetched.extend(
            missing
                .into_iter()
                .map(|symbol| (symbol, (session.date, through))),
        );
        Ok(())
    }

    pub fn spawn(&self) -> JoinHandle<()> {
        let Some(alpaca) = self.inner.alpaca.clone().filter(|_| self.inner.configured) else {
            return tokio::spawn(async {});
        };
        let streams = self.inner.streams.clone();
        tokio::spawn(pump::run(Arc::clone(&self.inner), streams, alpaca))
    }
}
