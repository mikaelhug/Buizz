from buizz import Bot, Context, run


class AuctionLate(Bot):
    def on_step(self, ctx: Context) -> None:
        ctx.order("AAA", 10, at="close", reason="too late for the close")


if __name__ == "__main__":
    run(AuctionLate)
