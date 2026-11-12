import numpy as np
import numpy.typing as npt
from buizz_bots.fundamentals import adjusted_shares, in_dollars

from buizz import Bot, Context, FundamentalPoint, Metric, run

METRICS: list[Metric] = ["net_income", "equity", "long_term_debt", "shares_outstanding"]
REQUIRED: list[Metric] = ["net_income", "equity", "shares_outstanding"]
HISTORY_BARS = 600


def latest_points(points: list[FundamentalPoint]) -> dict[tuple[str, Metric], FundamentalPoint]:
    newest: dict[tuple[str, Metric], FundamentalPoint] = {}
    for point in points:
        if (point.metric == "net_income" and point.period_type != "FY") or not in_dollars(point):
            continue
        key = (point.symbol, point.metric)
        current = newest.get(key)
        if current is None or (point.period_end, point.filed) > (current.period_end, current.filed):
            newest[key] = point
    return newest


def percentile_ranks(values: list[float]) -> npt.NDArray[np.float64]:
    ranks = np.argsort(np.argsort(values)).astype(np.float64)
    return ranks / max(len(values) - 1, 1)


class ValueQuality(Bot):
    def on_step(self, ctx: Context) -> None:
        date = ctx.session.date
        quarter = f"{date.year}Q{(date.month - 1) // 3 + 1}"
        if ctx.state.get("quarter") == quarter:
            return
        latest = latest_points(ctx.fundamentals(ctx.universe, METRICS))
        eligible = [
            symbol
            for symbol in ctx.bars
            if all((symbol, metric) in latest and latest[symbol, metric].value > 0 for metric in REQUIRED)
        ]
        if not eligible:
            return
        raw = ctx.history(eligible, "1d", HISTORY_BARS, "raw")
        adjusted = ctx.history(eligible, "1d", HISTORY_BARS, "split")
        rows: list[tuple[str, float, float, float]] = []
        for symbol in eligible:
            shares = latest[symbol, "shares_outstanding"]
            share_count = adjusted_shares(shares, raw[symbol], adjusted[symbol])
            if share_count is None:
                continue
            net_income = float(latest[symbol, "net_income"].value)
            equity = float(latest[symbol, "equity"].value)
            debt_point = latest.get((symbol, "long_term_debt"))
            debt = 0.0 if debt_point is None else float(debt_point.value)
            market_cap = share_count * ctx.bars[symbol].close
            rows.append((symbol, net_income / market_cap, net_income / equity, debt / equity))
        if not rows:
            return
        ctx.state["quarter"] = quarter
        score = (
            percentile_ranks([row[1] for row in rows])
            + percentile_ranks([row[2] for row in rows])
            + percentile_ranks([-row[3] for row in rows])
        ) / 3
        ranked = sorted(zip(score, rows, strict=True), key=lambda pair: pair[0], reverse=True)
        chosen = [row for _, row in ranked[: ctx.params["holdings"]]]
        ctx.rebalance(
            dict.fromkeys((row[0] for row in chosen), float(ctx.params["gross"]) / len(chosen)),
            reason={
                symbol: f"rank {place} of {len(rows)}: earnings yield {earnings_yield:.1%}, "
                f"ROE {roe:.1%}, long-term debt/equity {leverage:.2f}"
                for place, (symbol, earnings_yield, roe, leverage) in enumerate(chosen, start=1)
            },
            exit_reason="no longer among the top value-quality names",
        )


if __name__ == "__main__":
    run(ValueQuality)
