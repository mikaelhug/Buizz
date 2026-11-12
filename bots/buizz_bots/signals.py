from bisect import bisect_right
from datetime import date, timedelta
from typing import NamedTuple

import numpy as np
import numpy.typing as npt

from buizz import Context, History, MacroPoint
from buizz_bots.macro import RELEASE_LAG_MONTHS, latest_monthly
from buizz_bots.monthly import first_of_month
from buizz_bots.trend import Trend, daily_trend, month_end_trend

TREND_SESSIONS = 200
TREND_MONTHS = 10
BREADTH_THRESHOLD = 0.5
SPREAD_MONTHS = 6
VIX_STALE_DAYS = 10
UNRATE_MONTHS = 12
ARMED_DAYS = 730
SAHM_THRESHOLD = 0.5
SAHM_LOOKBACK_MONTHS = 12
SAHM_AVERAGE_MONTHS = 3
EXPENSIVE_QUANTILE = 0.2
VALUATION_MIN_YEARS = 3.0
VALUATION_FROM = date(2000, 1, 1)

Closes = tuple[int, npt.NDArray[np.float64]]


class Signal(NamedTuple):
    bullish: bool
    why: str


def trend(ctx: Context, symbol: str, sessions: int = TREND_SESSIONS) -> Signal | None:
    if (found := daily_trend(ctx, symbol, sessions)) is None:
        return None
    close, sma = found
    return Signal(close > sma, f"{symbol} {close:.2f} vs its {sessions}-day SMA {sma:.2f}")


def monthly_trend(ctx: Context, symbol: str, months: int = TREND_MONTHS) -> Signal | None:
    if (found := month_end_trend(ctx, symbol, months)) is None:
        return None
    close, sma = found
    return Signal(close > sma, f"{symbol} {close:.2f} vs its {months}-month SMA {sma:.2f}")


def extends(cached: Closes, recent: History) -> bool:
    last_start, closes = cached
    found = np.flatnonzero(recent.start == last_start)
    return found.size > 0 and bool(np.isclose(recent.close[found[0]], closes[-1], rtol=1e-9, atol=0))


class SmaCache:
    def __init__(self) -> None:
        self.closes: dict[str, Closes] = {}

    def refresh(self, ctx: Context, symbols: list[str], window: int) -> int:
        self.closes = {symbol: cached for symbol, cached in self.closes.items() if symbol in symbols}
        recent = ctx.history(symbols, "1d", 2, adjust="all")
        stale: list[str] = []
        for symbol in symbols:
            history = recent[symbol]
            cached = self.closes.get(symbol)
            if history.start.size == 0:
                self.closes.pop(symbol, None)
            elif cached is None or not extends(cached, history):
                stale.append(symbol)
            else:
                fresh = history.start > cached[0]
                self.closes[symbol] = (
                    int(history.start[-1]),
                    np.append(cached[1], history.close[fresh])[-window:],
                )
        if stale:
            for symbol, history in ctx.history(stale, "1d", window, adjust="all").items():
                self.closes[symbol] = (int(history.start[-1]), history.close)
        return len(stale)

    def trend(self, symbol: str, window: int) -> Trend | None:
        cached = self.closes.get(symbol)
        if cached is None or cached[1].size < window:
            return None
        return float(cached[1][-1]), float(cached[1].mean())

    def above(self, symbols: list[str], window: int) -> tuple[int, int]:
        trends = [trend for symbol in symbols if (trend := self.trend(symbol, window)) is not None]
        return sum(close > sma for close, sma in trends), len(trends)


def breadth(
    ctx: Context, cache: SmaCache, window: int = TREND_SESSIONS, threshold: float = BREADTH_THRESHOLD
) -> Signal | None:
    members = ctx.universe_at(ctx.session.date, index="sp500")
    cache.refresh(ctx, members, window)
    above, counted = cache.above(members, window)
    if not counted:
        return None
    share = above / counted
    return Signal(
        share >= threshold, f"{above} of {counted} S&P 500 members above their {window}-day SMA ({share:.1%})"
    )


def credit(ctx: Context, months: int = SPREAD_MONTHS) -> Signal | None:
    today = ctx.session.date
    points = ctx.macro(["baa", "gs10"], first_of_month(today, months + RELEASE_LAG_MONTHS), today)
    treasury = {point.date: point.value for point in points if point.series == "gs10"}
    spreads = [
        (point.date, float(point.value - treasury[point.date]))
        for point in points
        if point.series == "baa" and point.date in treasury
    ][-months:]
    if len(spreads) < months:
        return None
    month, spread = spreads[-1]
    average = sum(value for _, value in spreads) / months
    return Signal(
        spread <= average, f"Baa-10y spread {spread:.2f} ({month:%Y-%m}) vs its {months}-month average {average:.2f}"
    )


def vix_term(ctx: Context, stale_days: int = VIX_STALE_DAYS) -> Signal | None:
    today = ctx.session.date
    points = ctx.macro(["vix", "vix3m"], today - timedelta(days=stale_days), today)
    three_month = {point.date: point.value for point in points if point.series == "vix3m"}
    paired = [
        (point.date, point.value, three_month[point.date])
        for point in points
        if point.series == "vix" and point.date in three_month
    ]
    if not paired:
        return None
    day, vix, vix3m = max(paired)
    ratio = float(vix / vix3m)
    return Signal(ratio <= 1, f"VIX/VIX3M {vix}/{vix3m} = {ratio:.3f} ({day})")


def labour(ctx: Context, months: int = UNRATE_MONTHS) -> Signal | None:
    points = latest_monthly(ctx, "unrate", months)
    if len(points) < months:
        return None
    rate = float(points[-1].value)
    average = sum(float(point.value) for point in points) / months
    return Signal(
        rate <= average,
        f"unemployment {rate:.1f}% ({points[-1].date:%Y-%m}) vs its {months}-month average {average:.2f}%",
    )


def curve_inversions(ctx: Context, days: int) -> tuple[int, list[date]] | None:
    today = ctx.session.date
    points = ctx.macro(["dgs10", "treasury_3m"], today - timedelta(days=days), today)
    bills = {point.date: point.value for point in points if point.series == "treasury_3m"}
    curve = [
        (point.date, point.value - bills[point.date])
        for point in points
        if point.series == "dgs10" and point.date in bills
    ]
    return (len(curve), [day for day, spread in curve if spread < 0]) if curve else None


def sahm_gap(rates: list[float]) -> float:
    averages = [
        sum(rates[end - SAHM_AVERAGE_MONTHS : end]) / SAHM_AVERAGE_MONTHS
        for end in range(SAHM_AVERAGE_MONTHS, len(rates) + 1)
    ]
    return averages[-1] - min(averages[-1 - SAHM_LOOKBACK_MONTHS : -1])


def sahm(ctx: Context) -> float | None:
    needed = SAHM_LOOKBACK_MONTHS + SAHM_AVERAGE_MONTHS
    points = latest_monthly(ctx, "unrate", needed)
    return sahm_gap([float(point.value) for point in points]) if len(points) == needed else None


def curve(ctx: Context, armed_days: int = ARMED_DAYS, threshold: float = SAHM_THRESHOLD) -> Signal | None:
    if (found := curve_inversions(ctx, armed_days)) is None:
        return None
    paired, inverted = found
    if not inverted:
        return Signal(True, f"10y-3m not negative on any of {paired} days in the last {armed_days} days: unarmed")
    if (gap := sahm(ctx)) is None:
        return None
    return Signal(
        gap < threshold, f"armed: 10y-3m last negative on {inverted[-1]}; Sahm gap {gap:.2f} (trigger {threshold:.2f})"
    )


def earnings_yield_spreads(points: list[MacroPoint]) -> list[tuple[date, float]]:
    yields = sorted((point.published, point.date, float(point.value)) for point in points if point.series == "dgs10")
    published = [day for day, _, _ in yields]
    paired: list[tuple[date, float]] = []
    for point in sorted((point for point in points if point.series == "sp500_forward_pe"), key=lambda p: p.date):
        known = bisect_right(published, point.published)
        if known and point.value > 0:
            paired.append((point.date, 100 / float(point.value) - yields[known - 1][2]))
    return paired


def valuation(
    ctx: Context, quantile: float = EXPENSIVE_QUANTILE, min_years: float = VALUATION_MIN_YEARS
) -> Signal | None:
    history = earnings_yield_spreads(ctx.macro(["sp500_forward_pe", "dgs10"], VALUATION_FROM, ctx.session.date))
    if not history:
        return None
    first, (latest, spread) = history[0][0], history[-1]
    if (latest - first).days < min_years * 365.25:
        return None
    rank = sum(past <= spread for _, past in history) / len(history)
    return Signal(
        rank > quantile,
        f"earnings yield minus 10-year yield {spread:.2f} pp on {latest}, "
        f"percentile {rank:.0%} of {len(history)} since {first}, bearish at or below {quantile:.0%}",
    )
