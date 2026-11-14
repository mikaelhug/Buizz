use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::market::{Feed, Symbol};
use crate::money::{round_cash, round_price};
use crate::order::OrderId;
use crate::time::Timestamp;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LedgerEntry {
    pub seq: u64,
    pub at: Timestamp,
    pub kind: LedgerKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LedgerKind {
    Deposit {
        amount: Decimal,
    },
    Fill {
        order_id: OrderId,
        symbol: Symbol,
        qty: i64,
        price: Decimal,
        commission: Decimal,
        feed: Feed,
    },
    Dividend {
        symbol: Symbol,
        qty: i64,
        rate: Decimal,
        amount: Decimal,
    },
    Split {
        symbol: Symbol,
        old_rate: Decimal,
        new_rate: Decimal,
        qty_before: i64,
        qty_after: i64,
        cash_in_lieu: Decimal,
    },
    Reorganization {
        symbol: Symbol,
        qty_before: i64,
        qty_after: i64,
        new_symbol: Option<Symbol>,
        new_qty: i64,
        price: Decimal,
        cash: Decimal,
    },
    BorrowFee {
        symbol: Symbol,
        qty: i64,
        amount: Decimal,
    },
    Liquidation {
        symbol: Symbol,
        qty: i64,
        price: Decimal,
    },
    Financing {
        balance: Decimal,
        rate: Decimal,
        amount: Decimal,
    },
}

impl LedgerKind {
    pub fn symbol(&self) -> Option<&Symbol> {
        match self {
            Self::Deposit { .. } | Self::Financing { .. } => None,
            Self::Fill { symbol, .. }
            | Self::Dividend { symbol, .. }
            | Self::Split { symbol, .. }
            | Self::Reorganization { symbol, .. }
            | Self::BorrowFee { symbol, .. }
            | Self::Liquidation { symbol, .. } => Some(symbol),
        }
    }

    pub fn cash_delta(&self) -> Decimal {
        match self {
            Self::Deposit { amount } => *amount,
            Self::Fill {
                qty,
                price,
                commission,
                ..
            } => -round_cash(Decimal::from(*qty) * *price) - *commission,
            Self::Dividend { amount, .. } => *amount,
            Self::Split { cash_in_lieu, .. } => *cash_in_lieu,
            Self::Reorganization { cash, .. } => *cash,
            Self::BorrowFee { amount, .. } | Self::Financing { amount, .. } => -*amount,
            Self::Liquidation { qty, price, .. } => -round_cash(Decimal::from(*qty) * *price),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Position {
    pub qty: i64,
    pub cost_basis: Decimal,
    pub last_price: Decimal,
    pub realized_pnl: Decimal,
    #[serde(default)]
    pub opened_at: Option<Timestamp>,
}

impl Position {
    pub fn market_value(&self) -> Decimal {
        Decimal::from(self.qty) * self.last_price
    }

    pub fn average_cost(&self) -> Decimal {
        if self.qty == 0 {
            Decimal::ZERO
        } else {
            self.cost_basis / Decimal::from(self.qty)
        }
    }

    fn trade(&mut self, qty: i64, price: Decimal, at: Timestamp) {
        self.take(qty, Decimal::from(qty) * price, at);
    }

    fn take(&mut self, qty: i64, basis: Decimal, opened_at: Timestamp) {
        if self.qty == 0 || self.qty.signum() == qty.signum() {
            self.cost_basis += basis;
            self.qty += qty;
            self.opened_at = Some(self.opened_at.map_or(opened_at, |held| held.min(opened_at)));
            return;
        }
        let closing = qty.abs().min(self.qty.abs()) * qty.signum();
        let released = self.cost_basis * Decimal::from(-closing) / Decimal::from(self.qty);
        let used = basis * Decimal::from(closing) / Decimal::from(qty);
        self.realized_pnl -= used + released;
        self.cost_basis -= released;
        self.qty += closing;
        let remainder = qty - closing;
        if remainder != 0 {
            self.cost_basis = basis - used;
            self.qty = remainder;
            self.opened_at = Some(opened_at);
        } else if self.qty == 0 {
            self.opened_at = None;
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Portfolio {
    pub cash: Decimal,
    pub positions: BTreeMap<Symbol, Position>,
}

impl Portfolio {
    pub fn fold<'a>(entries: impl IntoIterator<Item = &'a LedgerEntry>) -> Self {
        let mut portfolio = Self::default();
        for entry in entries {
            portfolio.apply(entry.at, &entry.kind);
        }
        portfolio
    }

    pub fn apply(&mut self, at: Timestamp, kind: &LedgerKind) {
        self.cash += kind.cash_delta();
        match kind {
            LedgerKind::Deposit { .. } | LedgerKind::Financing { .. } => {}
            LedgerKind::Fill {
                symbol,
                qty,
                price,
                commission,
                ..
            } => {
                let position = self.positions.entry(symbol.clone()).or_default();
                position.trade(*qty, *price, at);
                position.realized_pnl -= *commission;
                if position.last_price.is_zero() {
                    position.last_price = *price;
                }
            }
            LedgerKind::Liquidation { symbol, qty, price } => {
                let position = self.positions.entry(symbol.clone()).or_default();
                position.trade(*qty, *price, at);
                position.last_price = *price;
            }
            LedgerKind::Dividend { symbol, amount, .. } => {
                self.positions
                    .entry(symbol.clone())
                    .or_default()
                    .realized_pnl += *amount;
            }
            LedgerKind::BorrowFee { symbol, amount, .. } => {
                self.positions
                    .entry(symbol.clone())
                    .or_default()
                    .realized_pnl -= *amount;
            }
            LedgerKind::Split {
                symbol,
                old_rate,
                new_rate,
                qty_before,
                qty_after,
                cash_in_lieu,
            } => {
                let position = self.positions.entry(symbol.clone()).or_default();
                if *qty_before != 0 {
                    let kept = Decimal::from(*qty_after) * *old_rate
                        / (*new_rate * Decimal::from(*qty_before));
                    let released = position.cost_basis - position.cost_basis * kept;
                    position.realized_pnl += *cash_in_lieu - released;
                    position.cost_basis -= released;
                }
                position.qty = *qty_after;
                if *qty_after == 0 {
                    position.opened_at = None;
                }
                position.last_price = round_price(position.last_price * *old_rate / *new_rate);
            }
            LedgerKind::Reorganization {
                symbol,
                qty_before,
                qty_after,
                new_symbol,
                new_qty,
                price,
                cash,
            } => {
                let transferred = Decimal::from(*new_qty) * *price;
                let position = self.positions.entry(symbol.clone()).or_default();
                let opened_at = position.opened_at.unwrap_or(at);
                let carried = (*qty_after == 0).then(|| {
                    let value = *cash + transferred;
                    let carried = if *new_qty == 0 {
                        Decimal::ZERO
                    } else if value.is_zero() {
                        position.cost_basis
                    } else {
                        position.cost_basis * transferred / value
                    };
                    position.realized_pnl += *cash + carried - position.cost_basis;
                    position.cost_basis = Decimal::ZERO;
                    position.opened_at = None;
                    carried
                });
                if carried.is_none() && *qty_before != 0 {
                    position.cost_basis -= transferred;
                    position.realized_pnl += *cash;
                }
                position.qty = *qty_after;
                if let Some(new_symbol) = new_symbol
                    && *new_qty != 0
                {
                    let received = self.positions.entry(new_symbol.clone()).or_default();
                    match carried {
                        Some(carried) => received.take(*new_qty, carried, opened_at),
                        None => received.trade(*new_qty, *price, at),
                    }
                    received.last_price = *price;
                }
            }
        }
        self.positions
            .retain(|_, position| position.qty != 0 || !position.realized_pnl.is_zero());
    }

    pub fn mark(&mut self, symbol: &Symbol, price: Decimal) {
        if let Some(position) = self.positions.get_mut(symbol) {
            position.last_price = price;
        }
    }

    pub fn qty(&self, symbol: &Symbol) -> i64 {
        self.positions
            .get(symbol)
            .map_or(0, |position| position.qty)
    }

    pub fn equity(&self) -> Decimal {
        self.cash
            + self
                .positions
                .values()
                .map(Position::market_value)
                .sum::<Decimal>()
    }

    pub fn gross_exposure(&self) -> Decimal {
        self.positions
            .values()
            .map(|position| position.market_value().abs())
            .sum()
    }

    pub fn open_positions(&self) -> impl Iterator<Item = (&Symbol, &Position)> {
        self.positions
            .iter()
            .filter(|(_, position)| position.qty != 0)
    }
}

pub fn split_qty(qty: i64, old_rate: Decimal, new_rate: Decimal) -> (i64, Decimal) {
    let exact = Decimal::from(qty) * new_rate / old_rate;
    let whole = exact.trunc();
    (i64::try_from(whole).unwrap_or(0), exact - whole)
}
