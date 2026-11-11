import json

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
        }
        ctx.log(json.dumps(report))


if __name__ == "__main__":
    run(Reference)
