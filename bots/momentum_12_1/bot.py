from buizz import Bot, Context, run


class Momentum121(Bot):
    def on_step(self, ctx: Context) -> None:
        month = ctx.session.date.strftime("%Y-%m")
        if ctx.state.get("month") == month:
            return
        lookback: int = ctx.params["lookback"]
        skip: int = ctx.params["skip"]
        returns: dict[str, float] = {}
        for symbol, history in ctx.history(ctx.universe, "1d", lookback + 1, "all").items():
            if len(history.close) == lookback + 1 and history.close[0] > 0:
                returns[symbol] = float(history.close[-1 - skip] / history.close[0] - 1)
        if not returns:
            return
        ctx.state["month"] = month
        count = max(1, int(len(returns) * ctx.params["top_fraction"]))
        winners = sorted(returns, key=lambda symbol: returns[symbol], reverse=True)[:count]
        ctx.rebalance(
            dict.fromkeys(winners, float(ctx.params["gross"]) / count),
            reason={symbol: f"12-1 return {returns[symbol]:.1%}, top {count} of {len(returns)}" for symbol in winners},
            exit_reason="dropped out of the top decile",
        )


if __name__ == "__main__":
    run(Momentum121)
