from buizz_bots.sessions import last_session_of_month

from buizz import Bot, Context, run


class SixtyForty(Bot):
    def on_step(self, ctx: Context) -> None:
        equity_weight = float(ctx.params["equity_weight"])
        targets: dict[str, float] = {ctx.params["equity"]: equity_weight, ctx.params["bonds"]: 1 - equity_weight}
        invested = any(ctx.position(symbol).qty != 0 for symbol in targets)
        if invested and not last_session_of_month(ctx):
            return
        if missing := sorted(targets.keys() - ctx.bars.keys()):
            ctx.log(f"no bar for {missing} at the close, rebalance skipped", level="warn")
            return
        book = float(ctx.portfolio.equity)
        weights = {symbol: ctx.position(symbol).qty * ctx.bars[symbol].close / book for symbol in targets}
        occasion = "month-end rebalance" if invested else "initial allocation"
        ctx.rebalance(
            targets,
            reason={
                symbol: f"{occasion}: {symbol} at {weights[symbol]:.1%} of equity, target {target:.0%}"
                for symbol, target in targets.items()
            },
            exit_reason="not part of the stock/bond mix",
        )


if __name__ == "__main__":
    run(SixtyForty)
