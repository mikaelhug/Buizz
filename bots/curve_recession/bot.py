from datetime import timedelta

from buizz_bots.macro import latest_monthly
from buizz_bots.sessions import last_session_of_month
from buizz_bots.switch import hold_only
from buizz_bots.trend import month_end_trend

from buizz import Bot, Context, run

SAHM_LOOKBACK_MONTHS = 12
SAHM_AVERAGE_MONTHS = 3


def sahm_gap(rates: list[float]) -> float:
    averages = [
        sum(rates[end - SAHM_AVERAGE_MONTHS : end]) / SAHM_AVERAGE_MONTHS
        for end in range(SAHM_AVERAGE_MONTHS, len(rates) + 1)
    ]
    return averages[-1] - min(averages[-1 - SAHM_LOOKBACK_MONTHS : -1])


class CurveRecession(Bot):
    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        asset: str = ctx.params["asset"]
        bonds: str = ctx.params["bonds"]
        armed_days = int(ctx.params["armed_days"])
        sahm_threshold = float(ctx.params["sahm_threshold"])
        unrate_months = int(ctx.params["unrate_months"])
        sma_months = int(ctx.params["sma_months"])
        today = ctx.session.date
        points = ctx.macro(["dgs10", "treasury_3m"], today - timedelta(days=armed_days), today)
        bills = {point.date: point.value for point in points if point.series == "treasury_3m"}
        curve = [
            (point.date, point.value - bills[point.date])
            for point in points
            if point.series == "dgs10" and point.date in bills
        ]
        if not curve:
            ctx.log(f"no 10y and 3m yields of the same day in the last {armed_days} days, no decision", level="warn")
            return
        inverted = [day for day, spread in curve if spread < 0]
        if not inverted:
            why = f"10y-3m not negative on any of {len(curve)} days in the last {armed_days} days: unarmed, {asset}"
            ctx.log(why)
            hold_only(ctx, asset, why)
            return
        needed = max(unrate_months, SAHM_LOOKBACK_MONTHS + SAHM_AVERAGE_MONTHS)
        found = latest_monthly(ctx, "unrate", needed)
        if len(found) < needed:
            ctx.log(f"{len(found)} of {needed} unemployment rates visible, no decision", level="warn")
            return
        if (trend := month_end_trend(ctx, asset, sma_months)) is None:
            ctx.log(f"{asset} has fewer than {sma_months} month-end closes, no decision", level="warn")
            return
        rates = [float(point.value) for point in found]
        gap = sahm_gap(rates)
        average = sum(rates[-unrate_months:]) / unrate_months
        close, sma = trend
        signal = (
            f"armed: 10y-3m last negative on {inverted[-1]}; unemployment {rates[-1]:.1f}% ({found[-1].date:%Y-%m}), "
            f"Sahm gap {gap:.2f} (trigger {sahm_threshold:.2f}), {unrate_months}-month average {average:.2f}%; "
            f"{asset} {close:.2f} vs its {sma_months}-month SMA {sma:.2f}"
        )
        triggered = gap >= sahm_threshold or rates[-1] > average
        if not triggered:
            hold, why = asset, f"{signal}: no labour trigger, {asset}"
        elif close > sma:
            hold, why = asset, f"{signal}: labour trigger but {asset} above its SMA, {asset}"
        else:
            hold, why = bonds, f"{signal}: labour trigger and {asset} at or below its SMA, {bonds}"
        ctx.log(why)
        hold_only(ctx, hold, why)


if __name__ == "__main__":
    run(CurveRecession)
