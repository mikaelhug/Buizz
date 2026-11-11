import numpy as np
import numpy.typing as npt

from buizz import Bot, Context, FundamentalPoint, run

METRICS = ["net_income", "equity", "long_term_debt", "shares_outstanding"]


def latest_values(points: list[FundamentalPoint]) -> dict[tuple[str, str], float]:
    newest: dict[tuple[str, str], FundamentalPoint] = {}
    for point in points:
        if point.metric == "net_income" and point.period_type != "FY":
            continue
        key = (point.symbol, point.metric)
        current = newest.get(key)
        if current is None or (point.period_end, point.filed) > (current.period_end, current.filed):
            newest[key] = point
    return {key: float(point.value) for key, point in newest.items()}


def percentile_ranks(values: list[float]) -> npt.NDArray[np.float64]:
    ranks = np.argsort(np.argsort(values)).astype(np.float64)
    return ranks / max(len(values) - 1, 1)


class ValueQuality(Bot):
    def on_step(self, ctx: Context) -> None:
        date = ctx.session.date
        quarter = f"{date.year}Q{(date.month - 1) // 3 + 1}"
        if ctx.state.get("quarter") == quarter:
            return
        values = latest_values(ctx.fundamentals(ctx.universe, METRICS))
        rows: list[tuple[str, float, float, float]] = []
        for symbol, bar in ctx.bars.items():
            net_income = values.get((symbol, "net_income"))
            equity = values.get((symbol, "equity"))
            shares = values.get((symbol, "shares_outstanding"))
            if net_income is None or equity is None or shares is None:
                continue
            if net_income <= 0 or equity <= 0 or shares <= 0:
                continue
            debt = values.get((symbol, "long_term_debt"), 0.0)
            rows.append((symbol, net_income / (shares * bar.close), net_income / equity, debt / equity))
        if not rows:
            return
        ctx.state["quarter"] = quarter
        score = (
            percentile_ranks([row[1] for row in rows])
            + percentile_ranks([row[2] for row in rows])
            + percentile_ranks([-row[3] for row in rows])
        ) / 3
        ranked = sorted(zip(score, rows, strict=True), key=lambda pair: pair[0], reverse=True)
        chosen = ranked[: ctx.params["holdings"]]
        weight = float(ctx.params["gross"]) / len(chosen)
        for place, (_, (symbol, earnings_yield, roe, leverage)) in enumerate(chosen, start=1):
            ctx.order_target_weight(
                symbol,
                weight,
                reason=f"rank {place} of {len(rows)}: earnings yield {earnings_yield:.1%}, "
                f"ROE {roe:.1%}, long-term debt/equity {leverage:.2f}",
            )
        keep = {row[0] for _, row in chosen}
        for symbol, position in ctx.portfolio.positions.items():
            if position.qty != 0 and symbol not in keep:
                ctx.order_target_weight(symbol, 0, reason="no longer among the top value-quality names")


if __name__ == "__main__":
    run(ValueQuality)
