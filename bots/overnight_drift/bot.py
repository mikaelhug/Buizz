from buizz import Bot, Context, run

SYMBOL = "SPY"


class OvernightDrift(Bot):
    def on_step(self, ctx: Context) -> None:
        weight: float = ctx.params["weight"]
        if ctx.position(SYMBOL).qty != 0:
            ctx.order_target_weight(SYMBOL, 0, at="open", reason="flat for the session")
        ctx.order_target_weight(SYMBOL, weight, at="close", reason="hold overnight")


if __name__ == "__main__":
    run(OvernightDrift)
