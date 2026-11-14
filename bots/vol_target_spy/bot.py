import numpy as np
from buizz_bots.monthly import MAX_SESSIONS_PER_MONTH, session_days
from buizz_bots.sessions import last_session_of_month

from buizz import Bot, Context, run

MONTHS_PER_YEAR = 12
MIN_RETURNS = 10


class VolTargetSpy(Bot):
    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        asset: str = ctx.params["asset"]
        cash: str = ctx.params["cash"]
        target_vol = float(ctx.params["target_vol"])
        cap = float(ctx.params["cap"])
        band = float(ctx.params["band"])
        if asset not in ctx.bars:
            ctx.log(f"no {asset} bar at the close, no decision this month", level="warn")
            return
        history = ctx.history([asset], "1d", MAX_SESSIONS_PER_MONTH + 1, adjust="all")[asset]
        month = session_days(history).astype("datetime64[M]")
        this_month = month == np.datetime64(ctx.session.date, "M")
        closes = history.close[max(int(np.argmax(this_month)) - 1, 0) :] if this_month.any() else history.close[:0]
        returns = closes[1:] / closes[:-1] - 1
        if returns.size < MIN_RETURNS:
            ctx.log(f"{asset} has {returns.size} daily returns this month, no decision yet")
            return
        variance = MONTHS_PER_YEAR * float(np.sum((returns - returns.mean()) ** 2))
        weight = min(cap, target_vol**2 / variance)
        rest = max(0.0, 1 - weight)
        held = ctx.position(asset).qty * ctx.bars[asset].close / float(ctx.portfolio.equity)
        signal = (
            f"{ctx.session.date:%Y-%m} realized variance of {returns.size} daily returns {variance:.4f} "
            f"({variance**0.5:.1%} vol), {target_vol:.0%}²/{variance:.4f} capped at {cap:.2f} = {weight:.2f}"
        )
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
