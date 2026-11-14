use std::str::FromStr;

use buizz_core::market::{NordicBar, NordicBars};
use chrono::NaiveDate;
use rust_decimal::Decimal;

pub(crate) const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

pub(crate) struct DayRow<'a> {
    pub date: NaiveDate,
    pub open: &'a str,
    pub high: &'a str,
    pub low: &'a str,
    pub close: &'a str,
    pub volume: &'a str,
}

fn digits(text: &str) -> String {
    text.trim().replace(',', "")
}

fn price(text: &str) -> Option<Decimal> {
    Decimal::from_str(&digits(text)).ok()
}

impl DayRow<'_> {
    fn bar(&self) -> Option<NordicBar> {
        Some(NordicBar {
            date: self.date,
            open: price(self.open)?,
            high: price(self.high)?,
            low: price(self.low)?,
            close: price(self.close)?,
            volume: digits(self.volume).parse().ok()?,
        })
    }
}

pub(crate) fn daily_bars<'a>(rows: impl IntoIterator<Item = DayRow<'a>>) -> NordicBars {
    let mut daily = NordicBars::default();
    for row in rows {
        match row.bar() {
            Some(bar) => daily.bars.push(bar),
            None => daily.skipped += 1,
        }
    }
    daily.bars.sort_by_key(|bar| bar.date);
    daily.bars.dedup_by_key(|bar| bar.date);
    daily
}
