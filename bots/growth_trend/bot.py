from buizz_bots.macro import latest_monthly
from buizz_bots.sessions import last_session_of_month
from buizz_bots.switch import hold_only
from buizz_bots.trend import month_end_trend

from buizz import Bot, Context, run


class GrowthTrend(Bot):
    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        asset: str = ctx.params["asset"]
        bonds: str = ctx.params["bonds"]
        sma_months = int(ctx.params["sma_months"])
        unrate_months = int(ctx.params["unrate_months"])
        if (trend := month_end_trend(ctx, asset, sma_months)) is None:
            ctx.log(f"{asset} has fewer than {sma_months} month-end closes, no decision", level="warn")
            return
        points = latest_monthly(ctx, "unrate", unrate_months)
        if len(points) < unrate_months:
            ctx.log(f"{len(points)} of {unrate_months} unemployment rates visible, no decision", level="warn")
            return
        close, sma = trend
        rate = float(points[-1].value)
        average = sum(float(point.value) for point in points) / unrate_months
        signal = (
            f"unemployment {rate:.1f}% ({points[-1].date:%Y-%m}) vs its {unrate_months}-month average {average:.2f}%, "
            f"{asset} {close:.2f} vs its {sma_months}-month SMA {sma:.2f}"
        )
        if rate > average and close < sma:
            hold, why = bonds, f"{signal}: both trends negative, {bonds}"
        else:
            hold, why = asset, f"{signal}: not both negative, {asset}"
        ctx.log(why)
        hold_only(ctx, hold, why)


if __name__ == "__main__":
    run(GrowthTrend)
