from datetime import datetime

from buizz import Bot, Context, run


def iso(moment: datetime) -> str:
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


class Intraday(Bot):
    def on_step(self, ctx: Context) -> None:
        clock = iso(ctx.clock)
        ctx.log(f"step at {clock}")
        bar = ctx.bars["AAA"]
        if clock == ctx.params["touch_at"]:
            limit = round(bar.close - ctx.params["touch_below"], 2)
            ctx.order("AAA", 10, limit_price=limit, reason=f"bid {limit:.2f} below the last close")
        elif clock == ctx.params["expire_at"]:
            limit = round(bar.close / 2, 2)
            ctx.order("AAA", 10, limit_price=limit, reason=f"bid {limit:.2f} far below the market")


if __name__ == "__main__":
    run(Intraday)
