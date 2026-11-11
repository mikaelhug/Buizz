import numpy as np

from buizz import Bot, Context, run


class SmaCross(Bot):
    def on_step(self, ctx: Context) -> None:
        fast: int = ctx.params["fast"]
        slow: int = ctx.params["slow"]
        weight: float = ctx.params["weight"]
        for symbol, history in ctx.history(ctx.universe, "1d", slow, "all").items():
            if len(history.close) < slow:
                continue
            fast_sma = float(np.mean(history.close[-fast:]))
            slow_sma = float(np.mean(history.close[-slow:]))
            held = ctx.position(symbol).qty > 0
            if fast_sma > slow_sma and not held:
                ctx.order_target_weight(
                    symbol, weight, reason=f"SMA{fast} {fast_sma:.2f} above SMA{slow} {slow_sma:.2f}"
                )
            elif fast_sma <= slow_sma and held:
                ctx.order_target_weight(symbol, 0, reason=f"SMA{fast} {fast_sma:.2f} below SMA{slow} {slow_sma:.2f}")


if __name__ == "__main__":
    run(SmaCross)
