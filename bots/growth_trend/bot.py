from buizz_bots.sessions import last_session_of_month
from buizz_bots.signals import labour, monthly_trend
from buizz_bots.switch import switch

from buizz import Bot, Context, run


class GrowthTrend(Bot):
    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        asset: str = ctx.params["asset"]
        bonds: str = ctx.params["bonds"]
        sma_months = int(ctx.params["sma_months"])
        unrate_months = int(ctx.params["unrate_months"])
        if (price := monthly_trend(ctx, asset, sma_months)) is None:
            ctx.log(f"{asset} has fewer than {sma_months} month-end closes, no decision", level="warn")
            return
        if (jobs := labour(ctx, unrate_months)) is None:
            ctx.log(f"fewer than {unrate_months} unemployment rates visible, no decision", level="warn")
            return
        signal = f"{jobs.why}, {price.why}"
        if not jobs.bullish and not price.bullish:
            why = f"{signal}: both trends negative, {bonds}"
        else:
            why = f"{signal}: not both negative, {asset}"
        ctx.log(why)
        switch(ctx, jobs.bullish or price.bullish, asset, bonds, why)


if __name__ == "__main__":
    run(GrowthTrend)
