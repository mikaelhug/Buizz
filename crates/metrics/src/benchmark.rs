use buizz_core::api::SparkPoint;
use buizz_core::market::{Bar, CorporateAction, CorporateActionKind, Session};
use buizz_core::money::round_cash;
use rust_decimal::Decimal;

pub fn buy_and_hold(
    start_cash: Decimal,
    bars: &[(Session, Bar)],
    actions: &[CorporateAction],
) -> Vec<SparkPoint> {
    let Some(((base_session, base), held)) = bars
        .split_first()
        .filter(|((_, base), _)| base.close > Decimal::ZERO)
    else {
        return Vec::new();
    };
    let mut actions = actions
        .iter()
        .filter(|action| action.ex_date > base_session.date)
        .peekable();
    let mut shares = start_cash / base.close;
    let mut dividends = Decimal::ZERO;
    let mut points = vec![SparkPoint {
        at: base_session.close,
        equity: round_cash(start_cash),
    }];
    for (session, bar) in held {
        while let Some(action) = actions.next_if(|action| action.ex_date <= session.date) {
            if !action.kind.has_valid_rates() {
                continue;
            }
            if let CorporateActionKind::CashDividend { rate } = action.kind {
                dividends += shares * rate;
            } else if let Some((old_rate, new_rate)) = action.kind.share_ratio() {
                shares = shares * new_rate / old_rate;
            }
        }
        if !dividends.is_zero() && bar.open > Decimal::ZERO {
            shares += dividends / bar.open;
            dividends = Decimal::ZERO;
        }
        points.push(SparkPoint {
            at: session.close,
            equity: round_cash(shares * bar.close + dividends),
        });
    }
    points
}

#[cfg(test)]
mod tests {
    use buizz_core::market::Feed;
    use buizz_core::time::Timestamp;
    use chrono::{NaiveDate, NaiveTime, TimeDelta};

    use super::*;

    fn day(date: &str, open: i64, close: i64) -> (Session, Bar) {
        let date: NaiveDate = date.parse().unwrap();
        let opens =
            Timestamp::at_exchange_time(date, NaiveTime::from_hms_opt(9, 30, 0).unwrap()).unwrap();
        let session = Session {
            date,
            open: opens,
            close: opens + TimeDelta::minutes(390),
        };
        let bar = Bar {
            symbol: "QQQ".into(),
            start: opens,
            open: Decimal::from(open),
            high: Decimal::from(open.max(close)),
            low: Decimal::from(open.min(close)),
            close: Decimal::from(close),
            volume: 1,
            feed: Feed::Sip,
        };
        (session, bar)
    }

    fn action(ex_date: &str, kind: CorporateActionKind) -> CorporateAction {
        CorporateAction {
            symbol: "QQQ".into(),
            ex_date: ex_date.parse().unwrap(),
            kind,
        }
    }

    #[test]
    fn the_benchmark_reinvests_dividends_and_follows_splits() {
        let bars = [
            day("2024-11-22", 100, 100),
            day("2024-11-25", 98, 99),
            day("2024-11-26", 50, 51),
            day("2024-11-27", 51, 52),
        ];
        let actions = [
            action(
                "2024-11-22",
                CorporateActionKind::CashDividend { rate: Decimal::TEN },
            ),
            action(
                "2024-11-25",
                CorporateActionKind::CashDividend { rate: Decimal::TWO },
            ),
            action(
                "2024-11-26",
                CorporateActionKind::Split {
                    old_rate: Decimal::ONE,
                    new_rate: Decimal::TWO,
                },
            ),
        ];
        let equity: Vec<Decimal> = buy_and_hold(Decimal::from(10_000), &bars, &actions)
            .into_iter()
            .map(|point| point.equity)
            .collect();
        let shares = Decimal::from(100) * (Decimal::ONE + Decimal::TWO / Decimal::from(98));
        assert_eq!(
            equity,
            [
                Decimal::from(10_000),
                round_cash(shares * Decimal::from(99)),
                round_cash(shares * Decimal::TWO * Decimal::from(51)),
                round_cash(shares * Decimal::TWO * Decimal::from(52)),
            ]
        );
    }
}
