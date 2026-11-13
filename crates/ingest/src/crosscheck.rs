use std::collections::{BTreeMap, BTreeSet};

use buizz_core::market::{Bar, CorporateAction, CorporateActionKind};
use buizz_core::ops::{ActionDisagreement, ActionVerdict, CheckedAction};
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

const CORROBORATED_LOG_DISTANCE: f64 = 0.1;
const SPLIT_MATCH_LOG_DISTANCE: f64 = 0.25;
const SPLIT_REACH_SESSIONS: usize = 3;
const SPLIT_REACH_DAYS: i64 = 7;
const DIVIDEND_TOLERANCE: Decimal = Decimal::from_parts(1, 0, 0, false, 2);
const UNPRICED_DIVIDEND_SHARE: Decimal = Decimal::from_parts(1, 0, 0, false, 1);
const REPEATED_DIVIDEND_DAYS: i64 = 3;

pub(crate) struct CrossCheck {
    pub disagreements: Vec<ActionDisagreement>,
    pub adopted: Vec<Adoption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Adoption {
    pub ex_date: NaiveDate,
    pub action: CheckedAction,
    pub tiingo: Vec<CorporateAction>,
}

pub(crate) fn checked_as(action: &CorporateAction, checked: CheckedAction) -> bool {
    let kind = match action.kind {
        CorporateActionKind::CashDividend { .. } => CheckedAction::CashDividend,
        _ if action.kind.share_ratio().is_some() => CheckedAction::Split,
        _ => return false,
    };
    kind == checked
}

pub(crate) fn cross_check(
    alpaca: &[CorporateAction],
    tiingo: &[CorporateAction],
    bars: &[Bar],
) -> CrossCheck {
    let mut check = CrossCheck {
        disagreements: Vec::new(),
        adopted: Vec::new(),
    };
    let adopt = |ex_date: NaiveDate, action: CheckedAction| Adoption {
        ex_date,
        action,
        tiingo: tiingo
            .iter()
            .filter(|tiingo| tiingo.ex_date == ex_date && checked_as(tiingo, action))
            .cloned()
            .collect(),
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
            check.adopted.push(adopt(date, CheckedAction::Split));
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
        let kept = match (ours, theirs) {
            (Some(ours), Some(theirs))
                if (*ours - *theirs).abs() <= DIVIDEND_TOLERANCE * (*ours).max(*theirs) =>
            {
                continue;
            }
            (Some(_), Some(_)) => {
                check.adopted.push(adopt(date, CheckedAction::CashDividend));
                ActionVerdict::Tiingo
            }
            _ => ActionVerdict::Alpaca,
        };
        check.disagreements.push(ActionDisagreement {
            symbol: symbol.clone(),
            ex_date: date,
            action: CheckedAction::CashDividend,
            alpaca: ours.copied(),
            tiingo: theirs.copied(),
            kept,
        });
    }
    check
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitEvidence {
    Shown,
    Moved(NaiveDate),
    Refuted,
    Unclear,
}

pub(crate) fn split_evidence(
    bars: &[Bar],
    date: NaiveDate,
    ratio: Decimal,
    distributed: Decimal,
    other_splits: &[NaiveDate],
) -> SplitEvidence {
    let index = bars.partition_point(|bar| bar.start.exchange_date() < date);
    let reached = bars.get(index).is_some_and(|bar| {
        index > 0 && bar.start.exchange_date() - date <= TimeDelta::days(SPLIT_REACH_DAYS)
    });
    if !reached {
        return SplitEvidence::Unclear;
    }
    let distance = |index: usize, distributed: Decimal, ratio: Decimal| {
        let close = bars[index - 1].close - distributed;
        let on = &bars[index];
        [on.open, on.close]
            .into_iter()
            .filter(|price| !price.is_zero() && close > Decimal::ZERO)
            .filter_map(|price| (close / price / ratio).to_f64())
            .map(|relative| relative.ln().abs())
            .reduce(f64::min)
    };
    if distance(index, distributed, ratio).is_some_and(|split| split <= SPLIT_MATCH_LOG_DISTANCE) {
        return SplitEvidence::Shown;
    }
    let nearby = index.saturating_sub(SPLIT_REACH_SESSIONS).max(1)
        ..=(index + SPLIT_REACH_SESSIONS).min(bars.len() - 1);
    let moved = nearby
        .filter(|near| *near != index && !other_splits.contains(&bars[*near].start.exchange_date()))
        .filter_map(|near| Some((distance(near, Decimal::ZERO, ratio)?, near)))
        .filter(|(split, _)| *split <= SPLIT_MATCH_LOG_DISTANCE)
        .min_by(|left, right| left.0.total_cmp(&right.0));
    if let Some((_, near)) = moved {
        return SplitEvidence::Moved(bars[near].start.exchange_date());
    }
    match (
        distance(index, distributed, ratio),
        distance(index, distributed, Decimal::ONE),
    ) {
        (Some(split), Some(none)) if none < split => SplitEvidence::Refuted,
        _ => SplitEvidence::Unclear,
    }
}

pub(crate) fn unpriced_dividend(rate: Decimal, previous_close: Decimal, ex_open: Decimal) -> bool {
    rate > previous_close * UNPRICED_DIVIDEND_SHARE
        && previous_close - ex_open < rate / Decimal::TWO
}

pub(crate) fn repeated_dividends(actions: &[CorporateAction]) -> BTreeSet<usize> {
    let mut dividends: Vec<(&CorporateAction, Decimal, usize)> = actions
        .iter()
        .enumerate()
        .filter_map(|(index, action)| match action.kind {
            CorporateActionKind::CashDividend { rate } => Some((action, rate, index)),
            _ => None,
        })
        .collect();
    dividends.sort_by_key(|(action, rate, _)| (action.symbol.clone(), *rate, action.ex_date));
    let mut repeated = BTreeSet::new();
    let mut paid: Option<&CorporateAction> = None;
    for (action, rate, index) in dividends {
        let repeats = paid.is_some_and(|paid| {
            paid.symbol == action.symbol
                && paid.kind == CorporateActionKind::CashDividend { rate }
                && action.ex_date - paid.ex_date <= TimeDelta::days(REPEATED_DIVIDEND_DAYS)
        });
        if repeats {
            repeated.insert(index);
        } else {
            paid = Some(action);
        }
    }
    repeated
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
    fn a_split_stands_where_the_prices_show_it_and_falls_where_they_show_none() {
        let dec = |text: &str| text.parse::<Decimal>().unwrap();
        let evidence = |close: i64, open: i64, ratio: &str, distributed: &str| {
            let bars = [
                bar("2020-04-02", close, close),
                bar("2020-04-03", open, open),
            ];
            split_evidence(&bars, date("2020-04-03"), dec(ratio), dec(distributed), &[])
        };
        let misdated = [
            bar("2026-02-25", 1, 1),
            bar("2026-02-26", 1, 1),
            bar("2026-02-27", 30, 30),
        ];
        assert_eq!(
            split_evidence(
                &misdated,
                date("2026-02-26"),
                dec("0.0333"),
                Decimal::ZERO,
                &[]
            ),
            SplitEvidence::Moved(date("2026-02-27")),
            "IXHL's 1-for-30 shows in the prices a session after Alpaca's ex-date"
        );
        assert_eq!(
            split_evidence(
                &misdated,
                date("2026-02-26"),
                dec("0.0333"),
                Decimal::ZERO,
                &[date("2026-02-27")]
            ),
            SplitEvidence::Refuted,
            "a second filing of a split Alpaca also has on the session the prices show it"
        );
        assert_eq!(
            evidence(86, 51, "0.5", "0"),
            SplitEvidence::Refuted,
            "RTX's 1-for-2 on the day UTX spun off CARR and OTIS: 86 to 51 is no 1-for-2"
        );
        assert_eq!(evidence(39, 39, "2", "0"), SplitEvidence::Refuted);
        assert_eq!(evidence(81, 41, "2", "0"), SplitEvidence::Shown);
        assert_eq!(
            evidence(1, 6, "0.1", "0"),
            SplitEvidence::Unclear,
            "a 1-for-10 followed by a slide is still nearer the split than no split"
        );
        assert_eq!(
            evidence(60, 60, "0.8", "12"),
            SplitEvidence::Shown,
            "a share consolidation paired with a special dividend leaves the price where it was"
        );
        assert_eq!(
            evidence(232, 240, "0.5", "115"),
            SplitEvidence::Shown,
            "HON's 1-for-2 on the day it spun off HONA, worth half the price"
        );
    }

    #[test]
    fn a_dividend_the_prices_do_not_pay_or_paid_twice_is_not_a_dividend() {
        let dec = |text: &str| text.parse::<Decimal>().unwrap();
        assert!(
            unpriced_dividend(dec("16.40"), dec("32"), dec("31.5")),
            "AIV's $16.40 on a $32 stock that opened unmoved"
        );
        assert!(!unpriced_dividend(dec("12"), dec("60"), dec("48")));
        assert!(!unpriced_dividend(dec("0.50"), dec("40"), dec("40.2")));
        let dividend = |day: &str, rate: &str| {
            action(day, CorporateActionKind::CashDividend { rate: dec(rate) })
        };
        let paid = [
            dividend("2020-08-31", "0.44"),
            dividend("2020-09-01", "0.44"),
            dividend("2020-09-01", "0.10"),
            dividend("2020-12-21", "0.44"),
        ];
        assert_eq!(
            repeated_dividends(&paid),
            BTreeSet::from([1]),
            "ESLT's dividend filed for two days running"
        );
    }

    #[test]
    fn tiingo_wins_a_split_the_prices_bear_out_and_a_dividend_both_record_differently() {
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
            action("2017-08-30", dividend("0.33")),
            action("2017-11-29", dividend("0.10")),
        ];
        let tiingo = [
            action("2017-05-17", split.clone()),
            action("2017-05-30", dividend("0.10")),
            action("2017-08-29", dividend("0.10")),
            action("2017-08-30", dividend("0.44")),
        ];
        let check = cross_check(&alpaca, &tiingo, &bars);
        let adopted: Vec<(NaiveDate, Vec<CorporateAction>)> = check
            .adopted
            .iter()
            .map(|adoption| (adoption.ex_date, adoption.tiingo.clone()))
            .collect();
        assert_eq!(
            adopted,
            vec![
                (date("2017-05-17"), vec![tiingo[0].clone()]),
                (date("2017-06-02"), vec![]),
                (date("2017-08-30"), vec![tiingo[3].clone()]),
            ],
            "81 to 41 is the 2-for-1 only Tiingo records; 2 June's unmoved price refutes Alpaca's; \
             AON's dividend is the one Tiingo records"
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
                    date("2017-08-29"),
                    CheckedAction::CashDividend,
                    ActionVerdict::Alpaca
                ),
                (
                    date("2017-08-30"),
                    CheckedAction::CashDividend,
                    ActionVerdict::Tiingo
                ),
                (
                    date("2017-11-29"),
                    CheckedAction::CashDividend,
                    ActionVerdict::Alpaca
                ),
            ]
        );
    }
}
