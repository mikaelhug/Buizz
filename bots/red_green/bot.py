from buizz import Bot, Context, run


class RedGreen(Bot):
    def on_step(self, ctx: Context) -> None:
        weight: float = ctx.params["weight"]
        for symbol, bar in ctx.bars.items():
            if bar.close < bar.open and ctx.position(symbol).qty == 0:
                ctx.order_target_weight(symbol, weight, reason="red day")
            elif bar.close > bar.open:
                ctx.order_target_weight(symbol, 0, reason="green day")


if __name__ == "__main__":
    run(RedGreen)
