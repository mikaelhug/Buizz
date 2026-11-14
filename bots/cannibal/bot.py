from buizz_bots.caps import market_caps
from buizz_bots.fundamentals import (
    adjusted_shares,
    amount,
    consecutive_fiscal_ends,
    domestic_annual,
    histories_since,
    year_end_shares,
)

from buizz import Bot, Context, FundamentalPoint, Metric, run

METRICS: list[Metric] = ["shares_outstanding", "net_income"]
YEARS_NEEDED = 2


def profitable_share_counts(ctx: Context) -> dict[str, tuple[FundamentalPoint, FundamentalPoint]]:
    annual = domestic_annual(ctx.fundamentals(ctx.universe, METRICS))
    found: dict[str, tuple[FundamentalPoint, FundamentalPoint]] = {}
    for symbol in ctx.universe:
        ends = consecutive_fiscal_ends(annual.get((symbol, "net_income"), {}), ctx.session.date, YEARS_NEEDED)
        if ends is None or (amount(annual, symbol, "net_income", ends[0]) or 0) <= 0:
            continue
        now, before = (year_end_shares(annual, symbol, end) for end in ends)
        if now is not None and before is not None and before.value > 0:
            found[symbol] = (now, before)
    return found


def shrinkers(ctx: Context) -> list[tuple[str, float]]:
    candidates = profitable_share_counts(ctx)
    if not candidates:
        return []
    raw, adjusted = histories_since(ctx, sorted(candidates), min(before.filed for _, before in candidates.values()))
    lines = market_caps(ctx, {symbol: now for symbol, (now, _) in candidates.items()}, raw, adjusted)
    changes: list[tuple[str, float]] = []
    for symbol in lines:
        now, before = (adjusted_shares(point, raw[symbol], adjusted[symbol]) for point in candidates[symbol])
        if now is not None and before is not None and now / before - 1 <= -float(ctx.params["min_shrink"]):
            changes.append((symbol, now / before - 1))
    return sorted(changes, key=lambda pair: pair[1])[: ctx.params["holdings"]]


class Cannibal(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        quarter = f"{today.year}Q{(today.month - 1) // 3 + 1}"
        if ctx.state.get("quarter") == quarter:
            return
        ctx.state["quarter"] = quarter
        chosen = shrinkers(ctx)
        weight = float(ctx.params["gross"]) / max(len(chosen), 1)
        ctx.rebalance(
            {symbol: weight for symbol, _ in chosen},
            reason={symbol: f"shares {change:.1%} year over year, net income positive" for symbol, change in chosen},
            exit_reason="no longer shrinking its share count",
        )


if __name__ == "__main__":
    run(Cannibal)
