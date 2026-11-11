from datetime import timedelta

from buizz import Bot, Context, run


class SixtyForty(Bot):
    def on_step(self, ctx: Context) -> None:
        equity_weight = float(ctx.params["equity_weight"])
        targets: dict[str, float] = {ctx.params["equity"]: equity_weight, ctx.params["bonds"]: 1 - equity_weight}
        invested = any(ctx.position(symbol).qty != 0 for symbol in targets)
        today = ctx.session.date
        following = ctx.calendar(today, today + timedelta(days=10))[1].date
        if invested and following.month == today.month:
            return
        if missing := sorted(targets.keys() - ctx.bars.keys()):
            ctx.log(f"no bar for {missing} at the close, rebalance skipped", level="warn")
            return
        book = float(ctx.portfolio.equity)
        weights = {symbol: ctx.position(symbol).qty * ctx.bars[symbol].close / book for symbol in targets}
        occasion = "month-end rebalance" if invested else "initial allocation"
        sells_first = sorted(targets, key=lambda symbol: weights[symbol] - targets[symbol], reverse=True)
        ctx.rebalance(
            {symbol: targets[symbol] for symbol in sells_first},
            reason={
                symbol: f"{occasion}: {symbol} at {weights[symbol]:.1%} of equity, target {target:.0%}"
                for symbol, target in targets.items()
            },
            exit_reason="not part of the stock/bond mix",
        )


if __name__ == "__main__":
    run(SixtyForty)
