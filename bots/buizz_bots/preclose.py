from collections.abc import Mapping
from datetime import UTC, datetime
from typing import Any

from buizz import Bar, Context, History


def session_so_far(ctx: Context, symbol: str) -> Bar | None:
    bar = ctx.bars.get(symbol)
    return bar if bar is not None and bar.start == ctx.session.open else None


class ProxyLog:
    def __init__(self, ctx: Context) -> None:
        self.ctx = ctx
        self.previous: dict[str, Any] = ctx.state.get("proxy", {"session": "", "decisions": {}})
        self.decisions: dict[str, list[Any]] = {}

    def check(self, symbol: str, official: History, settled: Mapping[str, object]) -> None:
        day = datetime.fromtimestamp(int(official.start[-1]) / 1e9, UTC).date().isoformat()
        decision = self.previous["decisions"].get(symbol)
        if self.previous["session"] != day or decision is None:
            return
        rule, acted = decision
        if acted != settled[rule]:
            self.ctx.log(
                f"pre-close {rule} decision for {symbol} on {day} was {acted}, the official close gives {settled[rule]}"
            )

    def record(self, symbol: str, rule: str, acted: object) -> None:
        self.decisions[symbol] = [rule, acted]

    def save(self) -> None:
        self.ctx.state["proxy"] = {"session": self.ctx.session.date.isoformat(), "decisions": self.decisions}
