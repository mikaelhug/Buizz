from datetime import date

import numpy as np
import numpy.typing as npt
from buizz_bots.monthly import MOMENTUM_MONTHS, first_of_month, month_end_closes
from buizz_bots.sessions import REBALANCED, first_step_or_month_end
from buizz_bots.signals import VALUATION_FROM, earnings_yield_spreads

from buizz import Bot, Context, MacroPoint, run

MAX_TILT = 0.5
MONTHS_PER_YEAR = 12
EQUITY_HISTORY_BARS = 10000


def monthly_last(points: list[tuple[date, float]]) -> list[float]:
    latest: dict[tuple[int, int], tuple[date, float]] = {}
    for day, value in points:
        if (day.year, day.month) not in latest or day >= latest[day.year, day.month][0]:
            latest[day.year, day.month] = (day, value)
    return [latest[month][1] for month in sorted(latest)]


def tilt(history: list[float], trim: float) -> float:
    low, median, high = np.quantile(history, [trim, 0.5, 1 - trim])
    if high <= low:
        return 0.0
    trimmed = min(max(history[-1], low), high)
    return float(np.clip((trimmed - median) / (high - low), -MAX_TILT, MAX_TILT))


def earnings_yields(ctx: Context, value: str) -> list[float]:
    if value == "ep_minus_bond":
        series = earnings_yield_spreads(ctx.macro(["sp500_forward_pe", "dgs10"], VALUATION_FROM, ctx.session.date))
    else:
        points = ctx.macro(["sp500_forward_pe"], VALUATION_FROM, ctx.session.date)
        series = [(point.date, 100 / float(point.value)) for point in points if point.value > 0]
    return monthly_last(series)


def bill_index(bills: list[MacroPoint], months: list[date]) -> npt.NDArray[np.float64]:
    rates = {point.date: float(point.value) for point in bills}
    rate = rates[min(rates)]
    growth: list[float] = []
    for month in months:
        rate = rates.get(month, rate)
        growth.append(1 + rate / 100 / MONTHS_PER_YEAR)
    return np.cumprod(growth)


def excess_momentum(ctx: Context, equity: str) -> list[float]:
    today = ctx.session.date
    closes = month_end_closes(ctx.history([equity], "1d", EQUITY_HISTORY_BARS, adjust="all")[equity], today)
    if closes.size <= MOMENTUM_MONTHS:
        return []
    months = [first_of_month(today, closes.size - 1 - index) for index in range(closes.size)]
    bills = ctx.macro(["tb3ms"], months[0], today)
    if not bills:
        return []
    cash = bill_index(bills, months)
    stock = closes[MOMENTUM_MONTHS:] / closes[:-MOMENTUM_MONTHS]
    return (stock - cash[MOMENTUM_MONTHS:] / cash[:-MOMENTUM_MONTHS]).tolist()


class ForwardPeTilt(Bot):
    def on_step(self, ctx: Context) -> None:
        if not first_step_or_month_end(ctx, range(1, 13)):
            return
        ctx.state[REBALANCED] = ctx.session.date.isoformat()
        equity: str = ctx.params["equity"]
        cash: str = ctx.params["cash"]
        trim = float(ctx.params["trim"])
        minimum = round(float(ctx.params["min_history_years"]) * MONTHS_PER_YEAR)
        signals = {
            "value": earnings_yields(ctx, ctx.params["value"]),
            "momentum": excess_momentum(ctx, equity),
        }
        tilts = {name: tilt(history, trim) if len(history) >= minimum else 0.0 for name, history in signals.items()}
        weight = 1 + sum(tilts.values()) / len(tilts)
        why = "; ".join(
            f"{name} {history[-1]:.2f} over {len(history)} months, tilt {tilts[name]:+.0%}"
            if len(history) >= minimum
            else f"{name} neutral with {len(history)} of {minimum} months"
            for name, history in signals.items()
        )
        ctx.rebalance(
            {equity: weight, cash: max(0.0, 1 - weight)},
            reason=f"equity at {weight:.0%}: {why}",
            exit_reason="not part of the value and momentum tilt",
        )


if __name__ == "__main__":
    run(ForwardPeTilt)
