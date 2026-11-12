from collections.abc import Mapping
from typing import Any

import numpy as np

from buizz import Bar, Context, History, to_ns
from buizz_bots.monthly import last_session


def session_so_far(ctx: Context, symbol: str) -> Bar | None:
    bar = ctx.bars.get(symbol)
    return bar if bar is not None and bar.start == ctx.session.open else None


def with_session_so_far(history: History, bar: Bar) -> History:
    return History(
        symbol=history.symbol,
        start=np.append(history.start, np.int64(to_ns(bar.start))),
        open=np.append(history.open, bar.open),
        high=np.append(history.high, bar.high),
        low=np.append(history.low, bar.low),
        close=np.append(history.close, bar.close),
        volume=np.append(history.volume, bar.volume),
    )


class ProxyLog:
    def __init__(self, ctx: Context) -> None:
        self.ctx = ctx
        self.previous: dict[str, Any] = ctx.state.get("proxy", {"session": "", "decisions": {}})
        self.decisions: dict[str, list[Any]] = {}

    def check(self, symbol: str, official: History, settled: Mapping[str, object]) -> None:
        day = last_session(official).isoformat()
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
