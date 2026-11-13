use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{Bar, CorporateAction, CorporateActionKind};
use buizz_core::ops::{ActionDisagreement, ActionVerdict, CheckedAction};
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
            around(bars, date)
                .and_then(|(close, open)| (close / open / ratio).to_f64())
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
                ActionVerdict::Tiingo
            } else {
                ActionVerdict::Alpaca
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
                kept: ActionVerdict::Alpaca,
            });
        }
    }
    check
}

pub(crate) fn refuted_by_prices(
    bars: &[Bar],
    date: NaiveDate,
    ratio: Decimal,
    distributed: Decimal,
    other_splits: &[NaiveDate],
) -> bool {
    let Some((close, open)) = around(bars, date).filter(|(close, _)| *close > distributed) else {
        return false;
    };
    let distance = |moved: Decimal, ratio: Decimal| {
        (moved / ratio).to_f64().map(|relative| relative.ln().abs())
    };
    let moved = (close - distributed) / open;
    let refuted = match (distance(moved, ratio), distance(moved, Decimal::ONE)) {
        (Some(split), Some(none)) => split - none > CORROBORATED_LOG_DISTANCE,
        _ => false,
    };
    let shown_on_another_session = bars.windows(2).any(|pair| {
        let moved = pair[0].close / pair[1].open;
        let session = pair[1].start.exchange_date();
        session != date
            && !other_splits.contains(&session)
            && !pair[1].open.is_zero()
            && match (distance(moved, ratio), distance(moved, Decimal::ONE)) {
                (Some(split), Some(none)) => split <= CORROBORATED_LOG_DISTANCE && split < none,
                _ => false,
            }
    });
    refuted && !shown_on_another_session
}

fn around(bars: &[Bar], date: NaiveDate) -> Option<(Decimal, Decimal)> {
    let index = bars.partition_point(|bar| bar.start.exchange_date() < date);
    let (previous, on) = (bars.get(index.checked_sub(1)?)?, bars.get(index)?);
    (on.start.exchange_date() == date && !on.open.is_zero()).then_some((previous.close, on.open))
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
    fn a_split_is_refuted_only_when_the_prices_move_far_closer_to_no_split() {
        let dec = |text: &str| text.parse::<Decimal>().unwrap();
        let refuted = |close: i64, open: i64, ratio: &str, distributed: &str| {
            let bars = [
                bar("2020-04-02", close, close),
                bar("2020-04-03", open, open),
            ];
            refuted_by_prices(&bars, date("2020-04-03"), dec(ratio), dec(distributed), &[])
        };
        let misdated = [
            bar("2025-02-26", 102, 102),
            bar("2025-02-27", 102, 102),
            bar("2025-02-28", 21, 21),
        ];
        assert!(
            !refuted_by_prices(&misdated, date("2025-02-27"), dec("5"), Decimal::ZERO, &[]),
            "a 5-for-1 the prices show a session away from Alpaca's ex-date is a real split"
        );
        assert!(
            refuted_by_prices(
                &misdated,
                date("2025-02-27"),
                dec("5"),
                Decimal::ZERO,
                &[date("2025-02-28")]
            ),
            "a second filing of a split Alpaca also has on the session the prices show it"
        );
        assert!(
            refuted(86, 51, "0.5", "0"),
            "RTX's 1-for-2 on the day UTX spun off CARR and OTIS: 86 to 51 is no 1-for-2"
        );
        assert!(refuted(39, 39, "2", "0"), "an unmoved price is no 2-for-1");
        assert!(!refuted(81, 41, "2", "0"), "a real 2-for-1");
        assert!(
            !refuted(1487, 1000, "1.487", "0"),
            "DowDuPont's DOW spin-off as the split that holds its value"
        );
        assert!(
            !refuted(60, 60, "0.8", "12"),
            "a share consolidation paired with a special dividend leaves the price where it was"
        );
        assert!(
            !refuted(232, 240, "0.5", "115"),
            "a 1-for-2 on the day of a spin-off worth half the price leaves the price where it was"
        );
        assert!(
            !refuted(100, 97, "1.011", "0"),
            "Honeywell's spin-off splits are too small for the prices to settle"
        );
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
        let kept: Vec<(NaiveDate, CheckedAction, ActionVerdict)> = check
            .disagreements
            .iter()
            .map(|found| (found.ex_date, found.action, found.kept))
            .collect();
        assert_eq!(
            kept,
            vec![
                (
                    date("2017-05-17"),
                    CheckedAction::Split,
                    ActionVerdict::Tiingo
                ),
                (
                    date("2017-06-02"),
                    CheckedAction::Split,
                    ActionVerdict::Tiingo
                ),
                (
                    date("2017-08-30"),
                    CheckedAction::CashDividend,
                    ActionVerdict::Alpaca
                ),
            ]
        );
    }
}
