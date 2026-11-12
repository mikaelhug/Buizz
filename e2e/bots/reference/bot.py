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


if __name__ == "__main__":
    run(Reference)
