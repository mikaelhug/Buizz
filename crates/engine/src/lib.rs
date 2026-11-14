use std::collections::{BTreeMap, BTreeSet};

use buizz_core::ledger::{LedgerEntry, LedgerKind, Portfolio, split_qty};
use buizz_core::market::{
    Bar, CorporateAction, CorporateActionKind, Feed, Instrument, Liquidity, Session, Symbol,
    Timeframe,
};
use buizz_core::money::{round_cash, round_price, round_price_with};
use buizz_core::order::{
    Fill, Order, OrderId, OrderRequest, OrderSize, OrderStatus, OrderType, OrderUpdate,
    RejectReason, Side, TimeInForce, Trail, target_weight_qty,
};
use buizz_core::protocol::{Holdings, StepEvent};
use buizz_core::run::{
    CostModel, DRAWDOWN_STOP, EquityPoint, MOC_CUTOFF_MINUTES, Mode, RATE_LOOKBACK_DAYS, RiskLimit,
    RiskLimits, RunConfig,
};
use buizz_core::time::Timestamp;
use chrono::{Datelike, NaiveDate, TimeDelta};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

pub const ENGINE_VERSION: u32 = 12;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine checkpoint is unreadable: {0}")]
    Checkpoint(#[from] rmp_serde::decode::Error),
    #[error(
        "engine checkpoint was written by engine {0}, this backtest cannot resume on engine {ENGINE_VERSION}"
    )]
    Version(u32),
}

pub struct Cycle<'a> {
    pub session: &'a Session,
    pub start: Timestamp,
    pub visible_at: Timestamp,
    pub bars: &'a [Bar],
    pub official: &'a [Bar],
    pub actions: &'a [CorporateAction],
    pub liquidity: &'a BTreeMap<Symbol, Liquidity>,
    pub delisted: &'a [Symbol],
    pub risk_free_rate: Option<Decimal>,
}

pub struct Decision<'a> {
    pub at: Timestamp,
    pub session: &'a Session,
    pub on_open: &'a Session,
    pub on_close: &'a Session,
}

impl Decision<'_> {
    fn auction_closed(&self) -> bool {
        self.at > self.on_close.close - TimeDelta::minutes(MOC_CUTOFF_MINUTES)
    }
}

fn locked_on_close(order: &Order, session: &Session, at: Timestamp) -> bool {
    order.order_type == OrderType::MarketOnClose
        && order.session == Some(session.date)
        && at > session.close - TimeDelta::minutes(MOC_CUTOFF_MINUTES)
        && at < session.close
}

struct Terms<'a> {
    delivered: Option<(&'a Symbol, Decimal, Decimal)>,
    cash_rate: Decimal,
}

fn action_name(kind: &CorporateActionKind) -> &'static str {
    match kind {
        CorporateActionKind::Split { .. } => "split",
        CorporateActionKind::CashDividend { .. } => "cash dividend",
        CorporateActionKind::StockDividend { .. } => "stock dividend",
        CorporateActionKind::SpinOff { .. } => "spin-off",
        CorporateActionKind::CashMerger { .. } => "cash merger",
        CorporateActionKind::StockMerger { .. } => "stock merger",
        CorporateActionKind::WorthlessRemoval => "worthless removal",
        CorporateActionKind::NameChange { .. } => "name change",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawdownStop {
    Closing,
    Flat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskBreach {
    pub limit: RiskLimit,
    pub value: Decimal,
    pub threshold: Decimal,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outbox {
    pub ledger: Vec<LedgerEntry>,
    pub orders: Vec<Order>,
    pub fills: Vec<Fill>,
    pub step_events: Vec<StepEvent>,
    pub equity_points: Vec<EquityPoint>,
    pub breaches: Vec<RiskBreach>,
    pub warnings: Vec<String>,
    pub rejections: Vec<Order>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    portfolio: Portfolio,
    last_close: BTreeMap<Symbol, Decimal>,
    working: BTreeMap<OrderId, Order>,
    #[serde(skip)]
    client_ids: BTreeSet<String>,
    orders_issued: u64,
    ledger_len: u64,
    peak_equity: Decimal,
    session: Option<Session>,
    session_filled: BTreeMap<Symbol, u64>,
    volume_unknown: BTreeSet<Symbol>,
    rate_missing: bool,
    breached: Vec<RiskLimit>,
    stopped_at: Option<Timestamp>,
    chain: [u8; 32],
    month: Option<(i32, u32)>,
    month_shares: u64,
    unmatched: Option<NaiveDate>,
    deferred_cancels: Vec<OrderId>,
}

#[derive(Serialize)]
struct CheckpointOut<'a> {
    version: u32,
    state: &'a State,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WrittenBy {
    Engine(u32),
    Workspace(serde::de::IgnoredAny),
}

#[derive(Deserialize)]
struct CheckpointHeader {
    version: WrittenBy,
}

#[derive(Deserialize)]
struct CheckpointIn {
    state: State,
}

pub struct Engine {
    timeframe: Timeframe,
    cost: CostModel,
    risk: RiskLimits,
    state: State,
    outbox: Outbox,
    changed_orders: BTreeMap<OrderId, Order>,
}

impl Engine {
    pub fn new(config: &RunConfig, start: Timestamp) -> Self {
        let start_cash = round_cash(config.start_cash);
        let mut engine = Self {
            timeframe: config.timeframe,
            cost: config.cost.clone(),
            risk: config.risk,
            state: State {
                peak_equity: start_cash,
                ..State::default()
            },
            outbox: Outbox::default(),
            changed_orders: BTreeMap::new(),
        };
        engine.post(start, LedgerKind::Deposit { amount: start_cash });
        engine
    }

    pub fn restore(
        config: &RunConfig,
        checkpoint: &[u8],
        client_ids: BTreeSet<String>,
    ) -> Result<Self, EngineError> {
        let header: CheckpointHeader = rmp_serde::from_slice(checkpoint)?;
        let written = match header.version {
            WrittenBy::Engine(version) => version,
            WrittenBy::Workspace(_) => 1,
        };
        let resumable =
            written == ENGINE_VERSION || (written < ENGINE_VERSION && config.mode == Mode::Live);
        if !resumable {
            return Err(EngineError::Version(written));
        }
        let CheckpointIn { mut state } = rmp_serde::from_slice(checkpoint)?;
        state.client_ids = client_ids;
        Ok(Self {
            timeframe: config.timeframe,
            cost: config.cost.clone(),
            risk: config.risk,
            state,
            outbox: Outbox::default(),
            changed_orders: BTreeMap::new(),
        })
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        rmp_serde::to_vec_named(&CheckpointOut {
            version: ENGINE_VERSION,
            state: &self.state,
        })
        .expect("engine state encodes as MessagePack")
    }

    pub fn pre_close(&mut self, cycle: &Cycle<'_>) {
        self.enter_session(cycle);
        for bar in cycle.bars {
            self.state.last_close.insert(bar.symbol.clone(), bar.close);
            self.state.portfolio.mark(&bar.symbol, bar.close);
        }
        self.state.unmatched = Some(cycle.session.date);
    }

    pub fn run_cycle(&mut self, cycle: &Cycle<'_>) {
        let bars: BTreeMap<&Symbol, &Bar> =
            cycle.bars.iter().map(|bar| (&bar.symbol, bar)).collect();
        self.enter_session(cycle);
        self.close_positions(cycle);
        self.fill(cycle, &bars);
        self.state.unmatched = None;
        for id in std::mem::take(&mut self.state.deferred_cancels) {
            self.close_where(cycle.visible_at, OrderStatus::Cancelled, |order| {
                order.id == id
            });
        }
        let session_closed = self.mark(cycle, &bars);
        self.check_limits(cycle.visible_at);
        if !session_closed && self.drawdown_stop() == Some(DrawdownStop::Flat) {
            self.record_equity(cycle.visible_at);
        }
    }

    pub fn drawdown_stop(&self) -> Option<DrawdownStop> {
        self.state.stopped_at?;
        let flat =
            self.state.portfolio.open_positions().next().is_none() && self.state.working.is_empty();
        Some(match flat {
            true => DrawdownStop::Flat,
            false => DrawdownStop::Closing,
        })
    }

    pub fn accept<'a>(
        &mut self,
        decision: &Decision<'_>,
        orders: Vec<OrderRequest>,
        cancels: Vec<String>,
        instrument: impl Fn(&Symbol) -> Result<&'a Instrument, RejectReason>,
    ) {
        let decided_at = decision.at;
        for client_id in &cancels {
            self.cancel(decision, client_id);
        }
        for request in orders {
            let duplicate = !self.state.client_ids.insert(request.client_id.clone());
            let known = instrument(&request.symbol);
            let session = match request.order_type {
                OrderType::MarketOnOpen => decision.on_open.date,
                OrderType::MarketOnClose => decision.on_close.date,
                _ => decision.session.date,
            };
            self.state.orders_issued += 1;
            let mut order = Order {
                id: OrderId(self.state.orders_issued),
                client_id: request.client_id,
                symbol: request.symbol,
                side: Side::Buy,
                qty: 0,
                filled_qty: 0,
                requested: request.size,
                order_type: request.order_type,
                tif: request.tif,
                reason: request.reason,
                decided_at,
                session: Some(session),
                status: OrderStatus::Working,
                reject_reason: None,
                updated_at: decided_at,
            };
            let sized = if duplicate {
                Err(RejectReason::DuplicateClientId)
            } else {
                let admitted = known.and_then(|instrument| {
                    self.admit(order.order_type, decision).map(|()| instrument)
                });
                self.size(&mut order, session, admitted)
            };
            if let Err(reason) = sized {
                order.status = OrderStatus::Rejected;
                order.reject_reason = Some(reason);
            }
            self.record(order);
        }
    }

    pub fn cancel_all(&mut self, at: Timestamp) {
        self.close_where(at, OrderStatus::Cancelled, |_| true);
    }

    pub fn drain(&mut self) -> Outbox {
        let mut outbox = std::mem::take(&mut self.outbox);
        outbox.orders = std::mem::take(&mut self.changed_orders)
            .into_values()
            .collect();
        outbox
    }

    pub fn quote(&mut self, closes: BTreeMap<Symbol, Decimal>) {
        for (symbol, close) in closes {
            self.state.last_close.insert(symbol, round_price(close));
        }
    }

    pub fn closing_auction(&self, session: NaiveDate) -> Vec<Symbol> {
        let symbols: BTreeSet<&Symbol> = self
            .state
            .working
            .values()
            .filter(|order| {
                order.order_type == OrderType::MarketOnClose
                    && order.session.is_some_and(|due| due <= session)
            })
            .map(|order| &order.symbol)
            .collect();
        symbols.into_iter().cloned().collect()
    }

    pub fn portfolio(&self) -> &Portfolio {
        &self.state.portfolio
    }

    pub fn date_positions(&mut self, ledger: &Portfolio) {
        for (symbol, position) in &mut self.state.portfolio.positions {
            position.opened_at = position.opened_at.or_else(|| {
                ledger
                    .positions
                    .get(symbol)
                    .and_then(|folded| folded.opened_at)
            });
        }
    }

    pub fn holdings(&self) -> Holdings {
        Holdings::new(&self.state.portfolio, self.state.working.values())
    }

    pub fn result_hash(&self) -> String {
        blake3::Hash::from_bytes(self.state.chain)
            .to_hex()
            .to_string()
    }

    fn enter_session(&mut self, cycle: &Cycle<'_>) {
        if self.state.session.map(|session| session.date) == Some(cycle.session.date) {
            return;
        }
        self.expire(cycle);
        self.apply_actions(cycle);
        self.delist(cycle);
        self.state.session = Some(*cycle.session);
        self.state.session_filled.clear();
        let month = (cycle.session.date.year(), cycle.session.date.month());
        if self.state.month != Some(month) {
            self.state.month = Some(month);
            self.state.month_shares = 0;
        }
    }

    fn cancel(&mut self, decision: &Decision<'_>, client_id: &str) {
        let Some(order) = self
            .state
            .working
            .values()
            .find(|order| order.client_id == client_id)
        else {
            return;
        };
        let id = order.id;
        if order.order_type == OrderType::MarketOnClose {
            if !locked_on_close(order, decision.on_close, decision.at) {
                self.close_where(decision.at, OrderStatus::Cancelled, |order| order.id == id);
            }
            return;
        }
        let matched_later = self
            .state
            .unmatched
            .is_some_and(|unmatched| order.session.is_none_or(|due| due <= unmatched));
        if !matched_later {
            self.close_where(decision.at, OrderStatus::Cancelled, |order| order.id == id);
        } else if !self.state.deferred_cancels.contains(&id) {
            self.state.deferred_cancels.push(id);
        }
    }

    fn admit(&self, order_type: OrderType, decision: &Decision<'_>) -> Result<(), RejectReason> {
        match order_type {
            OrderType::MarketOnOpen if self.timeframe == Timeframe::Minute => {
                Err(RejectReason::AuctionUnavailable)
            }
            OrderType::MarketOnClose if decision.auction_closed() => {
                Err(RejectReason::AuctionClosed)
            }
            OrderType::Limit { limit_price } if limit_price <= Decimal::ZERO => {
                Err(RejectReason::InvalidLimitPrice)
            }
            OrderType::Stop { stop_price } if stop_price <= Decimal::ZERO => {
                Err(RejectReason::InvalidStopPrice)
            }
            OrderType::TrailingStop { trail, .. } if !trail.is_valid() => {
                Err(RejectReason::InvalidStopPrice)
            }
            _ => Ok(()),
        }
    }

    fn apply_actions(&mut self, cycle: &Cycle<'_>) {
        for action in cycle.actions {
            if !action.kind.has_valid_rates() {
                self.outbox.warnings.push(format!(
                    "the {} {} on {} has rates that are not positive and was not applied",
                    action.symbol,
                    action_name(&action.kind),
                    action.ex_date
                ));
                continue;
            }
            self.outbox
                .step_events
                .push(StepEvent::CorporateAction(action.clone()));
            if let Some((old_rate, new_rate)) = action.kind.share_ratio() {
                self.split(cycle.start, &action.symbol, old_rate, new_rate);
                continue;
            }
            let closes = action.kind.ends_listing();
            if closes {
                self.close_where(cycle.start, OrderStatus::Cancelled, |order| {
                    order.symbol == action.symbol
                });
                self.state.last_close.remove(&action.symbol);
            }
            let held = self.state.portfolio.qty(&action.symbol);
            if held == 0 {
                continue;
            }
            let terms = match &action.kind {
                CorporateActionKind::CashDividend { rate } => {
                    self.post(
                        cycle.start,
                        LedgerKind::Dividend {
                            symbol: action.symbol.clone(),
                            qty: held,
                            rate: *rate,
                            amount: round_cash(Decimal::from(held) * *rate),
                        },
                    );
                    continue;
                }
                CorporateActionKind::SpinOff {
                    new_symbol,
                    old_rate,
                    new_rate,
                } => Terms {
                    delivered: Some((new_symbol, *old_rate, *new_rate)),
                    cash_rate: Decimal::ZERO,
                },
                CorporateActionKind::StockMerger {
                    acquirer,
                    old_rate,
                    new_rate,
                    cash_rate,
                } => Terms {
                    delivered: Some((acquirer, *old_rate, *new_rate)),
                    cash_rate: *cash_rate,
                },
                CorporateActionKind::CashMerger { rate } => Terms {
                    delivered: None,
                    cash_rate: *rate,
                },
                CorporateActionKind::WorthlessRemoval => Terms {
                    delivered: None,
                    cash_rate: Decimal::ZERO,
                },
                CorporateActionKind::NameChange { new_symbol } => Terms {
                    delivered: Some((new_symbol, Decimal::ONE, Decimal::ONE)),
                    cash_rate: Decimal::ZERO,
                },
                CorporateActionKind::Split { .. } | CorporateActionKind::StockDividend { .. } => {
                    continue;
                }
            };
            self.reorganize(cycle, action, held, if closes { 0 } else { held }, terms);
        }
    }

    fn reorganize(
        &mut self,
        cycle: &Cycle<'_>,
        action: &CorporateAction,
        held: i64,
        kept: i64,
        terms: Terms<'_>,
    ) {
        let mut cash = Decimal::from(held) * terms.cash_rate;
        let (mut new_symbol, mut new_qty, mut price) = (None, 0, Decimal::ZERO);
        if let Some((symbol, old_rate, new_rate)) = terms.delivered {
            let (whole, fraction) = split_qty(held, old_rate, new_rate);
            let open = cycle
                .bars
                .iter()
                .find(|bar| bar.symbol == *symbol)
                .map(|bar| bar.open)
                .or_else(|| self.state.last_close.get(symbol).copied());
            match open {
                Some(open) => {
                    price = round_price(open);
                    self.state.last_close.insert(symbol.clone(), price);
                }
                None => self.outbox.warnings.push(format!(
                    "{symbol} has no price at the open of {}, so the {whole} shares of it from the {} {} are held at no cost and their fraction is paid nothing",
                    cycle.session.date,
                    action.symbol,
                    action_name(&action.kind)
                )),
            }
            cash += fraction * price;
            new_symbol = Some(symbol.clone());
            new_qty = whole;
        }
        self.post(
            cycle.start,
            LedgerKind::Reorganization {
                symbol: action.symbol.clone(),
                qty_before: held,
                qty_after: kept,
                new_symbol,
                new_qty,
                price,
                cash: round_cash(cash),
            },
        );
    }

    fn split(&mut self, at: Timestamp, symbol: &Symbol, old_rate: Decimal, new_rate: Decimal) {
        let rescale = |price: Decimal| round_price(price * old_rate / new_rate);
        let shares = |qty: u64| {
            split_qty(i64::try_from(qty).unwrap_or(i64::MAX), old_rate, new_rate)
                .0
                .unsigned_abs()
        };
        if let Some(close) = self.state.last_close.get_mut(symbol) {
            *close = rescale(*close);
        }
        if let Some(position) = self.state.portfolio.positions.get(symbol)
            && position.qty != 0
        {
            let (qty_after, fraction) = split_qty(position.qty, old_rate, new_rate);
            let kind = LedgerKind::Split {
                symbol: symbol.clone(),
                old_rate,
                new_rate,
                qty_before: position.qty,
                qty_after,
                cash_in_lieu: round_cash(fraction * rescale(position.last_price)),
            };
            self.post(at, kind);
        }
        let affected: Vec<Order> = self
            .state
            .working
            .values()
            .filter(|order| order.symbol == *symbol)
            .cloned()
            .collect();
        for mut order in affected {
            order.qty = shares(order.qty);
            order.filled_qty = shares(order.filled_qty);
            match &mut order.order_type {
                OrderType::Limit { limit_price: price } | OrderType::Stop { stop_price: price } => {
                    *price = rescale(*price);
                }
                OrderType::TrailingStop { trail, stop_price } => {
                    *stop_price = stop_price.map(rescale);
                    if let Trail::Amount { amount } = trail {
                        *amount = rescale(*amount);
                    }
                }
                OrderType::Market | OrderType::MarketOnOpen | OrderType::MarketOnClose => {}
            }
            order.updated_at = at;
            self.record(order);
        }
    }

    fn delist(&mut self, cycle: &Cycle<'_>) {
        for symbol in cycle.delisted {
            if let Some(position) = self.state.portfolio.positions.get(symbol)
                && position.qty != 0
            {
                let kind = LedgerKind::Liquidation {
                    symbol: symbol.clone(),
                    qty: -position.qty,
                    price: round_price(position.last_price),
                };
                self.post(cycle.start, kind);
            }
            self.close_where(cycle.start, OrderStatus::Cancelled, |order| {
                order.symbol == *symbol
            });
            self.state.last_close.remove(symbol);
        }
    }

    fn expire(&mut self, cycle: &Cycle<'_>) {
        let at = self
            .state
            .session
            .map_or(cycle.start, |previous| previous.close);
        self.close_where(at, OrderStatus::Expired, |order| {
            order.tif == TimeInForce::Day
                && order
                    .session
                    .is_none_or(|session| session < cycle.session.date)
        });
    }

    fn close_positions(&mut self, cycle: &Cycle<'_>) {
        let Some(decided_at) = self.state.stopped_at else {
            return;
        };
        let covered: BTreeSet<&Symbol> = self
            .state
            .working
            .values()
            .filter(|order| order.order_type != OrderType::MarketOnClose)
            .map(|order| &order.symbol)
            .collect();
        let uncovered: Vec<(Symbol, i64)> = self
            .state
            .portfolio
            .open_positions()
            .filter(|(symbol, _)| !covered.contains(symbol))
            .map(|(symbol, position)| (symbol.clone(), -position.qty))
            .collect();
        for (symbol, qty) in uncovered {
            self.state.orders_issued += 1;
            let id = OrderId(self.state.orders_issued);
            let mut client_id = format!("drawdown-stop-{}", id.0);
            while !self.state.client_ids.insert(client_id.clone()) {
                client_id.push('+');
            }
            self.record(Order {
                id,
                client_id,
                symbol,
                side: Side::of_qty(qty),
                qty: qty.unsigned_abs(),
                filled_qty: 0,
                requested: OrderSize::Qty { qty },
                order_type: OrderType::Market,
                tif: TimeInForce::Day,
                reason: DRAWDOWN_STOP.to_owned(),
                decided_at,
                session: Some(cycle.session.date),
                status: OrderStatus::Working,
                reject_reason: None,
                updated_at: cycle.start,
            });
        }
    }

    fn fill(&mut self, cycle: &Cycle<'_>, bars: &BTreeMap<&Symbol, &Bar>) {
        let official: BTreeMap<&Symbol, &Bar> = cycle
            .official
            .iter()
            .map(|bar| (&bar.symbol, bar))
            .collect();
        if cycle.start == cycle.session.open {
            self.execute(
                cycle,
                cycle.session.open,
                |cost, order, slippage| match order.order_type {
                    OrderType::MarketOnOpen => official.get(&order.symbol).map(|bar| {
                        (
                            cost.market_fill_price(order.side, bar.open, slippage),
                            bar.feed,
                        )
                    }),
                    _ => None,
                },
            );
        }
        self.execute(cycle, cycle.start, |cost, order, slippage| {
            let bar = bars.get(&order.symbol)?;
            let price = match order.order_type {
                OrderType::Market => Some(cost.market_fill_price(order.side, bar.open, slippage)),
                OrderType::Limit { limit_price } => {
                    cost.limit_fill_price(order.side, limit_price, bar, slippage)
                }
                OrderType::Stop { stop_price }
                | OrderType::TrailingStop {
                    stop_price: Some(stop_price),
                    ..
                } => cost.stop_fill_price(order.side, stop_price, bar, slippage),
                _ => None,
            };
            price.map(|price| (price, bar.feed))
        });
        self.trail(cycle, bars);
        if cycle.session.is_closed_at(cycle.visible_at) {
            self.execute(
                cycle,
                cycle.session.close,
                |cost, order, slippage| match order.order_type {
                    OrderType::MarketOnClose => official.get(&order.symbol).map(|bar| {
                        (
                            cost.market_fill_price(order.side, bar.close, slippage),
                            bar.feed,
                        )
                    }),
                    _ => None,
                },
            );
        }
    }

    fn execute(
        &mut self,
        cycle: &Cycle<'_>,
        at: Timestamp,
        price: impl Fn(&CostModel, &Order, Decimal) -> Option<(Decimal, Feed)>,
    ) {
        let due: Vec<Order> = self
            .state
            .working
            .values()
            .filter(|order| order.session.is_some_and(|due| due <= cycle.session.date))
            .cloned()
            .collect();
        for mut order in due {
            let liquidity = cycle.liquidity.get(&order.symbol);
            let cap = self
                .cost
                .participation_cap(liquidity.map(|liquidity| liquidity.average_daily_volume));
            let used = self
                .state
                .session_filled
                .get(&order.symbol)
                .copied()
                .unwrap_or_default();
            let qty = order
                .remaining()
                .min(cap.map_or(u64::MAX, |cap| cap.saturating_sub(used)));
            if qty == 0 {
                continue;
            }
            let slippage = self.cost.slippage(liquidity, used, qty);
            let Some((price, feed)) = price(&self.cost, &order, slippage) else {
                continue;
            };
            if liquidity.is_none() && self.state.volume_unknown.insert(order.symbol.clone()) {
                self.outbox.warnings.push(format!(
                    "{} has no average daily volume, so its fills are not capped by volume",
                    order.symbol
                ));
            }
            self.state
                .session_filled
                .insert(order.symbol.clone(), used.saturating_add(qty));
            let commission = self.cost.commission(qty, self.state.month_shares);
            self.state.month_shares = self.state.month_shares.saturating_add(qty);
            let fill = Fill {
                order_id: order.id,
                client_id: order.client_id.clone(),
                symbol: order.symbol.clone(),
                at,
                qty: order.side.signed(qty),
                price,
                commission,
                feed,
            };
            self.post(
                at,
                LedgerKind::Fill {
                    order_id: fill.order_id,
                    symbol: fill.symbol.clone(),
                    qty: fill.qty,
                    price: fill.price,
                    commission: fill.commission,
                    feed: fill.feed,
                },
            );
            self.outbox.step_events.push(StepEvent::Fill(fill.clone()));
            self.outbox.fills.push(fill);
            order.filled_qty += qty;
            if order.remaining() == 0 {
                order.status = OrderStatus::Filled;
            }
            order.updated_at = at;
            self.record(order);
        }
    }

    fn trail(&mut self, cycle: &Cycle<'_>, bars: &BTreeMap<&Symbol, &Bar>) {
        for order in self.state.working.values_mut() {
            let OrderType::TrailingStop {
                trail,
                stop_price: Some(stop),
            } = &mut order.order_type
            else {
                continue;
            };
            let Some(bar) = bars.get(&order.symbol) else {
                continue;
            };
            if order.session.is_none_or(|due| due > cycle.session.date) {
                continue;
            }
            let (best, tighter): (Decimal, fn(&Decimal, &Decimal) -> bool) = match order.side {
                Side::Sell => (bar.high, Decimal::gt),
                Side::Buy => (bar.low, Decimal::lt),
            };
            let candidate = round_price(trail.stop_from(order.side, best));
            if tighter(&candidate, stop) {
                *stop = candidate;
                self.changed_orders.insert(order.id, order.clone());
            }
        }
    }
    fn mark(&mut self, cycle: &Cycle<'_>, bars: &BTreeMap<&Symbol, &Bar>) -> bool {
        for (symbol, bar) in bars {
            self.state.last_close.insert((*symbol).clone(), bar.close);
            self.state.portfolio.mark(symbol, bar.close);
        }
        let session_closed = cycle.session.is_closed_at(cycle.visible_at);
        if session_closed {
            let shorts: Vec<(Symbol, i64, Decimal)> = self
                .state
                .portfolio
                .open_positions()
                .filter(|(_, position)| position.qty < 0)
                .map(|(symbol, position)| (symbol.clone(), position.qty, position.last_price))
                .collect();
            for (symbol, qty, price) in shorts {
                let amount = self.cost.borrow_fee(qty.unsigned_abs(), price);
                if !amount.is_zero() {
                    self.post(
                        cycle.visible_at,
                        LedgerKind::BorrowFee {
                            symbol,
                            qty,
                            amount,
                        },
                    );
                }
            }
            let balance = self.state.portfolio.cash;
            if cycle.risk_free_rate.is_some() {
                self.state.rate_missing = false;
            } else if balance < Decimal::ZERO
                && !std::mem::replace(&mut self.state.rate_missing, true)
            {
                self.outbox.warnings.push(format!(
                    "no risk-free rate was published in the {RATE_LOOKBACK_DAYS} days before {}, so negative cash is financed at the spread alone until one is",
                    cycle.session.date
                ));
            }
            let rate = self
                .cost
                .financing_rate(cycle.risk_free_rate.unwrap_or_default());
            let amount = self.cost.financing_fee(balance, rate);
            if !amount.is_zero() {
                self.post(
                    cycle.visible_at,
                    LedgerKind::Financing {
                        balance,
                        rate: rate.normalize(),
                        amount,
                    },
                );
            }
        }
        self.state.peak_equity = self.state.peak_equity.max(self.state.portfolio.equity());
        if session_closed {
            self.record_equity(cycle.visible_at);
        }
        session_closed
    }

    fn record_equity(&mut self, at: Timestamp) {
        let portfolio = &self.state.portfolio;
        self.outbox.equity_points.push(EquityPoint {
            at,
            equity: portfolio.equity(),
            cash: portfolio.cash,
            gross_exposure: portfolio.gross_exposure(),
        });
    }

    fn check_limits(&mut self, at: Timestamp) {
        let portfolio = &self.state.portfolio;
        let equity = portfolio.equity();
        let peak = self.state.peak_equity;
        let mut readings = Vec::new();
        if equity > Decimal::ZERO {
            if let Some(threshold) = self.risk.max_position_weight {
                let value = portfolio
                    .positions
                    .values()
                    .map(|position| position.market_value().abs() / equity)
                    .max()
                    .unwrap_or_default();
                readings.push((
                    RiskLimit::MaxPositionWeight,
                    value,
                    threshold,
                    value > threshold,
                ));
            }
            if let Some(threshold) = self.risk.max_gross_exposure {
                let value = portfolio.gross_exposure() / equity;
                readings.push((
                    RiskLimit::MaxGrossExposure,
                    value,
                    threshold,
                    value > threshold,
                ));
            }
        }
        if let Some(threshold) = self.risk.max_drawdown
            && peak > Decimal::ZERO
            && self.state.stopped_at.is_none()
        {
            readings.push((
                RiskLimit::MaxDrawdown,
                (peak - equity) / peak,
                threshold,
                self.risk.drawdown_breached(peak, equity),
            ));
        }
        for (limit, value, threshold, breached) in readings {
            let known = self.state.breached.contains(&limit);
            if !breached {
                self.state.breached.retain(|breach| *breach != limit);
            } else if !known {
                self.state.breached.push(limit);
                self.outbox.breaches.push(RiskBreach {
                    limit,
                    value,
                    threshold,
                });
                if limit == RiskLimit::MaxDrawdown {
                    self.state.stopped_at = Some(at);
                    let session = self.state.session;
                    self.close_where(at, OrderStatus::Cancelled, |order| {
                        !session.is_some_and(|session| locked_on_close(order, &session, at))
                    });
                }
            }
        }
    }

    fn size(
        &self,
        order: &mut Order,
        session: NaiveDate,
        instrument: Result<&Instrument, RejectReason>,
    ) -> Result<(), RejectReason> {
        let instrument = instrument?;
        let price = self
            .state
            .last_close
            .get(&order.symbol)
            .copied()
            .filter(|price| *price > Decimal::ZERO)
            .filter(|_| instrument.last_session.is_none_or(|last| last >= session))
            .ok_or(RejectReason::NoPrice)?;
        let portfolio = &self.state.portfolio;
        let equity = portfolio.equity();
        let mut projected: BTreeMap<&Symbol, i64> = portfolio
            .open_positions()
            .map(|(symbol, position)| (symbol, position.qty))
            .collect();
        for working in self.state.working.values() {
            let contingent = matches!(
                working.order_type,
                OrderType::Stop { .. } | OrderType::TrailingStop { .. }
            );
            if !contingent && working.session.is_some_and(|due| due <= session) {
                let held = projected.entry(&working.symbol).or_default();
                *held = held
                    .checked_add(working.signed_remaining())
                    .ok_or(RejectReason::InvalidSize)?;
            }
        }
        let before = projected.get(&order.symbol).copied().unwrap_or(0);
        let delta = match order.requested {
            OrderSize::Qty { qty } => Some(qty),
            OrderSize::TargetWeight { weight } => target_weight_qty(weight, equity, price)
                .and_then(|target| target.checked_sub(before)),
        }
        .ok_or(RejectReason::InvalidSize)?;
        order.side = Side::of_qty(delta);
        order.qty = delta.unsigned_abs();
        if delta == 0 {
            return Err(RejectReason::ZeroQuantity);
        }
        match &mut order.order_type {
            OrderType::Limit { limit_price } => {
                let away_from_the_market = match order.side {
                    Side::Buy => RoundingStrategy::ToNegativeInfinity,
                    Side::Sell => RoundingStrategy::ToPositiveInfinity,
                };
                *limit_price = round_price_with(*limit_price, away_from_the_market);
                if *limit_price <= Decimal::ZERO {
                    return Err(RejectReason::InvalidLimitPrice);
                }
            }
            OrderType::Stop { stop_price } => {
                *stop_price = round_price(*stop_price);
                if *stop_price <= Decimal::ZERO {
                    return Err(RejectReason::InvalidStopPrice);
                }
            }
            OrderType::TrailingStop { trail, stop_price } => {
                let stop = round_price(trail.stop_from(order.side, price));
                if stop <= Decimal::ZERO {
                    return Err(RejectReason::InvalidStopPrice);
                }
                *stop_price = Some(stop);
            }
            OrderType::Market | OrderType::MarketOnOpen | OrderType::MarketOnClose => {}
        }
        let after = before.checked_add(delta).ok_or(RejectReason::InvalidSize)?;
        if delta < 0 && after < 0 && !instrument.shortable {
            return Err(RejectReason::NotShortable);
        }
        if after.abs() <= before.abs() {
            return Ok(());
        }
        let price_of = |symbol: &Symbol| {
            self.state
                .last_close
                .get(symbol)
                .or_else(|| portfolio.positions.get(symbol).map(|p| &p.last_price))
                .copied()
                .unwrap_or_default()
        };
        let gross_before: Decimal = projected
            .iter()
            .map(|(symbol, qty)| (Decimal::from(*qty) * price_of(symbol)).abs())
            .sum();
        let exposure_after = Decimal::from(after) * price;
        let gross_after =
            gross_before - (Decimal::from(before) * price).abs() + exposure_after.abs();
        self.risk.check_order(equity, exposure_after, gross_after)
    }

    fn close_where(
        &mut self,
        at: Timestamp,
        status: OrderStatus,
        matches: impl Fn(&Order) -> bool,
    ) {
        let closing: Vec<Order> = self
            .state
            .working
            .values()
            .filter(|order| matches(order))
            .cloned()
            .collect();
        for mut order in closing {
            order.status = status;
            order.updated_at = at;
            self.record(order);
        }
    }

    fn post(&mut self, at: Timestamp, kind: LedgerKind) {
        self.state.portfolio.apply(at, &kind);
        self.state.ledger_len += 1;
        let entry = LedgerEntry {
            seq: self.state.ledger_len,
            at,
            kind,
        };
        self.absorb(b"ledger", &entry);
        self.outbox.ledger.push(entry);
    }

    fn record(&mut self, order: Order) {
        self.absorb(b"order", &order);
        self.outbox
            .step_events
            .push(StepEvent::Order(OrderUpdate::from(&order)));
        if order.status == OrderStatus::Rejected {
            self.outbox.rejections.push(order.clone());
        }
        if order.status.is_terminal() {
            self.state.working.remove(&order.id);
        } else {
            self.state.working.insert(order.id, order.clone());
        }
        if order.reject_reason != Some(RejectReason::DuplicateClientId) {
            self.changed_orders.insert(order.id, order);
        }
    }

    fn absorb(&mut self, tag: &[u8], record: &impl Serialize) {
        let bytes = rmp_serde::to_vec_named(record).expect("engine records encode as MessagePack");
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.state.chain);
        hasher.update(tag);
        hasher.update(&bytes);
        self.state.chain = *hasher.finalize().as_bytes();
    }
}

#[cfg(test)]
mod tests {
    use buizz_core::market::Timeframe;
    use buizz_core::market::{AssetClass, Feed};
    use buizz_core::run::{UniverseSpec, Wake};
    use chrono::NaiveTime;

    use super::*;

    fn config(start_cash: &str, commission_min: &str) -> RunConfig {
        RunConfig {
            bot: "test".to_owned(),
            mode: Mode::Backtest,
            params: serde_json::Map::new(),
            universe: UniverseSpec::Symbols {
                symbols: vec![Symbol::new("AAA")],
            },
            timeframe: Timeframe::Day,
            wake: Wake::Bar,
            start: day(1),
            end: None,
            start_cash: start_cash.parse().unwrap(),
            cost: CostModel {
                commission_min: commission_min.parse().unwrap(),
                ..CostModel::default()
            },
            risk: RiskLimits::default(),
            seed: 1,
            benchmark: Symbol::new("SPY"),
            hedge_benchmark: None,
            holdout: false,
        }
    }

    fn day(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 11, day).unwrap()
    }

    fn session(date: u32) -> Session {
        let at = |hour, minute| {
            Timestamp::at_exchange_time(
                day(date),
                NaiveTime::from_hms_opt(hour, minute, 0).unwrap(),
            )
            .unwrap()
        };
        Session {
            date: day(date),
            open: at(9, 30),
            close: at(16, 0),
        }
    }

    fn bar(session: &Session, open: &str, low: &str, close: &str) -> Bar {
        Bar {
            symbol: Symbol::new("AAA"),
            start: session.open,
            open: open.parse().unwrap(),
            high: open.parse::<Decimal>().unwrap().max(close.parse().unwrap()),
            low: low.parse().unwrap(),
            close: close.parse().unwrap(),
            volume: 1_000_000,
            feed: Feed::Sip,
        }
    }

    fn step(engine: &mut Engine, session: &Session, bars: &[Bar], volume: &[(&str, u64)]) {
        let liquidity = volume
            .iter()
            .map(|(symbol, volume)| {
                let liquidity = Liquidity {
                    average_daily_volume: *volume,
                    ..Liquidity::default()
                };
                (Symbol::new(symbol), liquidity)
            })
            .collect();
        engine.run_cycle(&Cycle {
            session,
            start: session.open,
            visible_at: session.close,
            bars,
            official: bars,
            actions: &[],
            liquidity: &liquidity,
            delisted: &[],
            risk_free_rate: None,
        });
    }

    fn order(client_id: &str, size: OrderSize, order_type: OrderType) -> OrderRequest {
        OrderRequest {
            client_id: client_id.to_owned(),
            symbol: Symbol::new("AAA"),
            size,
            order_type,
            tif: TimeInForce::Day,
            reason: "test".to_owned(),
        }
    }

    fn buy(client_id: &str, qty: i64) -> OrderRequest {
        order(client_id, OrderSize::Qty { qty }, OrderType::Market)
    }

    fn submit(engine: &mut Engine, session: &Session, orders: Vec<OrderRequest>) -> Vec<Order> {
        let instrument = Instrument {
            symbol: Symbol::new("AAA"),
            name: "AAA".to_owned(),
            exchange: "NYSE".to_owned(),
            asset_class: AssetClass::Stock,
            active: true,
            shortable: false,
            first_session: None,
            last_session: None,
        };
        let known = |symbol: &Symbol| {
            (*symbol == instrument.symbol)
                .then_some(&instrument)
                .ok_or(RejectReason::UnknownSymbol)
        };
        let next = self::session(session.date.day() + 1);
        let decision = Decision {
            at: session.close,
            session: &next,
            on_open: &next,
            on_close: &next,
        };
        engine.accept(&decision, orders, Vec::new(), known);
        engine.drain().orders
    }

    #[test]
    fn identical_runs_hash_the_same_whatever_the_decimal_scale() {
        let hash = |start_cash, commission_min, financing_spread: &str| {
            let (first, second) = (session(20), session(21));
            let config = RunConfig {
                cost: CostModel {
                    financing_spread: financing_spread.parse().unwrap(),
                    ..config(start_cash, commission_min).cost
                },
                ..config(start_cash, commission_min)
            };
            let mut engine = Engine::new(&config, first.open);
            step(
                &mut engine,
                &first,
                &[bar(&first, "100", "99", "100")],
                &[("AAA", 1_000_000)],
            );
            submit(&mut engine, &first, vec![buy("AAA-1", 1500)]);
            step(
                &mut engine,
                &second,
                &[bar(&second, "101", "100", "102")],
                &[("AAA", 1_000_000)],
            );
            assert!(engine.portfolio().cash < Decimal::ZERO);
            engine.result_hash()
        };
        assert_eq!(
            hash("100000", "1", "0.015"),
            hash("100000.00", "1.00", "0.01500")
        );
    }

    #[test]
    fn a_limit_order_marketable_at_the_open_pays_the_slippage() {
        let (first, second) = (session(20), session(21));
        let mut engine = Engine::new(&config("100000", "1"), first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        let limit = OrderType::Limit {
            limit_price: Decimal::from(105),
        };
        submit(
            &mut engine,
            &first,
            vec![order("AAA-1", OrderSize::Qty { qty: 10 }, limit)],
        );
        step(
            &mut engine,
            &second,
            &[bar(&second, "100", "99", "101")],
            &[("AAA", 1_000_000)],
        );
        let fills = engine.drain().fills;
        assert_eq!(fills[0].price, "100.0500".parse::<Decimal>().unwrap());
    }

    #[test]
    fn a_limit_price_is_rounded_away_from_the_market_before_it_fills() {
        let (first, second) = (session(20), session(21));
        let mut engine = Engine::new(&config("100000", "1"), first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        let limit = OrderType::Limit {
            limit_price: "99.123456".parse().unwrap(),
        };
        submit(
            &mut engine,
            &first,
            vec![order("AAA-1", OrderSize::Qty { qty: 10 }, limit)],
        );
        step(
            &mut engine,
            &second,
            &[bar(&second, "100", "99", "101")],
            &[("AAA", 1_000_000)],
        );
        let fills = engine.drain().fills;
        assert_eq!(fills[0].price.to_string(), "99.1234");
    }

    #[test]
    fn an_order_from_the_previous_session_expires_before_a_split_rescales_it() {
        let (first, second, third) = (session(20), session(21), session(22));
        let mut engine = Engine::new(&config("100000", "1"), first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        let limit = OrderType::Limit {
            limit_price: Decimal::from(50),
        };
        submit(
            &mut engine,
            &first,
            vec![order("AAA-1", OrderSize::Qty { qty: 10 }, limit)],
        );
        step(
            &mut engine,
            &second,
            &[bar(&second, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        engine.drain();
        let split = CorporateAction {
            symbol: Symbol::new("AAA"),
            ex_date: third.date,
            kind: CorporateActionKind::Split {
                old_rate: Decimal::ONE,
                new_rate: Decimal::TWO,
            },
        };
        engine.run_cycle(&Cycle {
            session: &third,
            start: third.open,
            visible_at: third.close,
            bars: &[bar(&third, "50", "49", "50")],
            official: &[],
            actions: &[split],
            liquidity: &BTreeMap::new(),
            delisted: &[],
            risk_free_rate: None,
        });
        let expired = &engine.drain().orders[0];
        assert_eq!(expired.status, OrderStatus::Expired);
        assert_eq!(expired.qty, 10);
        assert_eq!(expired.order_type, limit);
    }

    fn hold_through(kind: CorporateActionKind) -> (Engine, Portfolio, Outbox) {
        let (first, second, third) = (session(20), session(21), session(22));
        let mut engine = Engine::new(&config("100000", "0"), first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        submit(&mut engine, &first, vec![buy("AAA-1", 101)]);
        step(
            &mut engine,
            &second,
            &[bar(&second, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        let limit = OrderType::Limit {
            limit_price: Decimal::from(90),
        };
        submit(
            &mut engine,
            &second,
            vec![order("AAA-2", OrderSize::Qty { qty: -101 }, limit)],
        );
        engine.drain();
        let before = engine.portfolio().clone();
        let action = CorporateAction {
            symbol: Symbol::new("AAA"),
            ex_date: third.date,
            kind,
        };
        let delivered = Bar {
            symbol: Symbol::new("BBB"),
            ..bar(&third, "30", "29", "31")
        };
        engine.run_cycle(&Cycle {
            session: &third,
            start: third.open,
            visible_at: third.close,
            bars: &[bar(&third, "60", "59", "60"), delivered],
            official: &[],
            actions: &[action],
            liquidity: &BTreeMap::new(),
            delisted: &[],
            risk_free_rate: None,
        });
        let outbox = engine.drain();
        (engine, before, outbox)
    }

    #[test]
    fn corporate_actions_settle_held_shares_on_their_terms() {
        let dec = |text: &str| text.parse::<Decimal>().unwrap();
        let (aaa, bbb) = (Symbol::new("AAA"), Symbol::new("BBB"));

        let (engine, Portfolio { cash, .. }, _) =
            hold_through(CorporateActionKind::StockDividend { rate: dec("0.05") });
        assert_eq!(engine.portfolio().qty(&aaa), 106, "101 × 1.05 = 106.05");
        assert_eq!(
            engine.portfolio().cash - cash,
            dec("4.76"),
            "0.05 × 100 / 1.05"
        );

        let (engine, Portfolio { cash, .. }, _) = hold_through(CorporateActionKind::SpinOff {
            new_symbol: bbb.clone(),
            old_rate: dec("3"),
            new_rate: dec("2"),
        });
        let portfolio = engine.portfolio();
        assert_eq!(portfolio.qty(&aaa), 101);
        assert_eq!(portfolio.qty(&bbb), 67, "101 × 2 / 3 = 67.33");
        assert_eq!(
            portfolio.cash - cash,
            dec("10.00"),
            "0.33 × 30 at BBB's open"
        );
        assert_eq!(portfolio.positions[&bbb].cost_basis, dec("2010"), "67 × 30");
        assert_eq!(
            portfolio.equity(),
            cash + dec("10.00") + Decimal::from(101 * 60 + 67 * 31)
        );

        let (engine, Portfolio { cash, .. }, outbox) =
            hold_through(CorporateActionKind::CashMerger { rate: dec("125") });
        assert_eq!(engine.portfolio().qty(&aaa), 0);
        assert_eq!(engine.portfolio().cash - cash, dec("12625.00"));
        assert!(
            outbox
                .orders
                .iter()
                .any(|order| order.client_id == "AAA-2" && order.status == OrderStatus::Cancelled),
            "a working order for the acquired company is cancelled"
        );

        let (engine, before, _) = hold_through(CorporateActionKind::StockMerger {
            acquirer: bbb.clone(),
            old_rate: dec("1"),
            new_rate: dec("0.5"),
            cash_rate: dec("10"),
        });
        let (portfolio, held) = (engine.portfolio(), &before.positions[&aaa]);
        assert_eq!(portfolio.qty(&aaa), 0);
        assert_eq!(portfolio.qty(&bbb), 50, "101 × 0.5 = 50.5");
        assert_eq!(
            portfolio.cash - before.cash,
            dec("1025.00"),
            "101 × 10 + 0.5 × 30"
        );
        let carried = held.cost_basis * dec("1500") / dec("2525");
        assert_eq!(
            portfolio.positions[&bbb].cost_basis, carried,
            "the acquirer's shares keep the part of the cost the 50 × 30 of stock is worth against the 1025 in cash"
        );
        assert_eq!(
            portfolio.positions[&aaa].realized_pnl - held.realized_pnl,
            dec("1025.00") + carried - held.cost_basis
        );
        assert_eq!(portfolio.positions[&bbb].opened_at, held.opened_at);

        let (engine, Portfolio { cash, .. }, _) =
            hold_through(CorporateActionKind::WorthlessRemoval);
        assert_eq!(engine.portfolio().qty(&aaa), 0);
        assert_eq!(engine.portfolio().cash, cash);
        assert_eq!(engine.portfolio().equity(), cash);

        let (engine, before, _) = hold_through(CorporateActionKind::NameChange {
            new_symbol: bbb.clone(),
        });
        let (portfolio, held) = (engine.portfolio(), &before.positions[&aaa]);
        assert_eq!(portfolio.qty(&aaa), 0, "the old ticker no longer trades");
        assert_eq!(portfolio.qty(&bbb), 101, "the renamed shares are held");
        assert_eq!(portfolio.cash, before.cash);
        assert_eq!(portfolio.equity(), before.cash + Decimal::from(101 * 31));
        assert_eq!(
            portfolio.positions[&aaa].realized_pnl, held.realized_pnl,
            "a rename realises nothing"
        );
        let renamed = &engine.holdings().positions[0];
        assert_eq!(
            (renamed.avg_cost, Some(renamed.opened_at)),
            (held.average_cost(), held.opened_at),
            "the renamed position keeps the cost and the opening time it was bought at, not BBB's open"
        );
    }

    #[test]
    fn an_order_without_an_average_daily_volume_fills_uncapped_and_warns_once() {
        let (first, second) = (session(20), session(21));
        let mut engine = Engine::new(&config("100000", "1"), first.open);
        step(&mut engine, &first, &[bar(&first, "100", "99", "100")], &[]);
        submit(
            &mut engine,
            &first,
            vec![buy("AAA-1", 10), buy("AAA-2", 10)],
        );
        step(
            &mut engine,
            &second,
            &[bar(&second, "100", "99", "101")],
            &[],
        );
        let outbox = engine.drain();
        assert_eq!(outbox.fills.len(), 2);
        assert_eq!(outbox.warnings.len(), 1);
    }

    #[test]
    fn sizes_that_overflow_are_rejected_instead_of_panicking() {
        let first = session(20);
        let mut engine = Engine::new(&config("100000", "1"), first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        let huge = OrderSize::TargetWeight {
            weight: Decimal::MAX,
        };
        let orders = submit(
            &mut engine,
            &first,
            vec![
                order("AAA-1", huge, OrderType::Market),
                buy("AAA-2", i64::MAX),
                buy("AAA-3", i64::MAX),
            ],
        );
        let reasons: Vec<_> = orders.iter().map(|order| order.reject_reason).collect();
        assert_eq!(
            reasons,
            [
                Some(RejectReason::InvalidSize),
                None,
                Some(RejectReason::InvalidSize)
            ]
        );
    }

    #[test]
    fn unknown_and_unshortable_orders_are_rejected_as_orders() {
        let first = session(20);
        let mut engine = Engine::new(&config("100000", "1"), first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        let mut unknown = buy("ZZZ-1", 10);
        unknown.symbol = Symbol::new("ZZZ");
        let orders = submit(&mut engine, &first, vec![unknown, buy("AAA-1", -10)]);
        let reasons: Vec<_> = orders.iter().map(|order| order.reject_reason).collect();
        assert_eq!(
            reasons,
            [
                Some(RejectReason::UnknownSymbol),
                Some(RejectReason::NotShortable)
            ]
        );
    }

    #[test]
    fn an_order_for_a_session_after_the_symbols_last_one_has_no_price() {
        let (first, second) = (session(20), session(21));
        let mut engine = Engine::new(&config("100000", "1"), first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        let delisted = Instrument {
            symbol: Symbol::new("AAA"),
            name: "AAA".to_owned(),
            exchange: "NYSE".to_owned(),
            asset_class: AssetClass::Stock,
            active: false,
            shortable: true,
            first_session: None,
            last_session: Some(first.date),
        };
        let decision = Decision {
            at: first.close - TimeDelta::minutes(30),
            session: &second,
            on_open: &second,
            on_close: &first,
        };
        let on_close = order(
            "AAA-close",
            OrderSize::Qty { qty: 10 },
            OrderType::MarketOnClose,
        );
        engine.accept(
            &decision,
            vec![buy("AAA-next", 10), on_close],
            Vec::new(),
            |_| Ok(&delisted),
        );
        let reasons: Vec<_> = engine
            .drain()
            .orders
            .iter()
            .map(|order| order.reject_reason)
            .collect();
        assert_eq!(reasons, [Some(RejectReason::NoPrice), None]);
    }

    #[test]
    fn a_repeated_client_id_is_rejected_without_touching_the_first_order() {
        let first = session(20);
        let mut engine = Engine::new(&config("100000", "1"), first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        submit(&mut engine, &first, vec![buy("AAA-1", 10)]);
        let next = session(21);
        let decision = Decision {
            at: first.close,
            session: &next,
            on_open: &next,
            on_close: &next,
        };
        engine.accept(&decision, vec![buy("AAA-1", 10)], Vec::new(), |_| {
            Err(RejectReason::UnknownSymbol)
        });
        let rejected = &engine.drain().rejections[0];
        assert_eq!(
            rejected.reject_reason,
            Some(RejectReason::DuplicateClientId)
        );
        assert_eq!(engine.holdings().working.len(), 1);
    }

    #[test]
    fn a_drawdown_stop_after_the_cutoff_keeps_the_locked_market_on_close() {
        let (first, second) = (session(20), session(21));
        let mut config = config("100000", "1");
        config.risk.max_drawdown = Some("0.1".parse().unwrap());
        let mut engine = Engine::new(&config, first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        submit(
            &mut engine,
            &first,
            vec![
                buy("AAA-1", 500),
                order(
                    "AAA-2",
                    OrderSize::Qty { qty: 10 },
                    OrderType::MarketOnClose,
                ),
            ],
        );
        let crash = bar(&second, "100", "40", "50");
        engine.run_cycle(&Cycle {
            session: &second,
            start: second.open,
            visible_at: second.close - TimeDelta::minutes(5),
            bars: std::slice::from_ref(&crash),
            official: &[],
            actions: &[],
            liquidity: &BTreeMap::new(),
            delisted: &[],
            risk_free_rate: None,
        });
        assert_eq!(engine.drawdown_stop(), Some(DrawdownStop::Closing));
        let working = engine.holdings().working;
        assert_eq!(working.len(), 1);
        assert_eq!(working[0].order_type, OrderType::MarketOnClose);
    }

    #[test]
    fn a_checkpoint_leaves_client_ids_to_the_orders_and_a_restored_engine_still_rejects_repeats() {
        let first = session(20);
        let config = config("100000", "1");
        let mut engine = Engine::new(&config, first.open);
        step(
            &mut engine,
            &first,
            &[bar(&first, "100", "99", "100")],
            &[("AAA", 1_000_000)],
        );
        let placed = submit(
            &mut engine,
            &first,
            vec![buy("client-id-kept-by-orders", 1)],
        );
        engine.cancel_all(first.close);
        let checkpoint = engine.checkpoint();
        assert!(
            !checkpoint
                .windows(placed[0].client_id.len())
                .any(|window| window == placed[0].client_id.as_bytes())
        );
        let ids = placed.into_iter().map(|order| order.client_id).collect();
        let mut restored = Engine::restore(&config, &checkpoint, ids).unwrap();
        let decision = Decision {
            at: first.close,
            session: &first,
            on_open: &first,
            on_close: &first,
        };
        restored.accept(
            &decision,
            vec![buy("client-id-kept-by-orders", 1)],
            Vec::new(),
            |_| Err(RejectReason::UnknownSymbol),
        );
        assert_eq!(
            restored.drain().rejections[0].reject_reason,
            Some(RejectReason::DuplicateClientId)
        );
    }

    #[test]
    fn a_live_run_resumes_from_an_older_engine_checkpoint_and_a_backtest_refuses() {
        #[derive(Serialize)]
        struct Legacy<'a> {
            version: &'a str,
            state: &'a State,
        }
        let first = session(20);
        let backtest = config("100000", "1");
        let live = RunConfig {
            mode: Mode::Live,
            ..backtest.clone()
        };
        let engine = Engine::new(&live, first.open);
        let checkpoint = rmp_serde::to_vec_named(&Legacy {
            version: "0.1.0",
            state: &engine.state,
        })
        .unwrap();
        let resumed = Engine::restore(&live, &checkpoint, BTreeSet::new()).unwrap();
        assert_eq!(resumed.result_hash(), engine.result_hash());
        assert!(matches!(
            Engine::restore(&backtest, &checkpoint, BTreeSet::new()),
            Err(EngineError::Version(1))
        ));
    }
}
