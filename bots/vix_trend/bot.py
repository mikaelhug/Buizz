from datetime import timedelta

from buizz_bots.switch import hold_only
from buizz_bots.trend import daily_trend

from buizz import Bot, Context, run


class VixTrend(Bot):
    def on_step(self, ctx: Context) -> None:
        asset: str = ctx.params["asset"]
        bonds: str = ctx.params["bonds"]
        sma_days = int(ctx.params["sma_days"])
        stale_days = int(ctx.params["stale_days"])
        if (trend := daily_trend(ctx, asset, sma_days)) is None:
            ctx.log(f"{asset} has no close today or fewer than {sma_days} closes, no decision", level="warn")
            return
        today = ctx.session.date
        points = ctx.macro(["vix", "vix3m"], today - timedelta(days=stale_days), today)
        three_month = {point.date: point.value for point in points if point.series == "vix3m"}
        paired = [
            (point.date, point.value, three_month[point.date])
            for point in points
            if point.series == "vix" and point.date in three_month
        ]
        if not paired:
            ctx.log(f"no VIX and VIX3M close of the same day in the last {stale_days} days, no decision", level="warn")
            return
        day, vix, vix3m = max(paired)
        ratio = float(vix / vix3m)
        close, sma = trend
        signal = (
            f"VIX/VIX3M {vix}/{vix3m} = {ratio:.3f} ({day}), {asset} {close:.2f} vs its {sma_days}-day SMA {sma:.2f}"
        )
        if ratio > 1 and close < sma:
            hold_only(ctx, bonds, f"{signal}: term structure inverted and {asset} below trend, {bonds}")
        else:
            hold_only(ctx, asset, f"{signal}: not both, {asset}")


if __name__ == "__main__":
    run(VixTrend)
