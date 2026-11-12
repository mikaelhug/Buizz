from bisect import bisect_right
from datetime import date

from buizz_bots.sessions import last_session_of_month
from buizz_bots.trend import month_end_trend

from buizz import Bot, Context, MacroPoint, run

HISTORY_FROM = date(2000, 1, 1)


def spreads(points: list[MacroPoint]) -> list[tuple[date, float]]:
    yields = sorted((point.published, point.date, float(point.value)) for point in points if point.series == "dgs10")
    published = [day for day, _, _ in yields]
    paired: list[tuple[date, float]] = []
    for point in sorted((point for point in points if point.series == "sp500_forward_pe"), key=lambda p: p.date):
        known = bisect_right(published, point.published)
        if known and point.value > 0:
            paired.append((point.date, 100 / float(point.value) - yields[known - 1][2]))
    return paired


class ForwardPeTilt(Bot):
    def on_step(self, ctx: Context) -> None:
        equity: str = ctx.params["equity"]
        bonds: str = ctx.params["bonds"]
        invested = any(ctx.position(symbol).qty != 0 for symbol in (equity, bonds))
        if invested and not last_session_of_month(ctx):
            return
        today = ctx.session.date
        months = int(ctx.params["sma_months"])
        history = spreads(ctx.macro(["sp500_forward_pe", "dgs10"], HISTORY_FROM, today))
        expensive, valuation = self.expensive(ctx, history)
        if (found := month_end_trend(ctx, equity, months)) is None:
            downtrend, trend = False, f"{equity} has fewer than {months} month-end closes"
        else:
            close, sma = found
            downtrend = close < sma
            relation = "below" if downtrend else "at or above"
            trend = f"{equity} {close:.2f} {relation} its {months}m SMA {sma:.2f}"
        tilted = expensive and downtrend
        bond_weight = float(ctx.params["bond_weight"]) if tilted else 0.0
        state = "expensive and in a downtrend, tilt" if tilted else "hold"
        ctx.rebalance(
            {equity: 1 - bond_weight, bonds: bond_weight},
            reason=f"{state}: {valuation}; {trend}",
            exit_reason="not part of the valuation and trend tilt",
        )

    def expensive(self, ctx: Context, history: list[tuple[date, float]]) -> tuple[bool, str]:
        if not history:
            return False, "no forward P/E and 10-year yield yet"
        years = float(ctx.params["min_history_years"])
        first, (latest, spread) = history[0][0], history[-1]
        if (latest - first).days < years * 365.25:
            return False, f"{len(history)} spreads since {first}, fewer than {years:g} years"
        rank = sum(past <= spread for _, past in history) / len(history)
        cutoff = float(ctx.params["expensive_quantile"])
        relation = "at or below" if rank <= cutoff else "above"
        return rank <= cutoff, (
            f"earnings yield minus 10-year yield {spread:.2f} pp on {latest}, "
            f"percentile {rank:.0%} of {len(history)} since {first}, {relation} {cutoff:.0%}"
        )


if __name__ == "__main__":
    run(ForwardPeTilt)
