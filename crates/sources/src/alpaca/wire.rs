use std::collections::BTreeMap;

use buizz_core::market::{
    AssetClass, Bar, Calendar, CorporateAction, CorporateActionKind, Feed, Instrument, NewsArticle,
    Session, Symbol, Timeframe,
};
use buizz_core::time::Timestamp;
use chrono::{NaiveDate, NaiveTime};
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::http::{SourceError, decimal};

pub trait Paged {
    fn take_next_page_token(&mut self) -> Option<String>;
}

#[derive(Deserialize)]
pub struct BarsPage {
    bars: Option<BTreeMap<String, Vec<WireBar>>>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct WireBar {
    t: Timestamp,
    #[serde(deserialize_with = "decimal")]
    o: Decimal,
    #[serde(deserialize_with = "decimal")]
    h: Decimal,
    #[serde(deserialize_with = "decimal")]
    l: Decimal,
    #[serde(deserialize_with = "decimal")]
    c: Decimal,
    v: u64,
}

impl Paged for BarsPage {
    fn take_next_page_token(&mut self) -> Option<String> {
        self.next_page_token.take()
    }
}

impl BarsPage {
    pub fn into_bars(self, timeframe: Timeframe, feed: Feed, calendar: &Calendar) -> Vec<Bar> {
        self.bars
            .unwrap_or_default()
            .into_iter()
            .flat_map(|(symbol, bars)| {
                let symbol = Symbol::new(&symbol);
                bars.into_iter().filter_map(move |bar| {
                    let session = calendar.session(bar.t.exchange_date())?;
                    let start = match timeframe {
                        Timeframe::Day => session.open,
                        Timeframe::Minute if (session.open..session.close).contains(&bar.t) => {
                            bar.t
                        }
                        Timeframe::Minute => return None,
                    };
                    Some(Bar {
                        symbol: symbol.clone(),
                        start,
                        open: bar.o,
                        high: bar.h,
                        low: bar.l,
                        close: bar.c,
                        volume: bar.v,
                        feed,
                    })
                })
            })
            .collect()
    }
}

#[derive(Deserialize)]
pub struct WireCalendarDay {
    date: NaiveDate,
    open: String,
    close: String,
}

impl WireCalendarDay {
    pub fn into_session(self) -> Result<Session, SourceError> {
        let at = |text: &str| {
            NaiveTime::parse_from_str(text, "%H:%M")
                .ok()
                .and_then(|time| Timestamp::at_exchange_time(self.date, time))
                .ok_or_else(|| {
                    SourceError::Decode(format!("calendar time {text:?} on {}", self.date))
                })
        };
        Ok(Session {
            date: self.date,
            open: at(&self.open)?,
            close: at(&self.close)?,
        })
    }
}

#[derive(Deserialize)]
pub struct WireAsset {
    class: String,
    exchange: String,
    symbol: String,
    name: String,
    status: String,
    shortable: bool,
}

impl WireAsset {
    pub fn into_instrument(self) -> Option<Instrument> {
        if self.class != "us_equity" {
            return None;
        }
        Some(Instrument {
            symbol: Symbol::new(&self.symbol),
            name: self.name,
            exchange: self.exchange,
            asset_class: AssetClass::Stock,
            active: self.status == "active",
            shortable: self.shortable,
            first_session: None,
            last_session: None,
        })
    }
}

#[derive(Deserialize)]
pub struct ActionsPage {
    #[serde(default)]
    corporate_actions: ActionLists,
    next_page_token: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ActionLists {
    forward_splits: Vec<WireSplit>,
    reverse_splits: Vec<WireSplit>,
    cash_dividends: Vec<WireRate>,
    stock_dividends: Vec<WireRate>,
    spin_offs: Vec<WireSpinOff>,
    cash_mergers: Vec<WireCashMerger>,
    stock_mergers: Vec<WireStockMerger>,
    stock_and_cash_mergers: Vec<WireStockMerger>,
    worthless_removals: Vec<WireRemoval>,
    name_changes: Vec<WireNameChange>,
}

#[derive(Deserialize)]
struct WireSplit {
    symbol: String,
    ex_date: NaiveDate,
    #[serde(deserialize_with = "decimal")]
    old_rate: Decimal,
    #[serde(deserialize_with = "decimal")]
    new_rate: Decimal,
}

#[derive(Deserialize)]
struct WireRate {
    symbol: String,
    ex_date: NaiveDate,
    #[serde(deserialize_with = "decimal")]
    rate: Decimal,
}

#[derive(Deserialize)]
struct WireSpinOff {
    source_symbol: String,
    #[serde(deserialize_with = "decimal")]
    source_rate: Decimal,
    new_symbol: String,
    #[serde(deserialize_with = "decimal")]
    new_rate: Decimal,
    ex_date: NaiveDate,
}

#[derive(Deserialize)]
struct WireCashMerger {
    acquiree_symbol: String,
    #[serde(deserialize_with = "decimal")]
    rate: Decimal,
    effective_date: NaiveDate,
}

#[derive(Deserialize)]
struct WireStockMerger {
    acquirer_symbol: String,
    #[serde(deserialize_with = "decimal")]
    acquirer_rate: Decimal,
    acquiree_symbol: String,
    #[serde(deserialize_with = "decimal")]
    acquiree_rate: Decimal,
    #[serde(default, deserialize_with = "decimal")]
    cash_rate: Decimal,
    effective_date: NaiveDate,
}

#[derive(Deserialize)]
struct WireRemoval {
    symbol: String,
    process_date: NaiveDate,
}

#[derive(Deserialize)]
struct WireNameChange {
    old_symbol: String,
    new_symbol: String,
    process_date: NaiveDate,
}

impl Paged for ActionsPage {
    fn take_next_page_token(&mut self) -> Option<String> {
        self.next_page_token.take()
    }
}

impl ActionsPage {
    pub fn into_actions(self) -> Vec<CorporateAction> {
        let lists = self.corporate_actions;
        let action = |symbol: &str, ex_date, kind| CorporateAction {
            symbol: Symbol::new(symbol),
            ex_date,
            kind,
        };
        let mut actions = Vec::new();
        for split in lists.forward_splits.into_iter().chain(lists.reverse_splits) {
            actions.push(action(
                &split.symbol,
                split.ex_date,
                CorporateActionKind::Split {
                    old_rate: split.old_rate,
                    new_rate: split.new_rate,
                },
            ));
        }
        for dividend in lists.cash_dividends {
            let kind = CorporateActionKind::CashDividend {
                rate: dividend.rate,
            };
            actions.push(action(&dividend.symbol, dividend.ex_date, kind));
        }
        for dividend in lists.stock_dividends {
            let kind = CorporateActionKind::StockDividend {
                rate: dividend.rate,
            };
            actions.push(action(&dividend.symbol, dividend.ex_date, kind));
        }
        for spin_off in lists.spin_offs {
            let kind = CorporateActionKind::SpinOff {
                new_symbol: Symbol::new(&spin_off.new_symbol),
                old_rate: spin_off.source_rate,
                new_rate: spin_off.new_rate,
            };
            actions.push(action(&spin_off.source_symbol, spin_off.ex_date, kind));
        }
        for merger in lists.cash_mergers {
            let kind = CorporateActionKind::CashMerger { rate: merger.rate };
            actions.push(action(&merger.acquiree_symbol, merger.effective_date, kind));
        }
        for merger in lists
            .stock_mergers
            .into_iter()
            .chain(lists.stock_and_cash_mergers)
        {
            let kind = CorporateActionKind::StockMerger {
                acquirer: Symbol::new(&merger.acquirer_symbol),
                old_rate: merger.acquiree_rate,
                new_rate: merger.acquirer_rate,
                cash_rate: merger.cash_rate,
            };
            actions.push(action(&merger.acquiree_symbol, merger.effective_date, kind));
        }
        for removal in lists.worthless_removals {
            let kind = CorporateActionKind::WorthlessRemoval;
            actions.push(action(&removal.symbol, removal.process_date, kind));
        }
        for change in lists.name_changes {
            let kind = CorporateActionKind::NameChange {
                new_symbol: Symbol::new(&change.new_symbol),
            };
            actions.push(action(&change.old_symbol, change.process_date, kind));
        }
        actions
    }
}

#[derive(Debug, Deserialize)]
pub struct WireNews {
    id: u64,
    #[serde(default)]
    symbols: Vec<Symbol>,
    headline: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    source: String,
    created_at: Timestamp,
    updated_at: Timestamp,
}

impl WireNews {
    pub fn into_article(self, received_at: Timestamp, backfilled: bool) -> NewsArticle {
        NewsArticle {
            id: self.id,
            symbols: self.symbols,
            headline: self.headline,
            summary: self.summary,
            source: self.source,
            created_at: self.created_at,
            updated_at: self.updated_at,
            received_at,
            backfilled,
        }
    }
}

#[derive(Deserialize)]
pub struct NewsPage {
    #[serde(default)]
    pub news: Vec<WireNews>,
    pub next_page_token: Option<String>,
}
