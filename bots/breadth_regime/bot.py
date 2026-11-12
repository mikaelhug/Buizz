from buizz_bots.signals import SmaCache
from buizz_bots.switch import switch

from buizz import Bot, Context, run


class BreadthRegime(Bot):
    def __init__(self) -> None:
        self.cache = SmaCache()

    def on_step(self, ctx: Context) -> None:
        asset: str = ctx.params["asset"]
        cash: str = ctx.params["cash"]
        window = int(ctx.params["sma_days"])
        smoothing = int(ctx.params["smoothing_days"])
        threshold = float(ctx.params["threshold"])
        members = ctx.universe_at(ctx.session.date, index="sp500")
        refetched = self.cache.refresh(ctx, [*members, asset], window)
        above, counted = self.cache.above(members, window)
        if not counted:
            ctx.log(f"none of {len(members)} members has {window} closes, no breadth", level="warn")
            return
        readings: list[float] = [*ctx.state.get("breadth", []), above / counted][-smoothing:]
        ctx.state["breadth"] = readings
        breadth = f"{above} of {counted} members above their {window}-day SMA ({readings[-1]:.1%})"
        excluded = f"{len(members) - counted} with fewer than {window} closes left out"
        ctx.log(f"{breadth}, {excluded}, {refetched} histories reloaded")
        if len(readings) < smoothing:
            return
        smoothed = sum(readings) / smoothing
        if (market := self.cache.trend(asset, window)) is None:
            ctx.log(f"{asset} has fewer than {window} closes, no decision", level="warn")
            return
        close, sma = market
        signal = f"{smoothing}-day breadth {smoothed:.1%}"
        if smoothed >= threshold:
            risk_on, why = True, f"{signal} at or above {threshold:.0%}"
        elif close > sma:
            risk_on, why = True, f"{signal} below {threshold:.0%} but {asset} {close:.2f} above its SMA {sma:.2f}"
        else:
            risk_on, why = (
                False,
                f"{signal} below {threshold:.0%} and {asset} {close:.2f} at or below its SMA {sma:.2f}",
            )
        switch(ctx, risk_on, asset, cash, why)


if __name__ == "__main__":
    run(BreadthRegime)
