import json
from datetime import datetime

from buizz import Bot, Context, run


def iso(moment: datetime) -> str:
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


class Probe(Bot):
    def on_step(self, ctx: Context) -> None:
        clock = iso(ctx.clock)
        history: list[dict[str, object]] = []
        for query in ctx.params.get("queries", []):
            for symbol, rows in ctx.history(ctx.universe, query["tf"], query["bars"], query["adjust"]).items():
                entry: dict[str, object] = {**query, "symbol": symbol, "starts": rows.start.tolist()}
                if query["tf"] == "1d":
                    entry["closes"] = rows.close.tolist()
                    entry["volumes"] = rows.volume.tolist()
                history.append(entry)
        symbols: list[str] = ctx.params.get("fundamentals", [])
        points = ctx.fundamentals(symbols, ctx.params.get("metrics", [])) if symbols else []
        fundamentals = [
            {"symbol": point.symbol, "metric": point.metric, "period_end": point.period_end.isoformat()}
            for point in points
        ]
        ctx.log(json.dumps({"clock": clock, "history": history, "fundamentals": fundamentals}))
        if clock in ctx.params.get("order_at", []):
            ctx.order("AAA", 1, reason=f"probe order decided at {clock}")


if __name__ == "__main__":
    run(Probe)
