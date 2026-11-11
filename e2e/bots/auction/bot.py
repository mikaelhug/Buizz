from buizz import Bot, Context, run


class Auction(Bot):
    def on_step(self, ctx: Context) -> None:
        bar = ctx.bars["AAA"]
        daily = ctx.history(["AAA"], "1d", 1)["AAA"]
        ctx.log(
            f"step at {ctx.clock:%Y-%m-%dT%H:%M:%SZ}: so far {bar.open:.2f} {bar.high:.2f} {bar.low:.2f} "
            f"{bar.close:.2f} {bar.volume:.0f}, last daily close {daily.close[-1]:.2f}"
        )
        if ctx.state.get("traded"):
            return
        ctx.state["traded"] = True
        ctx.order("AAA", 10, at="close", reason="buy the close")
        ctx.order("AAA", -10, at="open", reason="sell the next open")
        ctx.order("BBB", 10, stop_price=ctx.params["breakout"], tif="gtc", reason="breakout")


if __name__ == "__main__":
    run(Auction)
