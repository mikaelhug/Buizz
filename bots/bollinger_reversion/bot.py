from datetime import date
from typing import NamedTuple

import numpy as np
import numpy.typing as npt
from buizz_bots.preclose import ProxyLog, session_so_far

from buizz import Bot, Context, History, run


class Rules(NamedTuple):
    band_period: int
    band_sigma: float
    trend_sma: int

    @property
    def lookback(self) -> int:
        return max(self.trend_sma, self.band_period)


class Signal(NamedTuple):
    close: float
    middle: float
    sigma: float
    trend_sma: float | None

    @property
    def z(self) -> float:
        return (self.close - self.middle) / self.sigma if self.sigma > 0 else 0.0

    def enters(self, rules: Rules) -> bool:
        below_band = self.close < self.middle - rules.band_sigma * self.sigma
        return below_band and (self.trend_sma is None or self.close > self.trend_sma)

    def entry_reason(self, rules: Rules) -> str:
        band = f"close {-self.z:.2f} sigma below the {rules.band_period}-day mean {self.middle:.2f}"
        if self.trend_sma is None:
            return f"{band}, no trend filter"
        return f"{band}, above the {rules.trend_sma}-day SMA {self.trend_sma:.2f}"

    def exits(self) -> bool:
        return self.close >= self.middle


def signal(closes: npt.NDArray[np.float64], rules: Rules) -> Signal | None:
    if len(closes) < rules.lookback:
        return None
    band = closes[-rules.band_period :]
    trend = float(closes[-rules.trend_sma :].mean()) if rules.trend_sma > 0 else None
    return Signal(float(closes[-1]), float(band.mean()), float(band.std()), trend)


def sessions_held(history: History, entered: date) -> int:
    days = history.start.astype("datetime64[ns]").astype("datetime64[D]")
    return int((days > np.datetime64(entered)).sum()) + 1


def read_signals(ctx: Context, history: dict[str, History], rules: Rules, held: list[str]) -> dict[str, Signal]:
    proxy = ProxyLog(ctx)
    readings: dict[str, Signal] = {}
    for symbol, bars in history.items():
        bar = session_so_far(ctx, symbol)
        if len(bars.close) == 0 or bar is None:
            continue
        official = signal(bars.close, rules)
        if official is not None:
            proxy.check(symbol, bars, {"exit": official.exits(), "entry": official.enters(rules)})
        current = signal(np.append(bars.close, bar.close), rules)
        if current is None:
            continue
        readings[symbol] = current
        if symbol in held:
            proxy.record(symbol, "exit", current.exits())
        else:
            proxy.record(symbol, "entry", current.enters(rules))
    proxy.save()
    return readings


class BollingerReversion(Bot):
    def on_step(self, ctx: Context) -> None:
        params = ctx.params
        rules = Rules(params["band_period"], params["band_sigma"], params["trend_sma"])
        max_positions: int = params["max_positions"]
        max_hold: int = params["max_hold"]
        today = ctx.session.date
        held = sorted(symbol for symbol, position in ctx.portfolio.positions.items() if position.qty != 0)
        entries: dict[str, str] = {
            symbol: ctx.state.get("entries", {}).get(symbol, today.isoformat()) for symbol in held
        }
        symbols = sorted(set(ctx.universe) | set(held))
        history = ctx.history(symbols, "1d", rules.lookback, adjust="all")
        readings = read_signals(ctx, history, rules, held)
        equity = float(ctx.portfolio.equity)
        staying_value = 0.0
        for symbol in held:
            reading = readings.get(symbol)
            days = sessions_held(history[symbol], date.fromisoformat(entries[symbol]))
            if reading is not None and reading.exits():
                reason = f"last {reading.close:.2f} back at the {rules.band_period}-day mean {reading.middle:.2f}"
            elif days >= max_hold:
                reason = f"time exit after {days} sessions"
            else:
                closes = history[symbol].close
                price = reading.close if reading is not None else float(closes[-1]) if len(closes) else 0.0
                staying_value += abs(ctx.position(symbol).qty) * price
                continue
            del entries[symbol]
            ctx.order_target_weight(symbol, 0, at="close", reason=reason)
        candidates = sorted(
            (reading.z, symbol)
            for symbol, reading in readings.items()
            if symbol in ctx.universe and symbol not in held and reading.enters(rules)
        )[: max(max_positions - len(entries), 0)]
        if candidates and equity > 0:
            headroom = max(1 - staying_value / equity, 0.0)
            weight = min(1 / max_positions, headroom / len(candidates))
            for _, symbol in candidates:
                ctx.order_target_weight(symbol, weight, at="close", reason=readings[symbol].entry_reason(rules))
                entries[symbol] = today.isoformat()
        ctx.state["entries"] = entries


if __name__ == "__main__":
    run(BollingerReversion)
