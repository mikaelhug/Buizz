import numpy as np
import numpy.typing as npt

from buizz import Bot, Context, History, run

Closes = tuple[int, npt.NDArray[np.float64]]


def extends(cached: Closes, recent: History) -> bool:
    last_start, closes = cached
    found = np.flatnonzero(recent.start == last_start)
    return found.size > 0 and bool(np.isclose(recent.close[found[0]], closes[-1], rtol=1e-9, atol=0))


class BreadthRegime(Bot):
    def __init__(self) -> None:
        self.closes: dict[str, Closes] = {}

    def refresh(self, ctx: Context, symbols: list[str], window: int) -> int:
        self.closes = {symbol: cached for symbol, cached in self.closes.items() if symbol in symbols}
        recent = ctx.history(symbols, "1d", 2, adjust="all")
        stale: list[str] = []
        for symbol in symbols:
            history = recent[symbol]
            cached = self.closes.get(symbol)
            if history.start.size == 0:
                self.closes.pop(symbol, None)
            elif cached is None or not extends(cached, history):
                stale.append(symbol)
            else:
                fresh = history.start > cached[0]
                self.closes[symbol] = (
                    int(history.start[-1]),
                    np.append(cached[1], history.close[fresh])[-window:],
                )
        if stale:
            for symbol, history in ctx.history(stale, "1d", window, adjust="all").items():
                self.closes[symbol] = (int(history.start[-1]), history.close)
        return len(stale)

    def above_sma(self, symbol: str, window: int) -> tuple[float, float] | None:
        cached = self.closes.get(symbol)
        if cached is None or cached[1].size < window:
            return None
        return float(cached[1][-1]), float(cached[1].mean())

    def on_step(self, ctx: Context) -> None:
        asset: str = ctx.params["asset"]
        cash: str = ctx.params["cash"]
        window = int(ctx.params["sma_days"])
        smoothing = int(ctx.params["smoothing_days"])
        threshold = float(ctx.params["threshold"])
        members = ctx.universe_at(ctx.session.date, index="sp500")
        refetched = self.refresh(ctx, [*members, asset], window)
        trends = [trend for symbol in members if (trend := self.above_sma(symbol, window)) is not None]
        short = len(members) - len(trends)
        if not trends:
            ctx.log(f"none of {len(members)} members has {window} closes, no breadth", level="warn")
            return
        above = sum(close > sma for close, sma in trends)
        readings: list[float] = [*ctx.state.get("breadth", []), above / len(trends)][-smoothing:]
        ctx.state["breadth"] = readings
        breadth = f"{above} of {len(trends)} members above their {window}-day SMA ({readings[-1]:.1%})"
        excluded = f"{short} with fewer than {window} closes left out"
        ctx.log(f"{breadth}, {excluded}, {refetched} histories reloaded")
        if len(readings) < smoothing:
            return
        smoothed = sum(readings) / smoothing
        if (market := self.above_sma(asset, window)) is None:
            ctx.log(f"{asset} has fewer than {window} closes, no decision", level="warn")
            return
        close, sma = market
        signal = f"{smoothing}-day breadth {smoothed:.1%}"
        if smoothed >= threshold:
            hold, why = asset, f"{signal} at or above {threshold:.0%}"
        elif close > sma:
            hold, why = asset, f"{signal} below {threshold:.0%} but {asset} {close:.2f} above its SMA {sma:.2f}"
        else:
            hold, why = cash, f"{signal} below {threshold:.0%} and {asset} {close:.2f} at or below its SMA {sma:.2f}"
        held = {symbol for symbol, position in ctx.portfolio.positions.items() if position.qty != 0}
        if held == {hold}:
            return
        ctx.rebalance({hold: 1.0}, reason=why, exit_reason=why)


if __name__ == "__main__":
    run(BreadthRegime)
