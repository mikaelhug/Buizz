from buizz_bots.sessions import week_ends
from buizz_bots.volatility import ewma_sessions, ex_ante_volatility

from buizz import Bot, Context, run


class VolTargetSpy(Bot):
    def on_step(self, ctx: Context) -> None:
        if not week_ends(ctx):
            return
        asset: str = ctx.params["asset"]
        cash: str = ctx.params["cash"]
        target_vol = float(ctx.params["target_vol"])
        cap = float(ctx.params["cap"])
        band = float(ctx.params["band"])
        centre_of_mass = 1 / (2 ** (1 / float(ctx.params["vol_half_life_days"])) - 1)
        if asset not in ctx.bars:
            ctx.log(f"no {asset} bar at the close, no decision this week", level="warn")
            return
        sessions = ewma_sessions(centre_of_mass)
        closes = ctx.history([asset], "1d", sessions + 1, adjust="all")[asset].close
        if closes.size <= sessions:
            ctx.log(f"{asset} has {closes.size} of {sessions + 1} closes for its volatility, no decision yet")
            return
        vol = ex_ante_volatility(closes, centre_of_mass)
        weight = min(cap, target_vol / vol)
        rest = max(0.0, 1 - weight)
        held = ctx.position(asset).qty * ctx.bars[asset].close / float(ctx.portfolio.equity)
        signal = f"EWMA vol {vol:.1%}, {target_vol:.0%}/{vol:.1%} capped at {cap:.2f} = {weight:.2f}"
        if abs(weight - held) <= band:
            ctx.log(f"{signal}; {asset} at {held:.2f} is within {band:.2f} of it, no trade")
            return
        ctx.rebalance(
            {asset: weight, cash: rest},
            reason={
                asset: f"{signal}; {asset} at {held:.2f} is more than {band:.2f} away",
                cash: f"the {rest:.2f} of equity not in {asset}",
            },
            exit_reason=f"neither {asset} nor {cash}",
        )


if __name__ == "__main__":
    run(VolTargetSpy)
