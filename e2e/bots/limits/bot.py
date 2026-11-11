from buizz import Bot, Context, run


class Limits(Bot):
    def on_step(self, ctx: Context) -> None:
        ctx.log(f"step at {ctx.clock:%Y-%m-%dT%H:%M:%SZ} with equity {ctx.portfolio.equity}")
        if not ctx.state.get("asked"):
            ctx.state["asked"] = True
            ctx.order_target_weight("AAA", ctx.params["oversized"], reason="more AAA than the position limit allows")
        ctx.order_target_weight("BBB", ctx.params["weight"], reason="hold BBB at its target weight")


if __name__ == "__main__":
    run(Limits)
