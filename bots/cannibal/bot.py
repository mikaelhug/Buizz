from collections import defaultdict
from datetime import UTC, date, datetime, time, timedelta

import numpy as np

from buizz import Bot, Context, FundamentalPoint, History, run

YEARS_NEEDED = 2
MIN_GAP_DAYS = 300
MAX_GAP_DAYS = 430
HISTORY_BARS = 600
NANOSECONDS_PER_SECOND = 1_000_000_000


def annual_series(points: list[FundamentalPoint], metric: str) -> dict[str, list[FundamentalPoint]]:
    series: defaultdict[str, list[FundamentalPoint]] = defaultdict(list)
    for point in points:
        if point.metric == metric and point.period_type == "FY":
            series[point.symbol].append(point)
    return {symbol: sorted(found, key=lambda point: point.period_end, reverse=True) for symbol, found in series.items()}


def end_of_day_ns(day: date) -> int:
    midnight = datetime.combine(day + timedelta(days=1), time.min, tzinfo=UTC)
    return int(midnight.timestamp()) * NANOSECONDS_PER_SECOND


def split_factor(raw: History, adjusted: History, day: date) -> float | None:
    index = int(np.searchsorted(raw.start, end_of_day_ns(day), side="left")) - 1
    if index < 0 or index >= len(adjusted.close) or adjusted.close[index] <= 0:
        return None
    return float(raw.close[index] / adjusted.close[index])


def share_change(shares: list[FundamentalPoint], raw: History, adjusted: History) -> float | None:
    latest, prior = shares[0], shares[1]
    if not MIN_GAP_DAYS <= (latest.period_end - prior.period_end).days <= MAX_GAP_DAYS:
        return None
    latest_factor = split_factor(raw, adjusted, latest.period_end)
    prior_factor = split_factor(raw, adjusted, prior.period_end)
    if latest_factor is None or prior_factor is None or prior.value <= 0:
        return None
    return float(latest.value) * latest_factor / (float(prior.value) * prior_factor) - 1


def deepest_shrinkers(
    candidates: dict[str, list[FundamentalPoint]], raw: dict[str, History], adjusted: dict[str, History]
) -> list[tuple[str, float]]:
    changes: list[tuple[str, float]] = []
    for symbol, shares in candidates.items():
        change = share_change(shares, raw[symbol], adjusted[symbol])
        if change is not None:
            changes.append((symbol, change))
    return sorted(changes, key=lambda pair: pair[1])


class Cannibal(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        quarter = f"{today.year}Q{(today.month - 1) // 3 + 1}"
        if ctx.state.get("quarter") == quarter:
            return
        points = ctx.fundamentals(ctx.universe, ["shares_outstanding", "net_income"])
        profits = annual_series(points, "net_income")
        shares = annual_series(points, "shares_outstanding")
        candidates = {
            symbol: found
            for symbol, found in shares.items()
            if len(found) >= YEARS_NEEDED and symbol in profits and profits[symbol][0].value > 0
        }
        if not candidates:
            return
        ctx.state["quarter"] = quarter
        names = list(candidates)
        raw = ctx.history(names, "1d", HISTORY_BARS, "raw")
        adjusted = ctx.history(names, "1d", HISTORY_BARS, "split")
        shrink_limit = -float(ctx.params["min_shrink"])
        chosen: list[tuple[str, float]] = []
        for symbol, change in deepest_shrinkers(candidates, raw, adjusted):
            if change <= shrink_limit and len(chosen) < ctx.params["holdings"]:
                chosen.append((symbol, change))
        if not chosen:
            return
        weight = float(ctx.params["gross"]) / len(chosen)
        for symbol, change in chosen:
            ctx.order_target_weight(symbol, weight, reason=f"shares {change:.1%} year over year, net income positive")
        keep = {symbol for symbol, _ in chosen}
        for symbol, position in ctx.portfolio.positions.items():
            if position.qty != 0 and symbol not in keep:
                ctx.order_target_weight(symbol, 0, reason="no longer shrinking its share count")


if __name__ == "__main__":
    run(Cannibal)
