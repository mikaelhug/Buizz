import json
from datetime import date

from buizz import Bot, Context, run


class Reference(Bot):
    def on_step(self, ctx: Context) -> None:
        symbols: list[str] = ctx.params["symbols"]
        profiles = ctx.instruments(symbols)
        report = {
            "clock": f"{ctx.clock:%Y-%m-%dT%H:%M:%SZ}",
            "universe": ctx.universe,
            "bars": sorted(ctx.bars),
            "instruments": {
                symbol: [profile.exchange, profile.asset_class, profile.sic] for symbol, profile in profiles.items()
            },
            "earnings": [release.accession for release in ctx.earnings(symbols)],
            "forward_pe": [
                [str(point.date), str(point.value), str(point.published), f"{point.available_at:%Y-%m-%dT%H:%M:%SZ}"]
                for point in ctx.macro(["sp500_forward_pe"], date(2024, 1, 1), ctx.session.date)
            ],
        }
        ctx.log(json.dumps(report))
        hedge: str | None = ctx.params.get("hedge")
        if hedge is not None and "hedged" not in ctx.state:
            ctx.state["hedged"] = True
            ctx.order_target_weight(hedge, 0.5, reason=f"{hedge} hedge outside the universe")
        unhedge: str | None = ctx.params.get("unhedge")
        if hedge is not None and unhedge is not None and str(ctx.session.date) == unhedge:
            ctx.order_target_weight(hedge, 0, reason=f"close the {hedge} hedge")


if __name__ == "__main__":
    run(Reference)
