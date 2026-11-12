from datetime import UTC, datetime, timedelta

import numpy as np
import numpy.typing as npt

from buizz import Bot, Context, History, Session, run, to_ns

SYMBOL = "SPY"
DECISION_INTERVAL = timedelta(minutes=30)
AUCTION_CUTOFF = timedelta(minutes=10)
MINUTE = timedelta(minutes=1)
SESSION_MINUTES = 390
MAX_HISTORY_BARS = 10_000


def marks(session: Session) -> list[datetime]:
    found: list[datetime] = []
    mark = session.open + DECISION_INTERVAL
    while mark < session.close:
        found.append(mark)
        mark += DECISION_INTERVAL
    return found


def mark_key(session: Session, mark: datetime) -> str:
    return str((mark - session.open) // MINUTE)


def within(history: History, session: Session) -> npt.NDArray[np.bool_]:
    return (history.start >= to_ns(session.open)) & (history.start < to_ns(session.close))


def move(opening: float, price: float) -> float:
    return abs(price / opening - 1)


def recorded_moves(ctx: Context, lookback: int) -> dict[str, list[float]]:
    today = ctx.session.date
    history = ctx.history([SYMBOL], "1m", min(MAX_HISTORY_BARS, (lookback + 1) * SESSION_MINUTES))[SYMBOL]
    first = datetime.fromtimestamp(int(history.start[0]) / 1e9, UTC).date()
    moves: dict[str, list[float]] = {}
    for earlier in ctx.calendar(first, today - timedelta(days=1))[1:]:
        inside = within(history, earlier)
        if not inside.any():
            continue
        starts = history.start[inside]
        closes = history.close[inside]
        opening = float(history.open[inside][0])
        for mark in marks(earlier):
            visible = starts < to_ns(mark)
            if visible.any():
                moves.setdefault(mark_key(earlier, mark), []).append(move(opening, float(closes[visible][-1])))
    return {key: values[-lookback:] for key, values in moves.items()}


class NoiseBoundary(Bot):
    def on_step(self, ctx: Context) -> None:
        session = ctx.session
        if ctx.clock == session.close - AUCTION_CUTOFF:
            held = ctx.position(SYMBOL).qty
            if held != 0:
                ctx.order(SYMBOL, -held, at="close", reason="flat at the close")
            return
        elapsed = ctx.clock - session.open
        if ctx.clock >= session.close or elapsed % DECISION_INTERVAL:
            return

        lookback: int = ctx.params["lookback"]
        today = session.date.isoformat()
        previous = ctx.calendar(session.date - timedelta(days=10), session.date - timedelta(days=1))[-1].date
        if ctx.state.get("moves_session") not in (today, previous.isoformat()):
            ctx.state["moves"] = recorded_moves(ctx, lookback)
        ctx.state["moves_session"] = today
        moves: dict[str, list[float]] = ctx.state["moves"]

        history = ctx.history([SYMBOL], "1m", elapsed // MINUTE)[SYMBOL]
        inside = within(history, session)
        if not inside.any():
            ctx.log(f"no {SYMBOL} minute bars in {session.date} by {ctx.clock:%H:%M}: no decision", level="warn")
            return
        opening = float(history.open[inside][0])
        price = float(history.close[inside][-1])
        typical = (history.high[inside] + history.low[inside] + history.close[inside]) / 3
        vwap = float(np.sum(typical * history.volume[inside]) / np.sum(history.volume[inside]))

        if ctx.state.get("day") != today:
            self.size_the_day(ctx, today, opening)
        shares: int = ctx.state["shares"]

        key = mark_key(session, ctx.clock)
        recorded = moves.setdefault(key, [])
        if len(recorded) == lookback and shares > 0:
            previous_close: float = ctx.state["previous_close"]
            sigma = float(np.mean(recorded)) * ctx.params["vol_multiplier"]
            upper = max(opening, previous_close) * (1 + sigma)
            lower = min(opening, previous_close) * (1 - sigma)
            self.decide(ctx, price, upper, lower, vwap, shares)
        recorded.append(move(opening, price))
        del recorded[:-lookback]

    def size_the_day(self, ctx: Context, today: str, opening: float) -> None:
        window: int = ctx.params["vol_window"]
        closes = ctx.history([SYMBOL], "1d", window + 1, adjust="all")[SYMBOL].close
        ctx.state["day"] = today
        ctx.state["shares"] = 0
        ctx.state["leverage"] = 0.0
        if len(closes) <= window:
            return
        ctx.state["previous_close"] = float(closes[-1])
        daily_vol = float(np.std(closes[1:] / closes[:-1] - 1, ddof=1))
        leverage = min(float(ctx.params["max_leverage"]), ctx.params["target_daily_vol"] / daily_vol)
        ctx.state["leverage"] = leverage
        ctx.state["shares"] = round(float(ctx.portfolio.equity) * leverage / opening)

    def decide(self, ctx: Context, price: float, upper: float, lower: float, vwap: float, shares: int) -> None:
        held = ctx.position(SYMBOL).qty
        sizing = f"{shares} shares at {ctx.state['leverage']:.2f}x"
        long_stop = max(upper, vwap)
        short_stop = min(lower, vwap)
        if price > long_stop:
            target = shares
            reason = f"{price:.2f} above the noise area {upper:.2f} and VWAP {vwap:.2f}, {sizing}"
        elif price < short_stop:
            target = -shares
            reason = f"{price:.2f} below the noise area {lower:.2f} and VWAP {vwap:.2f}, {sizing}"
        elif held > 0:
            target = 0
            level = "the noise area" if upper >= vwap else "VWAP"
            reason = f"trailing stop: {price:.2f} not above {level} {long_stop:.2f}"
        elif held < 0:
            target = 0
            level = "the noise area" if lower <= vwap else "VWAP"
            reason = f"trailing stop: {price:.2f} not below {level} {short_stop:.2f}"
        else:
            return
        if target != held:
            ctx.order(SYMBOL, target - held, reason=reason)


if __name__ == "__main__":
    run(NoiseBoundary)
