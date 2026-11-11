from datetime import timedelta

import numpy as np

from buizz import Bot, Context, run

NANOSECONDS_PER_SECOND = 1_000_000_000


class OpeningRange(Bot):
    def on_step(self, ctx: Context) -> None:
        session = ctx.session
        today = session.date.isoformat()
        if ctx.state.get("day") != today:
            ctx.state["day"] = today
            ctx.state["entered"] = list[str]()
        entered: list[str] = ctx.state["entered"]

        if ctx.clock >= session.close - timedelta(minutes=ctx.params["flatten_minutes_before_close"]):
            for symbol, position in ctx.portfolio.positions.items():
                if position.qty != 0:
                    ctx.order_target_weight(symbol, 0, reason="flat before the close")
            return

        range_minutes: int = ctx.params["range_minutes"]
        elapsed = int((ctx.clock - session.open).total_seconds() // 60)
        if elapsed <= range_minutes:
            return
        open_ns = int(session.open.timestamp()) * NANOSECONDS_PER_SECOND
        range_end_ns = open_ns + range_minutes * 60 * NANOSECONDS_PER_SECOND
        buffer = float(ctx.params["limit_buffer_bps"]) / 10_000
        weight: float = ctx.params["weight"]

        for symbol, history in ctx.history(ctx.universe, "1m", elapsed, "raw").items():
            session_bars = history.start >= open_ns
            opening = session_bars & (history.start < range_end_ns)
            if not opening.any():
                continue
            high = float(np.max(history.high[opening]))
            low = float(np.min(history.low[opening]))
            last = float(history.close[session_bars][-1])
            held = ctx.position(symbol).qty
            if held > 0 and last < low:
                ctx.order_target_weight(symbol, 0, reason=f"stopped: {last:.2f} below range low {low:.2f}")
            elif held < 0 and last > high:
                ctx.order_target_weight(symbol, 0, reason=f"stopped: {last:.2f} above range high {high:.2f}")
            elif held == 0 and symbol not in entered:
                if last > high:
                    entered.append(symbol)
                    ctx.order_target_weight(
                        symbol,
                        weight,
                        limit_price=round(last * (1 + buffer), 2),
                        reason=f"breakout: {last:.2f} above {range_minutes}m range high {high:.2f}",
                    )
                elif last < low:
                    entered.append(symbol)
                    ctx.order_target_weight(
                        symbol,
                        -weight,
                        limit_price=round(last * (1 - buffer), 2),
                        reason=f"breakdown: {last:.2f} below {range_minutes}m range low {low:.2f}",
                    )


if __name__ == "__main__":
    run(OpeningRange)
