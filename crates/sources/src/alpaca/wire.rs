use std::collections::BTreeMap;

use buizz_core::market::{
    AssetClass, Bar, Calendar, CorporateAction, CorporateActionKind, Feed, Instrument, Session,
    Symbol, Timeframe,
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
        let is_etf = self
            .name
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word.eq_ignore_ascii_case("etf"));
        Some(Instrument {
            symbol: Symbol::new(&self.symbol),
            name: self.name,
            exchange: self.exchange,
            asset_class: if is_etf {
                AssetClass::Etf
            } else {
                AssetClass::Stock
            },
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
struct ActionLists {
    #[serde(default)]
    forward_splits: Vec<WireSplit>,
    #[serde(default)]
    reverse_splits: Vec<WireSplit>,
    #[serde(default)]
    cash_dividends: Vec<WireDividend>,
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
struct WireDividend {
    symbol: String,
    ex_date: NaiveDate,
    #[serde(deserialize_with = "decimal")]
    rate: Decimal,
}

impl Paged for ActionsPage {
    fn take_next_page_token(&mut self) -> Option<String> {
        self.next_page_token.take()
    }
}

impl ActionsPage {
    pub fn into_actions(self) -> Vec<CorporateAction> {
        let lists = self.corporate_actions;
        let splits = lists
            .forward_splits
            .into_iter()
            .chain(lists.reverse_splits)
            .map(|split| CorporateAction {
                symbol: Symbol::new(&split.symbol),
                ex_date: split.ex_date,
                kind: CorporateActionKind::Split {
                    old_rate: split.old_rate,
                    new_rate: split.new_rate,
                },
            });
        let dividends = lists
            .cash_dividends
            .into_iter()
            .map(|dividend| CorporateAction {
                symbol: Symbol::new(&dividend.symbol),
                ex_date: dividend.ex_date,
                kind: CorporateActionKind::CashDividend {
                    rate: dividend.rate,
                },
            });
        splits.chain(dividends).collect()
    }
}
