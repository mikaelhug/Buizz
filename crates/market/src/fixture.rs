use std::path::Path;

use buizz_core::market::{Bar, Calendar, CorporateAction, Feed, Symbol};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::codec::action_kind;
use crate::{MarketError, Result};

pub(crate) async fn rows<T: DeserializeOwned>(dir: &Path, name: &str) -> Result<Vec<T>> {
    let path = dir.join(name);
    let bytes = tokio::fs::read(&path).await?;
    csv::Reader::from_reader(bytes.as_slice())
        .deserialize()
        .collect::<std::result::Result<Vec<T>, _>>()
        .map_err(|error| MarketError::Fixture(format!("{}: {error}", path.display())))
}

#[derive(Deserialize)]
pub(crate) struct DailyRow {
    symbol: Symbol,
    date: NaiveDate,
    open: Decimal,
    high: Decimal,
    low: Decimal,
    close: Decimal,
    volume: u64,
}

impl DailyRow {
    pub(crate) fn into_bar(self, calendar: &Calendar) -> Result<Bar> {
        let session = calendar.session(self.date).ok_or_else(|| {
            MarketError::Fixture(format!(
                "daily bar {} {} has no calendar session",
                self.symbol, self.date
            ))
        })?;
        Ok(Bar {
            symbol: self.symbol,
            start: session.open,
            open: self.open,
            high: self.high,
            low: self.low,
            close: self.close,
            volume: self.volume,
            feed: Feed::Sip,
        })
    }
}

#[derive(Deserialize)]
pub(crate) struct ActionRow {
    symbol: Symbol,
    ex_date: NaiveDate,
    kind: String,
    old_rate: Option<Decimal>,
    new_rate: Option<Decimal>,
    rate: Option<Decimal>,
}

impl ActionRow {
    pub(crate) fn into_action(self) -> Result<CorporateAction> {
        let kind =
            action_kind(&self.kind, self.old_rate, self.new_rate, self.rate).ok_or_else(|| {
                MarketError::Fixture(format!(
                    "corporate action {} {} has kind {:?} without its rates",
                    self.symbol, self.ex_date, self.kind
                ))
            })?;
        Ok(CorporateAction {
            symbol: self.symbol,
            ex_date: self.ex_date,
            kind,
        })
    }
}
