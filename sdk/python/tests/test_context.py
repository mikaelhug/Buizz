from __future__ import annotations

from datetime import date
from decimal import Decimal
from typing import TYPE_CHECKING, Any

import pytest

from buizz.context import Context
from buizz.wire import (
    BarBuffer,
    Holdings,
    Market,
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


def stepped(wake: str, holdings: Holdings) -> Context:
    ready = Ready(1, "bot", "backtest", "1d", wake, {}, [], date(2024, 12, 2), None, 0, None, CASH, CASH, holdings)
    ctx = Context(ready, unanswered)
    ctx.begin(Step(1, 0, SessionFrame(date(2024, 12, 2), 0, 0), None, NO_BARS, [], CASH, CASH, None))
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
