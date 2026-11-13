import json
from datetime import date, timedelta

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
        universes: dict[str, object] = {}
        for day in ctx.params.get("universe_dates", []):
            try:
                universes[day] = ctx.universe_at(date.fromisoformat(day), ctx.params["universe_top"])
            except SpineError as error:
                universes[day] = error.code
        series = ctx.params.get("macro", [])
        start, end = (date.fromisoformat(ctx.params.get(key, "2000-01-01")) for key in ("macro_from", "macro_to"))
        macro = [
            [
                point.series,
                point.date.isoformat(),
                float(point.value),
                point.published.isoformat(),
                f"{point.available_at:{STAMP}}",
            ]
            for point in (ctx.macro(series, start, end) if series else [])
        ]
        short_symbols: list[str] = ctx.params.get("short", [])
        short_since = date.fromisoformat(ctx.params.get("short_since", "2000-01-01"))
        short_interest = [
            [
                position.symbol,
                position.settlement.isoformat(),
                position.shares,
                position.published.isoformat(),
                f"{position.available_at:{STAMP}}",
            ]
            for position in (ctx.short_interest(short_symbols, short_since) if short_symbols else [])
        ]
        short_volume = [
            [volume.symbol, volume.date.isoformat(), f"{volume.available_at:{STAMP}}"]
            for volume in (ctx.short_volume(short_symbols, short_since) if short_symbols else [])
        ]
        report = {
            "clock": clock,
            "history": history,
            "fundamentals": fundamentals,
            "calendar": calendar,
            "universe": universes,
            "macro": macro,
            "short_interest": short_interest,
            "short_volume": short_volume,
        }
        ctx.log(json.dumps(report))
        if clock in ctx.params.get("order_at", []):
            ctx.order("AAA", 1, reason=f"probe order decided at {clock}")


if __name__ == "__main__":
    run(Probe)
