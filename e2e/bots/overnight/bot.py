from buizz import Bot, Context, EndReason, run


class Overnight(Bot):
    def __init__(self) -> None:
        self.steps_in_process = 0

    def on_step(self, ctx: Context) -> None:
        steps = int(ctx.state.get("steps", 0)) + 1
        ctx.state["steps"] = steps
        self.steps_in_process += 1
        working = " ".join(sorted(order.client_id for order in ctx.portfolio.working)) or "none"
        ctx.log(f"step {steps}, {self.steps_in_process} in this process, working {working}")
        if steps == 1:
            close = ctx.bars["AAA"].close
            ctx.order("AAA", 1, limit_price=round(close / 2, 2), tif="gtc", reason="rest far below", client_id="rest")

    def on_end(self, ctx: Context, reason: EndReason) -> None:
        print(f"ended with {reason} after step {ctx.state.get('steps')}")


if __name__ == "__main__":
    run(Overnight)
