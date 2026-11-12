import math
from datetime import date
from typing import NamedTuple

import numpy as np
from buizz_bots.fundamentals import (
    Annual,
    adjusted_shares,
    amount,
    consecutive_fiscal_ends,
    domestic_annual,
    histories_since,
    traded_today,
)

from buizz import Bot, Context, FundamentalPoint, History, Metric, run

METRICS: list[Metric] = [
    "shares_outstanding",
    "equity",
    "preferred_stock",
    "share_repurchases",
    "operating_cash_flow",
    "capex",
]
YEARS_NEEDED = 2


class Candidate(NamedTuple):
    symbol: str
    latest: FundamentalPoint
    prior: FundamentalPoint
    book_equity: float
    buyback_to_fcf: float | None


class Signal(NamedTuple):
    symbol: str
    issuance: float
    book_to_market: float
    buyback_to_fcf: float | None


def buyback_to_fcf(annual: Annual, symbol: str, period_end: date) -> float | None:
    repurchases = amount(annual, symbol, "share_repurchases", period_end)
    operating = amount(annual, symbol, "operating_cash_flow", period_end)
    capex = amount(annual, symbol, "capex", period_end)
    if repurchases is None or operating is None or capex is None or operating - capex <= 0:
        return None
    return repurchases / (operating - capex)


def candidate(annual: Annual, symbol: str, today: date) -> Candidate | None:
    shares = annual.get((symbol, "shares_outstanding"), {})
    equity = annual.get((symbol, "equity"), {})
    fiscal_ends = consecutive_fiscal_ends(set(shares) & set(equity), today, YEARS_NEEDED)
    if fiscal_ends is None:
        return None
    latest, prior = fiscal_ends
    preferred = amount(annual, symbol, "preferred_stock", latest) or 0.0
    book_equity = float(equity[latest].value) - preferred
    if book_equity <= 0 or shares[latest].value <= 0 or shares[prior].value <= 0:
        return None
    return Candidate(symbol, shares[latest], shares[prior], book_equity, buyback_to_fcf(annual, symbol, latest))


def signal(ctx: Context, found: Candidate, raw: History, adjusted: History) -> Signal | None:
    if not traded_today(ctx, raw):
        return None
    latest = adjusted_shares(found.latest, raw, adjusted)
    prior = adjusted_shares(found.prior, raw, adjusted)
    if latest is None or prior is None:
        return None
    market_cap = latest * float(raw.close[-1])
    return Signal(found.symbol, math.log(latest / prior), found.book_equity / market_cap, found.buyback_to_fcf)


def months_between(start: str, end: str) -> int:
    start_year, start_month = map(int, start.split("-"))
    end_year, end_month = map(int, end.split("-"))
    return (end_year - start_year) * 12 + end_month - start_month


def qualifiers_of(ctx: Context) -> dict[str, str]:
    today = ctx.session.date
    annual = domestic_annual(ctx.fundamentals(ctx.universe, METRICS))
    candidates = [found for symbol in ctx.universe if (found := candidate(annual, symbol, today)) is not None]
    if not candidates:
        return {}
    oldest = min(min(found.latest.filed, found.prior.filed) for found in candidates)
    raw, adjusted = histories_since(ctx, [found.symbol for found in candidates], oldest)
    signals = [
        measured
        for found in candidates
        if found.symbol in raw
        and (measured := signal(ctx, found, raw[found.symbol], adjusted[found.symbol])) is not None
    ]
    if not signals:
        return {}
    issuance = np.array([measured.issuance for measured in signals])
    book_to_market = np.array([measured.book_to_market for measured in signals])
    issuance_cut = float(np.quantile(issuance, float(ctx.params["issuance_bottom"])))
    value_cut = float(np.quantile(book_to_market, 1 - float(ctx.params["value_top"])))
    value_percentile = (np.argsort(np.argsort(book_to_market)) + 1) / len(signals) * 100
    return {
        measured.symbol: (
            f"shares {math.expm1(measured.issuance):+.1%}/yr, B/M pct {percentile:.0f}, buyback/FCF "
            + ("n/a" if measured.buyback_to_fcf is None else f"{measured.buyback_to_fcf:.0%}")
        )
        for measured, percentile in zip(signals, value_percentile, strict=True)
        if measured.issuance < 0 and measured.issuance <= issuance_cut and measured.book_to_market >= value_cut
    }


class Singleton(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        month = f"{today:%Y-%m}"
        if ctx.state.get("month") == month:
            return
        ctx.state["month"] = month
        hold_months = int(ctx.params["hold_months"])
        cohorts: dict[str, list[str]] = ctx.state.setdefault("cohorts", {})
        qualifiers = qualifiers_of(ctx)
        for formed, members in list(cohorts.items()):
            kept = [symbol for symbol in members if symbol not in qualifiers]
            if kept and months_between(formed, month) < hold_months:
                cohorts[formed] = kept
            else:
                del cohorts[formed]
        if qualifiers:
            cohorts[month] = sorted(qualifiers)
        formed_in = {symbol: formed for formed, members in cohorts.items() for symbol in members}
        latest_bars = ctx.history(sorted(formed_in), "1d", 1, "raw") if formed_in else {}
        trading = [symbol for symbol, history in latest_bars.items() if traded_today(ctx, history)]
        weight = 1 / max(len(trading), 1)
        ctx.rebalance(
            dict.fromkeys(trading, weight),
            reason={
                symbol: qualifiers.get(symbol)
                or f"{formed_in[symbol]} cohort, month {months_between(formed_in[symbol], month) + 1} of {hold_months}"
                for symbol in trading
            },
            exit_reason=f"{hold_months}-month holding period since its last qualifying month ended",
        )


if __name__ == "__main__":
    run(Singleton)
