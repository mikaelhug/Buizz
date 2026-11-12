use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{Bar, CorporateAction, CorporateActionKind, DataSource};
use buizz_core::ops::{ActionDisagreement, CheckedAction};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

const CORROBORATED_LOG_DISTANCE: f64 = 0.1;
const DIVIDEND_TOLERANCE: Decimal = Decimal::from_parts(5, 0, 0, false, 3);

pub(crate) struct CrossCheck {
    pub disagreements: Vec<ActionDisagreement>,
    pub adopted_splits: Vec<(NaiveDate, Option<CorporateAction>)>,
}

pub(crate) fn cross_check(
    alpaca: &[CorporateAction],
    tiingo: &[CorporateAction],
    bars: &[Bar],
) -> CrossCheck {
    let mut check = CrossCheck {
        disagreements: Vec::new(),
        adopted_splits: Vec::new(),
    };
    let Some(symbol) = tiingo.first().map(|action| action.symbol.clone()) else {
        return check;
    };
    let ratios = |actions: &[CorporateAction]| {
        let mut ratios: BTreeMap<NaiveDate, Decimal> = BTreeMap::new();
        for action in actions {
            if let Some((old_rate, new_rate)) = action.kind.share_ratio() {
                *ratios.entry(action.ex_date).or_insert(Decimal::ONE) *= new_rate / old_rate;
            }
        }
        ratios
    };
    let dividends = |actions: &[CorporateAction]| {
        let mut dividends: BTreeMap<NaiveDate, Decimal> = BTreeMap::new();
        for action in actions {
            if let CorporateActionKind::CashDividend { rate } = action.kind {
                *dividends.entry(action.ex_date).or_default() += rate;
            }
        }
        dividends
    };
    let reorganized: BTreeSet<NaiveDate> = alpaca
        .iter()
        .filter(|action| action.kind.delivered().is_some() || action.kind.ends_listing())
        .map(|action| action.ex_date)
        .collect();
    let (alpaca_ratios, tiingo_ratios) = (ratios(alpaca), ratios(tiingo));
    let split_dates: BTreeSet<NaiveDate> = alpaca_ratios
        .keys()
        .chain(tiingo_ratios.keys())
        .copied()
        .filter(|date| !reorganized.contains(date))
        .collect();
    for date in split_dates {
        let (ours, theirs) = (alpaca_ratios.get(&date), tiingo_ratios.get(&date));
        if ours == theirs {
            continue;
        }
        let distance = |ratio: Option<&Decimal>| {
            let ratio = ratio.copied().unwrap_or(Decimal::ONE);
            price_ratio(bars, date)
                .and_then(|moved| (moved / ratio).to_f64())
                .map(|relative| relative.ln().abs())
        };
        let tiingo_wins = match (distance(ours), distance(theirs)) {
            (Some(ours), Some(theirs)) => theirs < ours && theirs <= CORROBORATED_LOG_DISTANCE,
            _ => false,
        };
        if tiingo_wins {
            let adopted = tiingo
                .iter()
                .find(|action| action.ex_date == date && action.kind.share_ratio().is_some())
                .cloned();
            check.adopted_splits.push((date, adopted));
        }
        check.disagreements.push(ActionDisagreement {
            symbol: symbol.clone(),
            ex_date: date,
            action: CheckedAction::Split,
            alpaca: ours.copied(),
            tiingo: theirs.copied(),
            kept: if tiingo_wins {
                DataSource::Tiingo
            } else {
                DataSource::Alpaca
            },
        });
    }
    let (alpaca_dividends, tiingo_dividends) = (dividends(alpaca), dividends(tiingo));
    let dividend_dates: BTreeSet<NaiveDate> = alpaca_dividends
        .keys()
        .chain(tiingo_dividends.keys())
        .copied()
        .collect();
    for date in dividend_dates {
        let (ours, theirs) = (alpaca_dividends.get(&date), tiingo_dividends.get(&date));
        let agree = match (ours, theirs) {
            (Some(ours), Some(theirs)) => {
                (*ours - *theirs).abs() <= DIVIDEND_TOLERANCE * (*ours).max(*theirs)
            }
            _ => false,
        };
        if !agree {
            check.disagreements.push(ActionDisagreement {
                symbol: symbol.clone(),
                ex_date: date,
                action: CheckedAction::CashDividend,
                alpaca: ours.copied(),
                tiingo: theirs.copied(),
                kept: DataSource::Alpaca,
            });
        }
    }
    check
}

fn price_ratio(bars: &[Bar], date: NaiveDate) -> Option<Decimal> {
    let index = bars.partition_point(|bar| bar.start.exchange_date() < date);
    let (previous, on) = (bars.get(index.checked_sub(1)?)?, bars.get(index)?);
    (on.start.exchange_date() == date && !on.open.is_zero()).then(|| previous.close / on.open)
}

#[cfg(test)]
mod tests {
    use buizz_core::market::{Feed, Symbol};
    use buizz_core::time::Timestamp;

    use super::*;

    fn date(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    fn bar(day: &str, open: i64, close: i64) -> Bar {
        Bar {
            symbol: Symbol::new("BALL"),
            start: format!("{day}T13:30:00Z").parse::<Timestamp>().unwrap(),
            open: Decimal::from(open),
            high: Decimal::from(open.max(close)),
            low: Decimal::from(open.min(close)),
            close: Decimal::from(close),
            volume: 1,
            feed: Feed::Sip,
        }
    }

    fn action(day: &str, kind: CorporateActionKind) -> CorporateAction {
        CorporateAction {
            symbol: Symbol::new("BALL"),
            ex_date: date(day),
            kind,
        }
    }

    #[test]
    fn a_split_alpaca_lacks_is_taken_from_tiingo_when_the_prices_bear_it_out() {
        let split = CorporateActionKind::Split {
            old_rate: Decimal::ONE,
            new_rate: Decimal::TWO,
        };
        let dividend = |rate: &str| CorporateActionKind::CashDividend {
            rate: rate.parse().unwrap(),
        };
        let bars = [
            bar("2017-05-15", 82, 82),
            bar("2017-05-16", 82, 81),
            bar("2017-05-17", 41, 40),
            bar("2017-06-01", 40, 40),
            bar("2017-06-02", 40, 40),
        ];
        let alpaca = [
            action("2017-05-30", dividend("0.10")),
            action("2017-06-02", split.clone()),
        ];
        let tiingo = [
            action("2017-05-17", split.clone()),
            action("2017-05-30", dividend("0.10")),
            action("2017-08-30", dividend("0.10")),
        ];
        let check = cross_check(&alpaca, &tiingo, &bars);
        assert_eq!(
            check.adopted_splits,
            vec![
                (date("2017-05-17"), Some(tiingo[0].clone())),
                (date("2017-06-02"), None),
            ],
            "81 to 41 is the 2-for-1 only Tiingo records; 2 June's unmoved price refutes Alpaca's"
        );
        let kept: Vec<(NaiveDate, CheckedAction, DataSource)> = check
            .disagreements
            .iter()
            .map(|found| (found.ex_date, found.action, found.kept))
            .collect();
        assert_eq!(
            kept,
            vec![
                (date("2017-05-17"), CheckedAction::Split, DataSource::Tiingo),
                (date("2017-06-02"), CheckedAction::Split, DataSource::Tiingo),
                (
                    date("2017-08-30"),
                    CheckedAction::CashDividend,
                    DataSource::Alpaca
                ),
            ]
        );
    }
}
