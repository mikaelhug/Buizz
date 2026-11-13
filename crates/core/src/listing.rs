use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;

use crate::market::{CorporateAction, CorporateActionKind, Symbol};

type Changes = BTreeMap<Symbol, Vec<(NaiveDate, Symbol)>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listings {
    renamed_to: Changes,
    renamed_from: Changes,
    succeeded_from: Changes,
    merged_away: BTreeMap<Symbol, Vec<NaiveDate>>,
    ended: BTreeMap<Symbol, Vec<NaiveDate>>,
}

impl Listings {
    pub fn new<'a>(
        actions: impl IntoIterator<Item = &'a CorporateAction>,
        successions: impl IntoIterator<Item = &'a CorporateAction>,
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
        for action in successions {
            if let CorporateActionKind::StockMerger {
                acquirer,
                old_rate,
                new_rate,
                cash_rate,
            } = &action.kind
                && old_rate == new_rate
                && cash_rate.is_zero()
            {
                listings
                    .succeeded_from
                    .entry(acquirer.clone())
                    .or_default()
                    .push((action.ex_date, action.symbol.clone()));
                listings
                    .merged_away
                    .entry(action.symbol.clone())
                    .or_default()
                    .push(action.ex_date);
            }
        }
        for (ticker, last) in keys.into_iter().filter_map(Symbol::ended_listing) {
            listings.ended.entry(ticker).or_default().push(last);
        }
        for changes in listings
            .renamed_to
            .values_mut()
            .chain(listings.renamed_from.values_mut())
            .chain(listings.succeeded_from.values_mut())
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
            let taken_over = |until: NaiveDate| {
                self.renamed_from.get(&ticker).is_some_and(|changes| {
                    changes.iter().any(|(date, _)| on < *date && *date <= until)
                })
            };
            let renamed = self.renamed_to.get(&ticker).and_then(|changes| {
                changes
                    .iter()
                    .find(|(date, _)| *date > on && !taken_over(*date))
            });
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
        let later = |changes: &Changes| {
            changes
                .get(&ticker)
                .is_some_and(|changes| changes.iter().any(|(date, _)| *date > on))
        };
        let arrives_later = later(&self.renamed_from);
        let merged_away = self
            .merged_away
            .get(&ticker)
            .is_some_and(|dates| dates.iter().any(|date| *date > on));
        let held_before = arrives_later && (later(&self.renamed_to) || merged_away);
        (!arrives_later || held_before).then_some(ticker)
    }

    pub fn renamed_into(&self, ticker: &Symbol, since: NaiveDate) -> bool {
        self.renamed_from
            .get(ticker)
            .is_some_and(|changes| changes.iter().any(|(date, _)| *date >= since))
    }

    pub fn ticker(
        &self,
        symbol: &Symbol,
        on: NaiveDate,
        since: Option<NaiveDate>,
        until: Option<NaiveDate>,
    ) -> Symbol {
        if let Some(until) = until {
            let (mut renamed, mut since) = (symbol.clone(), since);
            let mut seen = BTreeSet::new();
            while seen.insert(renamed.clone()) {
                let Some((date, new)) = self.renamed_to.get(&renamed).and_then(|changes| {
                    changes.iter().find(|(date, _)| {
                        since.is_none_or(|since| since < *date) && *date <= on && *date < until
                    })
                }) else {
                    break;
                };
                (renamed, since) = (new.clone(), Some(*date));
            }
            if renamed != *symbol {
                return renamed;
            }
        }
        let (mut symbol, mut bound) = (symbol.clone(), until.unwrap_or(NaiveDate::MAX));
        let mut seen = BTreeSet::new();
        while seen.insert(symbol.clone()) {
            let within = |changes: &Vec<(NaiveDate, Symbol)>| {
                changes
                    .iter()
                    .rfind(|(date, _)| on < *date && *date <= bound)
                    .cloned()
            };
            let renamed = self.renamed_from.get(&symbol).and_then(within);
            let succeeded = self
                .succeeded_from
                .get(&self.current(&symbol, on))
                .and_then(within);
            let Some((date, old)) = renamed.into_iter().chain(succeeded).max() else {
                break;
            };
            bound = date.pred_opt().unwrap_or(NaiveDate::MIN);
            symbol = old;
        }
        symbol
    }

    pub fn ticker_of(&self, key: &Symbol, on: NaiveDate) -> Option<Symbol> {
        let ticker = match key.ended_listing() {
            Some((ticker, last)) if on <= last => ticker,
            Some(_) => return None,
            None => self.ticker(key, on, None, None),
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

    #[test]
    fn a_ticker_renamed_away_by_a_company_that_took_it_later_names_the_earlier_company_before_then()
    {
        let ended = sym("XL@2016-07-22");
        let listings = Listings::new(
            &[
                renamed("PIC", "XL", "2020-12-24"),
                renamed("XL", "SPRU", "2022-11-14"),
            ],
            [],
            [&ended],
        );
        assert_eq!(
            listings.listing(&sym("XL"), date("2016-03-01")),
            Some(ended)
        );
        assert_eq!(
            listings.listing(&sym("XL"), date("2018-03-01")),
            Some(sym("XL")),
            "XL Group traded under XL before XL Fleet took the ticker and renamed it away"
        );
        assert_eq!(
            listings.listing(&sym("XL"), date("2021-06-01")),
            Some(sym("SPRU"))
        );
    }

    #[test]
    fn a_member_merged_one_for_one_into_a_new_company_was_the_merged_company_before() {
        let merged = CorporateAction {
            symbol: sym("PARA"),
            ex_date: date("2025-08-07"),
            kind: CorporateActionKind::StockMerger {
                acquirer: sym("SKYD"),
                old_rate: 1.into(),
                new_rate: 1.into(),
                cash_rate: 0.into(),
            },
        };
        let listings = Listings::new(
            &[
                renamed("PSKY", "SKYD", "2026-10-06"),
                renamed("BNZI", "PARA", "2026-08-07"),
            ],
            [&merged],
            [],
        );
        assert_eq!(
            listings.listing(&sym("PARA"), date("2024-01-02")),
            Some(sym("PARA")),
            "Paramount traded as PARA before another company took the ticker"
        );
        assert_eq!(
            listings.ticker(&sym("SKYD"), date("2024-01-02"), None, None),
            sym("PARA")
        );
        assert_eq!(
            listings.ticker(&sym("SKYD"), date("2025-09-02"), None, None),
            sym("PSKY")
        );
    }
}
