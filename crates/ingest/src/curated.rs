use buizz_core::market::{
    AssetClass, CorporateAction, CorporateActionKind, DataSource, Dataset, Instrument, Symbol,
    Timeframe,
};
use buizz_market::ActionWindow;
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;

use crate::{Ingest, IngestError};

const RENAMES: [(&str, &str, &str); 2] =
    [("UA", "UAA", "2016-04-08"), ("DLPH", "APTV", "2017-12-05")];
const SPIN_OFFS: [(&str, &str, &str, &str); 2] = [
    ("UAA", "UA", "2016-04-08", "1"),
    ("APTV", "DLPH", "2017-12-05", "0.33333333"),
];
const ENDED: [(&str, &str); 1] = [("TIVO", "2016-09-07")];

fn date(text: &str) -> NaiveDate {
    text.parse().expect("a curated date is a date")
}

fn curated_actions() -> Vec<CorporateAction> {
    let renames = RENAMES.iter().map(|(old, new, on)| CorporateAction {
        symbol: Symbol::new(old),
        ex_date: date(on),
        kind: CorporateActionKind::NameChange {
            new_symbol: Symbol::new(new),
        },
    });
    let spin_offs = SPIN_OFFS
        .iter()
        .map(|(parent, child, on, rate)| CorporateAction {
            symbol: Symbol::new(parent),
            ex_date: date(on),
            kind: CorporateActionKind::SpinOff {
                new_symbol: Symbol::new(child),
                old_rate: Decimal::ONE,
                new_rate: rate.parse().expect("a curated rate is a decimal"),
            },
        });
    renames.chain(spin_offs).collect()
}

impl Ingest {
    pub(crate) async fn curate(&self) -> Result<u64, IngestError> {
        let mut rows = 0;
        let priced = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
            .await?;
        let stored = self.market.actions_reaching_other_symbols().await?;
        let actions: Vec<CorporateAction> = curated_actions()
            .into_iter()
            .filter(|action| priced.contains_key(&action.symbol))
            .collect();
        if actions.iter().any(|action| !stored.contains(action)) {
            let window = ActionWindow {
                symbols: actions.iter().map(|action| action.symbol.clone()).collect(),
                from: actions
                    .iter()
                    .map(|action| action.ex_date)
                    .min()
                    .unwrap_or(NaiveDate::MIN),
                through: actions
                    .iter()
                    .map(|action| action.ex_date)
                    .max()
                    .unwrap_or(NaiveDate::MIN),
                covered_to: None,
            };
            rows += self
                .market
                .replace_corporate_actions(DataSource::Curated, actions, window)
                .await?
                .rows;
        }
        let instruments = self.market.instruments().await?;
        let ended: Vec<Instrument> = ENDED
            .iter()
            .filter(|(ticker, _)| priced.contains_key(&Symbol::new(ticker)))
            .map(|(ticker, last)| Instrument {
                symbol: Symbol::ended(&Symbol::new(ticker), date(last)),
                name: String::new(),
                exchange: String::new(),
                asset_class: AssetClass::Stock,
                active: false,
                shortable: false,
                first_session: None,
                last_session: Some(date(last)),
            })
            .filter(|ended| {
                !instruments
                    .iter()
                    .any(|stored| stored.symbol == ended.symbol)
            })
            .collect();
        if !ended.is_empty() {
            rows += self
                .market
                .upsert_instruments(DataSource::Curated, ended)
                .await?
                .rows;
        }
        let floor = self.config.history_start;
        let mut listed = Vec::new();
        for (old, new, on) in RENAMES {
            let last = date(on) - TimeDelta::days(1);
            listed.extend(
                self.listed_as(&Symbol::new(old), &Symbol::new(new), floor, last)
                    .await?,
            );
        }
        for (ticker, last) in ENDED {
            let ticker = Symbol::new(ticker);
            let ended = Symbol::ended(&ticker, date(last));
            listed.extend(self.listed_as(&ticker, &ended, floor, date(last)).await?);
        }
        if !listed.is_empty() {
            rows += self
                .market
                .upsert_bars(DataSource::Alpaca, Timeframe::Day, listed)
                .await?
                .rows;
        }
        Ok(rows)
    }
}
