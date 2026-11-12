from buizz_bots.macro import RELEASE_LAG_MONTHS
from buizz_bots.monthly import first_of_month
from buizz_bots.sessions import week_ends
from buizz_bots.switch import hold_only
from buizz_bots.trend import daily_trend

from buizz import Bot, Context, run


class CreditTrend(Bot):
    def on_step(self, ctx: Context) -> None:
        if not week_ends(ctx):
            return
        asset: str = ctx.params["asset"]
        bonds: str = ctx.params["bonds"]
        sma_days = int(ctx.params["sma_days"])
        spread_months = int(ctx.params["spread_months"])
        if (trend := daily_trend(ctx, asset, sma_days)) is None:
            ctx.log(f"{asset} has no close today or fewer than {sma_days} closes, no decision", level="warn")
            return
        today = ctx.session.date
        points = ctx.macro(["baa", "gs10"], first_of_month(today, spread_months + RELEASE_LAG_MONTHS), today)
        treasury = {point.date: point.value for point in points if point.series == "gs10"}
        spreads = [
            (point.date, float(point.value - treasury[point.date]))
            for point in points
            if point.series == "baa" and point.date in treasury
        ][-spread_months:]
        if len(spreads) < spread_months:
            ctx.log(f"{len(spreads)} of {spread_months} monthly Baa-10y spreads visible, no decision", level="warn")
            return
        close, sma = trend
        month, spread = spreads[-1]
        average = sum(value for _, value in spreads) / spread_months
        signal = (
            f"Baa-10y spread {spread:.2f} ({month:%Y-%m}) vs its {spread_months}-month average {average:.2f}, "
            f"{asset} {close:.2f} vs its {sma_days}-day SMA {sma:.2f}"
        )
        if spread > average and close < sma:
            hold, why = bonds, f"{signal}: spread widening and {asset} below trend, {bonds}"
        else:
            hold, why = asset, f"{signal}: not both, {asset}"
        ctx.log(why)
        hold_only(ctx, hold, why)


if __name__ == "__main__":
    run(CreditTrend)
