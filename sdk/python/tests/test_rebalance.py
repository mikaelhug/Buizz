from __future__ import annotations

from datetime import date
from decimal import Decimal

from buizz.context import Context
from buizz.wire import (
    BarBuffer,
    Holdings,
    Market,
    QtySize,
    Ready,
    Result,
    SessionFrame,
    Step,
    Stop,
    WeightSize,
    WorkingOrder,
)

NO_BARS = BarBuffer([], b"", b"", b"", b"", b"", b"", b"")


def unanswered(request: object) -> Result:
    raise AssertionError(f"rebalance queried {request}")


def working(symbol: str, client_id: str, order_type: Market | Stop) -> WorkingOrder:
    return WorkingOrder(1, client_id, symbol, "buy", 100, 0, order_type)


def test_rebalance_before_the_close_exits_what_the_session_will_still_fill() -> None:
    holdings = Holdings(
        [],
        [
            working("AAA", "AAA-buy", Market()),
            working("BBB", "BBB-buy", Market()),
            working("CCC", "CCC-breakout", Stop(Decimal(50))),
        ],
    )
    cash = Decimal(10_000)
    ready = Ready(
        1, "bot", "backtest", "1d", "before_close:10m", {}, [], date(2024, 12, 2), None, 0, None, cash, cash, holdings
    )
    ctx = Context(ready, unanswered)
    ctx.begin(Step(1, 0, SessionFrame(date(2024, 12, 2), 0, 0), None, NO_BARS, [], cash, cash, None))

    ctx.rebalance({"BBB": 0}, exit_reason="flat", reason="drop BBB")

    ack = ctx.finish()
    assert ack.cancels == ["AAA-buy", "BBB-buy", "CCC-breakout"]
    assert [(order.symbol, order.size) for order in ack.orders] == [
        ("BBB", WeightSize(Decimal(0))),
        ("AAA", WeightSize(Decimal(0))),
    ]
    assert not any(order.size == QtySize(0) for order in ack.orders)
