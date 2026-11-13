use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{
    AssetClass, Bar, CorporateAction, CorporateActionKind, Coverage, DataSource, Dataset,
    Instrument, Symbol, Timeframe,
};
use buizz_market::ActionWindow;
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;

use crate::{Ingest, IngestError, day_start};

const RENAMES: [(&str, &str, &str); 33] = [
    ("MIL", "MFCB", "2016-02-16"),
    ("UA", "UAA", "2016-04-08"),
    ("TSO", "ANDV", "2017-08-01"),
    ("DOW", "DWDP", "2017-09-01"),
    ("DLPH", "APTV", "2017-12-05"),
    ("WYN", "WYND", "2018-06-01"),
    ("DPS", "KDP", "2018-07-10"),
    ("ITUS", "ANIX", "2018-10-01"),
    ("ALN", "PLAG", "2018-10-01"),
    ("TPIV", "MRKR", "2018-10-18"),
    ("CCE", "CCEP", "2018-11-07"),
    ("KAAC", "ALTM", "2018-11-12"),
    ("RXII", "PHIO", "2018-11-19"),
    ("WGP", "WES", "2019-02-28"),
    ("AMGP", "AM", "2019-03-13"),
    ("FOXA", "TFCFA", "2019-03-19"),
    ("ORPN", "ENLV", "2019-03-26"),
    ("KCAP", "PTMN", "2019-04-02"),
    ("PRAN", "ATHE", "2019-04-09"),
    ("NVMM", "REKR", "2019-04-30"),
    ("CMSS", "KXIN", "2019-05-02"),
    ("FONE", "NXTG", "2019-05-30"),
    ("DWDP", "DD", "2019-06-03"),
    ("OMAD", "PACK", "2019-06-03"),
    ("MFCB", "SRL", "2019-06-04"),
    ("MTEC", "KERN", "2019-06-18"),
    ("CYTX", "PSTV", "2019-07-29"),
    ("AETI", "SLNG", "2019-07-29"),
    ("ESV", "VAL", "2019-07-31"),
    ("BHGE", "BKR", "2019-10-18"),
    ("IPOA", "SPCE", "2019-10-28"),
    ("CUR", "SNCA", "2019-11-01"),
    ("LB", "BBWI", "2021-08-03"),
];
const SPIN_OFFS: [(&str, &str, &str, &str); 4] = [
    ("UAA", "UA", "2016-04-08", "1"),
    ("APTV", "DLPH", "2017-12-05", "0.33333333"),
    ("TNL", "WH", "2018-06-01", "1"),
    ("BBWI", "VSXY", "2021-08-03", "0.33333333"),
];
const CASH: [(&str, &str, &str); 1] = [("CCEP", "2016-05-31", "14.50")];
const ENDED: [(&str, &str); 1] = [("TIVO", "2016-09-07")];

fn date(text: &str) -> NaiveDate {
    text.parse().expect("a curated date is a date")
}

fn curated_actions(priced: &BTreeMap<Symbol, Coverage>) -> Vec<CorporateAction> {
    let priced = |tickers: &[&str]| {
        tickers
            .iter()
            .any(|ticker| priced.contains_key(&Symbol::new(ticker)))
    };
    let renames = RENAMES
        .iter()
        .filter(|(old, new, _)| priced(&[old, new]))
        .map(|(old, new, on)| CorporateAction {
            symbol: Symbol::new(old),
            ex_date: date(on),
            kind: CorporateActionKind::NameChange {
                new_symbol: Symbol::new(new),
            },
        });
    let spin_offs = SPIN_OFFS
        .iter()
        .filter(|(parent, _, _, _)| priced(&[parent]))
        .map(|(parent, child, on, rate)| CorporateAction {
            symbol: Symbol::new(parent),
            ex_date: date(on),
            kind: CorporateActionKind::SpinOff {
                new_symbol: Symbol::new(child),
                old_rate: Decimal::ONE,
                new_rate: rate.parse().expect("a curated rate is a decimal"),
            },
        });
    let cash = CASH
        .iter()
        .filter(|(symbol, _, _)| priced(&[symbol]))
        .map(|(symbol, on, rate)| CorporateAction {
            symbol: Symbol::new(symbol),
            ex_date: date(on),
            kind: CorporateActionKind::CashDividend {
                rate: rate.parse().expect("a curated rate is a decimal"),
            },
        });
    renames.chain(spin_offs).chain(cash).collect()
}

impl Ingest {
    pub(crate) async fn curate(&self) -> Result<u64, IngestError> {
        let mut rows = 0;
        let priced = self
            .symbol_coverage(Dataset::Bars1d, DataSource::Alpaca)
            .await?;
        let actions = curated_actions(&priced);
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
        let stored = self
            .market
            .corporate_actions(window.symbols.clone(), window.from, window.through)
            .await?;
        if actions.iter().any(|action| !stored.contains(action)) {
            rows += self
                .market
                .replace_corporate_actions(DataSource::Curated, actions, window)
                .await?
                .rows;
            let listings = self.market.listings();
            let renamed: Vec<Symbol> = RENAMES
                .iter()
                .filter_map(|(_, new, on)| listings.listing(&Symbol::new(new), date(on)))
                .collect();
            self.market
                .forget_coverage(Dataset::CorporateActions, DataSource::Alpaca, renamed)
                .await?;
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
        let keys: BTreeSet<Symbol> = instruments
            .into_iter()
            .map(|instrument| instrument.symbol)
            .collect();
        let listings = self.market.listings();
        let mut listed = Vec::new();
        for (old, new, on) in RENAMES {
            let (old, new, on) = (Symbol::new(old), Symbol::new(new), date(on));
            if !priced.contains_key(&old) && !priced.contains_key(&new) {
                continue;
            }
            let Some(listing) = listings.listing(&new, on) else {
                continue;
            };
            let eve = self
                .market
                .calendar(on - TimeDelta::days(10), on - TimeDelta::days(1))
                .await?
                .sessions()
                .last()
                .map(|session| session.date);
            let traded_as = eve
                .map(|eve| Symbol::ended(&old, eve))
                .filter(|ended| keys.contains(ended))
                .unwrap_or(old);
            let bars = self
                .market
                .bars_between(
                    Timeframe::Day,
                    vec![traded_as],
                    day_start(floor),
                    day_start(on),
                )
                .await?;
            listed.extend(bars.into_iter().map(|bar| Bar {
                symbol: listing.clone(),
                ..bar
            }));
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
