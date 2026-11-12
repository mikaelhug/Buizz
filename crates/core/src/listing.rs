use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;

use crate::market::{CorporateAction, CorporateActionKind, Symbol};

type Changes = BTreeMap<Symbol, Vec<(NaiveDate, Symbol)>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listings {
    renamed_to: Changes,
    renamed_from: Changes,
    ended: BTreeMap<Symbol, Vec<NaiveDate>>,
}

impl Listings {
    pub fn new<'a>(
        actions: impl IntoIterator<Item = &'a CorporateAction>,
        keys: impl IntoIterator<Item = &'a Symbol>,
    ) -> Self {
        let mut listings = Self::default();
        for action in actions {
            let into_regular_way = |new_symbol: &Symbol| {
                action
                    .symbol
                    .as_str()
                    .split_once('.')
                    .is_some_and(|(base, _)| base == new_symbol.as_str())
            };
            if let CorporateActionKind::NameChange { new_symbol } = &action.kind
                && *new_symbol != action.symbol
                && !into_regular_way(new_symbol)
            {
                listings
                    .renamed_to
                    .entry(action.symbol.clone())
                    .or_default()
                    .push((action.ex_date, new_symbol.clone()));
                listings
                    .renamed_from
                    .entry(new_symbol.clone())
                    .or_default()
                    .push((action.ex_date, action.symbol.clone()));
            }
        }
        for (ticker, last) in keys.into_iter().filter_map(Symbol::ended_listing) {
            listings.ended.entry(ticker).or_default().push(last);
        }
        for changes in listings
            .renamed_to
            .values_mut()
            .chain(listings.renamed_from.values_mut())
        {
            changes.sort();
        }
        for lasts in listings.ended.values_mut() {
            lasts.sort();
        }
        listings
    }

    pub fn current(&self, symbol: &Symbol, on: NaiveDate) -> Symbol {
        let (mut symbol, mut on) = (symbol.clone(), on);
        let mut seen = BTreeSet::new();
        while seen.insert(symbol.clone()) {
            let Some((date, renamed)) = self
                .renamed_to
                .get(&symbol)
                .and_then(|changes| changes.iter().find(|(date, _)| *date >= on))
            else {
                break;
            };
            (symbol, on) = (renamed.clone(), *date);
        }
        symbol
    }

    pub fn former(&self, symbols: &[Symbol]) -> Vec<Symbol> {
        let mut wanted: BTreeSet<Symbol> = symbols.iter().cloned().collect();
        let mut pending: Vec<Symbol> = symbols.to_vec();
        while let Some(symbol) = pending.pop() {
            for (_, old) in self.renamed_from.get(&symbol).into_iter().flatten() {
                if wanted.insert(old.clone()) {
                    pending.push(old.clone());
                }
            }
        }
        wanted.into_iter().collect()
    }

    pub fn rekey(&self, mut action: CorporateAction) -> CorporateAction {
        let on = action.ex_date;
        action.symbol = self.current(&action.symbol, on);
        match &mut action.kind {
            CorporateActionKind::SpinOff { new_symbol, .. } => {
                *new_symbol = self.current(new_symbol, on);
            }
            CorporateActionKind::StockMerger { acquirer, .. } => {
                *acquirer = self.current(acquirer, on);
            }
            _ => {}
        }
        action
    }

    pub fn listing(&self, ticker: &Symbol, on: NaiveDate) -> Option<Symbol> {
        let (mut ticker, mut on) = (ticker.clone(), on);
        let mut seen = BTreeSet::new();
        while seen.insert(ticker.clone()) {
            let renamed = self
                .renamed_to
                .get(&ticker)
                .and_then(|changes| changes.iter().find(|(date, _)| *date > on));
            let ended = self
                .ended
                .get(&ticker)
                .and_then(|lasts| lasts.iter().find(|last| **last >= on));
            match (ended, renamed) {
                (Some(last), renamed) if renamed.is_none_or(|(date, _)| last < date) => {
                    return Some(Symbol::ended(&ticker, *last));
                }
                (_, Some((date, new))) => (ticker, on) = (new.clone(), *date),
                (_, None) => break,
            }
        }
        let arrives_later = self
            .renamed_from
            .get(&ticker)
            .is_some_and(|changes| changes.iter().any(|(date, _)| *date > on));
        (!arrives_later).then_some(ticker)
    }

    pub fn renamed_into(&self, ticker: &Symbol, since: NaiveDate) -> bool {
        self.renamed_from
            .get(ticker)
            .is_some_and(|changes| changes.iter().any(|(date, _)| *date >= since))
    }

    pub fn ticker(&self, symbol: &Symbol, on: NaiveDate, until: Option<NaiveDate>) -> Symbol {
        let (mut symbol, mut bound) = (symbol.clone(), until.unwrap_or(NaiveDate::MAX));
        let mut seen = BTreeSet::new();
        while seen.insert(symbol.clone()) {
            let Some((date, old)) = self.renamed_from.get(&symbol).and_then(|changes| {
                changes
                    .iter()
                    .rfind(|(date, _)| on < *date && *date <= bound)
            }) else {
                break;
            };
            bound = date.pred_opt().unwrap_or(NaiveDate::MIN);
            symbol = old.clone();
        }
        symbol
    }

    pub fn ticker_of(&self, key: &Symbol, on: NaiveDate) -> Option<Symbol> {
        let ticker = match key.ended_listing() {
            Some((ticker, last)) if on <= last => ticker,
            Some(_) => return None,
            None => self.ticker(key, on, None),
        };
        (self.listing(&ticker, on).as_ref() == Some(key)).then_some(ticker)
    }

    pub fn shadowed(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Vec<(Symbol, NaiveDate, NaiveDate, Symbol)> {
        let tickers: BTreeSet<&Symbol> = self.renamed_to.keys().chain(self.ended.keys()).collect();
        tickers
            .into_iter()
            .flat_map(|ticker| {
                self.segments(ticker, from, to)
                    .into_iter()
                    .filter(move |(_, _, key)| key != ticker)
                    .map(move |(lo, hi, key)| (ticker.clone(), lo, hi, key))
            })
            .collect()
    }

    pub fn segments(
        &self,
        ticker: &Symbol,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Vec<(NaiveDate, NaiveDate, Symbol)> {
        let mut reached = BTreeSet::from([ticker.clone()]);
        let mut pending = vec![ticker.clone()];
        while let Some(symbol) = pending.pop() {
            for (_, new) in self.renamed_to.get(&symbol).into_iter().flatten() {
                if reached.insert(new.clone()) {
                    pending.push(new.clone());
                }
            }
        }
        let mut changes: BTreeSet<NaiveDate> = BTreeSet::from([from]);
        for symbol in &reached {
            let renames = [&self.renamed_to, &self.renamed_from]
                .into_iter()
                .filter_map(|changes| changes.get(symbol))
                .flatten()
                .map(|(date, _)| *date);
            let ends = self
                .ended
                .get(symbol)
                .into_iter()
                .flatten()
                .filter_map(|last| last.succ_opt());
            changes.extend(
                renames
                    .chain(ends)
                    .filter(|date| from < *date && *date <= to),
            );
        }
        let starts: Vec<NaiveDate> = changes.into_iter().collect();
        let mut segments: Vec<(NaiveDate, NaiveDate, Symbol)> = Vec::new();
        for (index, start) in starts.iter().enumerate() {
            let end = starts
                .get(index + 1)
                .and_then(|next| next.pred_opt())
                .unwrap_or(to);
            let Some(key) = self.listing(ticker, *start) else {
                continue;
            };
            match segments.last_mut() {
                Some(last) if last.2 == key && last.1.succ_opt() == Some(*start) => last.1 = end,
                _ => segments.push((*start, end, key)),
            }
        }
        segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    fn renamed(old: &str, new: &str, on: &str) -> CorporateAction {
        CorporateAction {
            symbol: Symbol::new(old),
            ex_date: date(on),
            kind: CorporateActionKind::NameChange {
                new_symbol: Symbol::new(new),
            },
        }
    }

    fn sym(text: &str) -> Symbol {
        Symbol::new(text)
    }

    #[test]
    fn an_action_is_keyed_by_the_ticker_its_company_trades_under_today() {
        let listings = Listings::new(
            &[
                renamed("SYMC", "NLOK", "2019-11-04"),
                renamed("NLOK", "GEN", "2022-11-08"),
                renamed("FB", "META", "2022-06-09"),
                renamed("UTX", "RTX", "2020-04-03"),
            ],
            [],
        );
        let spin_off = listings.rekey(CorporateAction {
            symbol: sym("UTX"),
            ex_date: date("2020-04-03"),
            kind: CorporateActionKind::SpinOff {
                new_symbol: sym("OTIS"),
                old_rate: 2.into(),
                new_rate: 1.into(),
            },
        });
        assert_eq!(spin_off.symbol, sym("RTX"));
        assert_eq!(
            listings.current(&sym("NLOK"), date("2020-02-03")),
            sym("GEN")
        );
        assert_eq!(
            listings.current(&sym("FB"), date("2021-12-14")),
            sym("META")
        );
        assert_eq!(
            listings.current(&sym("FB"), date("2025-09-25")),
            sym("FB"),
            "a dividend of the fund that took the ticker later stays with it"
        );
        assert_eq!(
            listings.former(&[sym("GEN")]),
            vec![sym("GEN"), sym("NLOK"), sym("SYMC")]
        );
    }
}
