from buizz_bots.signals import trend, vix_term
from buizz_bots.switch import switch

from buizz import Bot, Context, run


class VixTrend(Bot):
    def on_step(self, ctx: Context) -> None:
        asset: str = ctx.params["asset"]
        bonds: str = ctx.params["bonds"]
        sma_days = int(ctx.params["sma_days"])
        stale_days = int(ctx.params["stale_days"])
        if (price := trend(ctx, asset, sma_days)) is None:
            ctx.log(f"{asset} has no close today or fewer than {sma_days} closes, no decision", level="warn")
            return
        if (term := vix_term(ctx, stale_days)) is None:
            ctx.log(f"no VIX and VIX3M close of the same day in the last {stale_days} days, no decision", level="warn")
            return
        signal = f"{term.why}, {price.why}"
        if not term.bullish and not price.bullish:
            why = f"{signal}: term structure inverted and {asset} below trend, {bonds}"
        else:
            why = f"{signal}: not both, {asset}"
        switch(ctx, term.bullish or price.bullish, asset, bonds, why)


if __name__ == "__main__":
    run(VixTrend)
