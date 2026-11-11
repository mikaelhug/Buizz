import json
from datetime import timedelta

from buizz import Bot, Context, SpineError, run

STAMP = "%Y-%m-%dT%H:%M:%SZ"


class Probe(Bot):
    def on_step(self, ctx: Context) -> None:
        clock = f"{ctx.clock:{STAMP}}"
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
        calendar: dict[str, object] = {}
        for days in ctx.params.get("calendar_days", []):
            try:
                sessions = ctx.calendar(ctx.session.date, ctx.session.date + timedelta(days=days))
                calendar[str(days)] = [[session.date.isoformat(), f"{session.close:{STAMP}}"] for session in sessions]
            except SpineError as error:
                calendar[str(days)] = error.code
        report = {"clock": clock, "history": history, "fundamentals": fundamentals, "calendar": calendar}
        ctx.log(json.dumps(report))
        if clock in ctx.params.get("order_at", []):
            ctx.order("AAA", 1, reason=f"probe order decided at {clock}")


if __name__ == "__main__":
    run(Probe)
