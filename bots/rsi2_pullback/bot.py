from datetime import UTC, datetime
from typing import Any, NamedTuple

import numpy as np
import numpy.typing as npt

from buizz import Bot, Context, run

RSI_WARMUP = 100


class Rules(NamedTuple):
    rsi_period: int
    entry_rsi: float
    trend_sma: int
    exit_sma: int


class Signal(NamedTuple):
    close: float
    rsi: float
    trend_sma: float
    exit_sma: float

    def enters(self, rules: Rules) -> bool:
        return self.close > self.trend_sma and self.rsi < rules.entry_rsi

    def exits(self) -> bool:
        return self.close > self.exit_sma


def wilder_rsi(closes: npt.NDArray[np.float64], period: int) -> float:
    changes = np.diff(closes)
    gains = np.clip(changes, 0, None).tolist()
    losses = np.clip(-changes, 0, None).tolist()
    average_gain = sum(gains[:period]) / period
    average_loss = sum(losses[:period]) / period
    for gain, loss in zip(gains[period:], losses[period:], strict=True):
        average_gain = (average_gain * (period - 1) + gain) / period
        average_loss = (average_loss * (period - 1) + loss) / period
    if average_loss == 0:
        return 100.0
    return 100 - 100 / (1 + average_gain / average_loss)


def signal(closes: npt.NDArray[np.float64], rules: Rules) -> Signal | None:
    if len(closes) < max(rules.trend_sma, rules.exit_sma, rules.rsi_period + 1):
        return None
    return Signal(
        float(closes[-1]),
        wilder_rsi(closes, rules.rsi_period),
        float(closes[-rules.trend_sma :].mean()),
        float(closes[-rules.exit_sma :].mean()),
    )


def check_proxy(ctx: Context, proxy: dict[str, Any], symbol: str, day: str, official: Signal, rules: Rules) -> None:
    decision = proxy["decisions"].get(symbol)
    if proxy["session"] != day or decision is None:
        return
    rule, acted = decision
    settled = official.exits() if rule == "exit" else official.enters(rules)
    if acted != settled:
        ctx.log(f"pre-close {rule} decision for {symbol} on {day} was {acted}, the official close gives {settled}")


class Rsi2Pullback(Bot):
    def on_step(self, ctx: Context) -> None:
        params = ctx.params
        rules = Rules(params["rsi_period"], params["entry_rsi"], params["trend_sma"], params["exit_sma"])
        max_positions: int = params["max_positions"]
        proxy: dict[str, Any] = ctx.state.get("proxy", {"session": "", "decisions": {}})
        history = ctx.history(ctx.universe, "1d", rules.trend_sma + RSI_WARMUP, adjust="all")
        today: dict[str, Signal] = {}
        for symbol in ctx.universe:
            bars = history[symbol]
            bar = ctx.bars.get(symbol)
            if len(bars.close) == 0 or bar is None or bar.start != ctx.session.open:
                continue
            official = signal(bars.close, rules)
            if official is not None:
                day = datetime.fromtimestamp(int(bars.start[-1]) / 1e9, UTC).date().isoformat()
                check_proxy(ctx, proxy, symbol, day, official, rules)
            current = signal(np.append(bars.close, bar.close), rules)
            if current is not None:
                today[symbol] = current
        held = [symbol for symbol in ctx.universe if ctx.position(symbol).qty != 0]
        ctx.state["proxy"] = {
            "session": ctx.session.date.isoformat(),
            "decisions": {
                symbol: ["exit", s.exits()] if symbol in held else ["entry", s.enters(rules)]
                for symbol, s in today.items()
            },
        }
        staying = len(held)
        for symbol in held:
            s = today.get(symbol)
            if s is not None and s.exits():
                staying -= 1
                ctx.order_target_weight(
                    symbol,
                    0,
                    at="close",
                    reason=f"last {s.close:.2f} above the {rules.exit_sma}-day SMA {s.exit_sma:.2f}",
                )
        candidates = sorted((s.rsi, symbol) for symbol, s in today.items() if symbol not in held and s.enters(rules))
        for rsi, symbol in candidates[: max(max_positions - staying, 0)]:
            s = today[symbol]
            ctx.order_target_weight(
                symbol,
                1 / max_positions,
                at="close",
                reason=(
                    f"RSI({rules.rsi_period}) {rsi:.1f} below {rules.entry_rsi} with last {s.close:.2f} "
                    f"above the {rules.trend_sma}-day SMA {s.trend_sma:.2f}"
                ),
            )


if __name__ == "__main__":
    run(Rsi2Pullback)
