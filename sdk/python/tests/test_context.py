from __future__ import annotations

from datetime import date
from decimal import Decimal
from typing import TYPE_CHECKING, Any

import numpy as np
import pytest

from buizz.context import Context
from buizz.wire import (
    BarBuffer,
    HistoryRequest,
    HistoryResult,
    Holdings,
    LogLine,
    Market,
    Position,
    Ready,
    Result,
    SessionFrame,
    Step,
    Stop,
    WeightSize,
    WorkingOrder,
)

if TYPE_CHECKING:
    from collections.abc import Callable

CASH = Decimal(10_000)
NO_BARS = BarBuffer([], b"", b"", b"", b"", b"", b"", b"")


def unanswered(request: object) -> Result:
    raise AssertionError(f"the bot queried {request}")


def closes(prices: dict[str, float]) -> BarBuffer:
    count = len(prices)
    values = np.array(list(prices.values()), "<f8").tobytes()
    zeros = np.zeros(count, "<f8").tobytes()
    index = np.arange(count, dtype="<u2").tobytes()
    return BarBuffer(list(prices), index, np.zeros(count, "<i8").tobytes(), values, values, values, values, zeros)


def stepped(
    wake: str, holdings: Holdings, bars: BarBuffer = NO_BARS, query: Callable[[Any], Result] = unanswered
) -> Context:
    ready = Ready(1, "bot", "backtest", "1d", wake, {}, [], date(2024, 12, 2), None, 0, None, CASH, CASH, holdings)
    ctx = Context(ready, query)
    ctx.begin(Step(1, 0, SessionFrame(date(2024, 12, 2), 0, 0), None, bars, [], CASH, CASH, None))
    return ctx


def working(symbol: str, client_id: str, order_type: Market | Stop) -> WorkingOrder:
    return WorkingOrder(1, client_id, symbol, "buy", 100, 0, order_type)


def test_rebalance_before_the_close_exits_what_the_session_will_still_fill() -> None:
    ctx = stepped(
        "before_close:10m",
        Holdings(
            [],
            [
                working("AAA", "AAA-buy", Market()),
                working("BBB", "BBB-buy", Market()),
                working("CCC", "CCC-breakout", Stop(Decimal(50))),
            ],
        ),
    )

    ctx.rebalance({"BBB": 0}, exit_reason="flat", reason="drop BBB")

    ack = ctx.finish()
    assert ack.cancels == ["AAA-buy", "BBB-buy", "CCC-breakout"]
    assert [(order.symbol, order.size) for order in ack.orders] == [
        ("BBB", WeightSize(Decimal(0))),
        ("AAA", WeightSize(Decimal(0))),
    ]


def test_rebalance_places_reductions_first_and_skips_sleeves_already_on_target() -> None:
    held = Holdings([Position("AAA", 50, Decimal(100)), Position("CCC", 20, Decimal(100))], [])
    ctx = stepped("bar", held, closes({"BBB": 100.0, "AAA": 100.0, "CCC": 100.0}))

    ctx.rebalance({"BBB": 0.5, "CCC": 0.2, "AAA": 0.2}, exit_reason="out", reason="mix")

    ack = ctx.finish()
    assert [(order.symbol, order.size) for order in ack.orders] == [
        ("AAA", WeightSize(Decimal("0.2"))),
        ("BBB", WeightSize(Decimal("0.5"))),
    ]


def test_rebalance_prices_from_history_and_skips_a_target_never_priced() -> None:
    def history(request: object) -> Result:
        assert request == HistoryRequest(["AAA", "BBB"], "1d", 1, "raw")
        return HistoryResult(closes({"AAA": 100.0}))

    ctx = stepped("bar", Holdings([], []), query=history)

    ctx.rebalance({"AAA": 0.5, "BBB": 0.5}, exit_reason="out", reason="mix")

    ack = ctx.finish()
    assert [(order.symbol, order.size) for order in ack.orders] == [("AAA", WeightSize(Decimal("0.5")))]
    assert ack.logs == [LogLine("warn", "rebalance skipped BBB: no price seen yet for a weight of 0.5")]


MISTAKES: list[tuple[Callable[[Any], object], type[Exception]]] = [
    (lambda ctx: ctx.history("SPY"), TypeError),
    (lambda ctx: ctx.fundamentals("AAPL", ["revenue"]), TypeError),
    (lambda ctx: ctx.order("SPY", 1, reason=None), TypeError),
    (lambda ctx: ctx.order("SPY", 1, reason="buy", client_id=7), TypeError),
    (lambda ctx: ctx.log("x", level="warning"), ValueError),
]


@pytest.mark.parametrize(("mistake", "error"), MISTAKES)
def test_argument_mistakes_raise_at_the_call(mistake: Callable[[Any], object], error: type[Exception]) -> None:
    ctx = stepped("bar", Holdings([], []))

    with pytest.raises(error):
        mistake(ctx)

    ack = ctx.finish()
    assert (ack.orders, ack.logs) == ([], [])
