from datetime import datetime

from buizz import Bot, Context, run

RESTING = "rest-"


def iso(moment: datetime) -> str:
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


class Tape(Bot):
    def on_step(self, ctx: Context) -> None:
        steps = int(ctx.state.get("steps", 0)) + 1
        ctx.state["steps"] = steps
        bars = dict(sorted(ctx.bars.items()))
        ctx.log(f"step {steps} saw " + " ".join(f"{symbol}@{iso(bar.start)}" for symbol, bar in bars.items()))
        for symbol, bar in bars.items():
            held = ctx.position(symbol).qty
            if bar.close < bar.open and held == 0:
                ctx.order_target_weight(symbol, ctx.params["weight"], reason="red minute")
            elif bar.close > bar.open and held > 0:
                ctx.order_target_weight(symbol, 0, reason="green minute")
        if bars and not any(order.client_id.startswith(RESTING) for order in ctx.portfolio.working):
            symbol, bar = next(iter(bars.items()))
            ctx.order(
                symbol,
                1,
                limit_price=round(bar.close / 2, 2),
                reason="resting bid far below the market",
                client_id=f"{RESTING}{iso(ctx.clock)}",
            )


if __name__ == "__main__":
    run(Tape)
