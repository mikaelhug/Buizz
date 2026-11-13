from buizz_bots.signals import trend
from buizz_bots.switch import switch

from buizz import Bot, Context, run


class LeverageLongRun(Bot):
    def on_step(self, ctx: Context) -> None:
        market: str = ctx.params["market"]
        asset: str = ctx.params["asset"]
        cash: str = ctx.params["cash"]
        if (signal := trend(ctx, market)) is None:
            ctx.log(f"{market} has no 200-session trend yet, no decision", level="warn")
            return
        holding = asset if signal.bullish else cash
        switch(
            ctx,
            signal.bullish,
            asset,
            cash,
            f"{market} {'above' if signal.bullish else 'at or below'} trend, {holding}: {signal.why}",
        )


if __name__ == "__main__":
    run(LeverageLongRun)
