from datetime import datetime, timedelta

import numpy as np
import numpy.typing as npt

from buizz import Bot, Context, run, to_ns

SYMBOL = "SPY"
HALF_HOUR = timedelta(minutes=30)
MINUTE = timedelta(minutes=1)


def price_at(starts: npt.NDArray[np.int64], closes: npt.NDArray[np.float64], moment: datetime) -> float:
    return float(closes[np.searchsorted(starts, to_ns(moment)) - 1])


def side(signals: list[float]) -> int:
    if all(signal > 0 for signal in signals):
        return 1
    if all(signal <= 0 for signal in signals):
        return -1
    return 0


class IntradayMomentum(Bot):
    def on_step(self, ctx: Context) -> None:
        session = ctx.session
        first_half_hour_end = session.open + HALF_HOUR
        previous_close = float(ctx.history([SYMBOL], "1d", 1, "all")[SYMBOL].close[-1])
        minutes = ctx.history([SYMBOL], "1m", (ctx.clock - session.open) // MINUTE, "all")[SYMBOL]
        today = minutes.start >= to_ns(session.open)
        starts, closes = minutes.start[today], minutes.close[today]
        if starts.size == 0 or starts[0] >= to_ns(first_half_hour_end):
            ctx.log(f"no {SYMBOL} trade in the first half hour of {session.date}: no signal", level="warn")
            return

        signals = {"r1": price_at(starts, closes, first_half_hour_end) / previous_close - 1}
        if ctx.params["use_r12"]:
            signals["r12"] = price_at(starts, closes, ctx.clock) / price_at(starts, closes, ctx.clock - HALF_HOUR) - 1
        described = ", ".join(f"{name} {value:+.2%}" for name, value in signals.items())
        direction = side(list(signals.values()))
        if direction == 0:
            ctx.log(f"{described} disagree: out for the last half hour")
            return

        size: float = ctx.params["size"]
        ctx.order_target_weight(
            SYMBOL, direction * size, reason=f"{described} → last half hour {'long' if direction > 0 else 'short'}"
        )
        ctx.order_target_weight(SYMBOL, 0, at="close", reason="flat at the close")


if __name__ == "__main__":
    run(IntradayMomentum)
