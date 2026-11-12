from buizz_bots.sessions import week_ends
from buizz_bots.signals import credit, trend
from buizz_bots.switch import switch

from buizz import Bot, Context, run


class CreditTrend(Bot):
    def on_step(self, ctx: Context) -> None:
        if not week_ends(ctx):
            return
        asset: str = ctx.params["asset"]
        bonds: str = ctx.params["bonds"]
        sma_days = int(ctx.params["sma_days"])
        spread_months = int(ctx.params["spread_months"])
        if (price := trend(ctx, asset, sma_days)) is None:
            ctx.log(f"{asset} has no close today or fewer than {sma_days} closes, no decision", level="warn")
            return
        if (spread := credit(ctx, spread_months)) is None:
            ctx.log(f"fewer than {spread_months} monthly Baa-10y spreads visible, no decision", level="warn")
            return
        signal = f"{spread.why}, {price.why}"
        if not spread.bullish and not price.bullish:
            why = f"{signal}: spread widening and {asset} below trend, {bonds}"
        else:
            why = f"{signal}: not both, {asset}"
        ctx.log(why)
        switch(ctx, spread.bullish or price.bullish, asset, bonds, why)


if __name__ == "__main__":
    run(CreditTrend)
