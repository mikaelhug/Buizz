from collections import defaultdict

from buizz_bots.fundamentals import adjusted_shares

from buizz import Bot, Context, FundamentalPoint, History, run

YEARS_NEEDED = 2
MIN_GAP_DAYS = 300
MAX_GAP_DAYS = 430
HISTORY_BARS = 600


def annual_series(points: list[FundamentalPoint], metric: str) -> dict[str, list[FundamentalPoint]]:
    series: defaultdict[str, list[FundamentalPoint]] = defaultdict(list)
    for point in points:
        if point.metric == metric and point.period_type == "FY":
            series[point.symbol].append(point)
    return {symbol: sorted(found, key=lambda point: point.period_end, reverse=True) for symbol, found in series.items()}


def share_change(shares: list[FundamentalPoint], raw: History, adjusted: History) -> float | None:
    latest, prior = shares[0], shares[1]
    if not MIN_GAP_DAYS <= (latest.period_end - prior.period_end).days <= MAX_GAP_DAYS:
        return None
    latest_shares = adjusted_shares(latest, raw, adjusted)
    prior_shares = adjusted_shares(prior, raw, adjusted)
    if latest_shares is None or prior_shares is None or prior_shares <= 0:
        return None
    return latest_shares / prior_shares - 1


def shrinkers(ctx: Context, candidates: dict[str, list[FundamentalPoint]]) -> list[tuple[str, float]]:
    if not candidates:
        return []
    raw = ctx.history(list(candidates), "1d", HISTORY_BARS, "raw")
    adjusted = ctx.history(list(candidates), "1d", HISTORY_BARS, "split")
    shrink_limit = -float(ctx.params["min_shrink"])
    changes: list[tuple[str, float]] = []
    for symbol, shares in candidates.items():
        change = share_change(shares, raw[symbol], adjusted[symbol])
        if change is not None and change <= shrink_limit:
            changes.append((symbol, change))
    return sorted(changes, key=lambda pair: pair[1])[: ctx.params["holdings"]]


class Cannibal(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        quarter = f"{today.year}Q{(today.month - 1) // 3 + 1}"
        if ctx.state.get("quarter") == quarter:
            return
        points = ctx.fundamentals(ctx.universe, ["shares_outstanding", "net_income"])
        if not points:
            return
        ctx.state["quarter"] = quarter
        profits = annual_series(points, "net_income")
        candidates = {
            symbol: found
            for symbol, found in annual_series(points, "shares_outstanding").items()
            if len(found) >= YEARS_NEEDED and symbol in profits and profits[symbol][0].value > 0
        }
        chosen = shrinkers(ctx, candidates)
        weight = float(ctx.params["gross"]) / max(len(chosen), 1)
        ctx.rebalance(
            {symbol: weight for symbol, _ in chosen},
            reason={symbol: f"shares {change:.1%} year over year, net income positive" for symbol, change in chosen},
            exit_reason="no longer shrinking its share count",
        )


if __name__ == "__main__":
    run(Cannibal)
