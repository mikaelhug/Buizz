from __future__ import annotations

from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from typing import TYPE_CHECKING, Any

import msgspec
import numpy as np
import numpy.typing as npt

from buizz.errors import ProtocolError, StateTooLarge
from buizz.wire import (
    STATE_LIMIT,
    Ack,
    Adjust,
    BarBuffer,
    Event,
    FundamentalsRequest,
    FundamentalsResult,
    HistoryRequest,
    HistoryResult,
    Limit,
    LogLevel,
    LogLine,
    Market,
    OrderRequest,
    Position,
    QtySize,
    Ready,
    Step,
    Timeframe,
    WeightSize,
    WorkingOrder,
)

if TYPE_CHECKING:
    from collections.abc import Callable

type QueryFn = Callable[[HistoryRequest | FundamentalsRequest], HistoryResult | FundamentalsResult]

EPOCH = datetime(1970, 1, 1, tzinfo=UTC)
INTRADAY_STAMP = "%Y-%m-%dT%H:%M"


def from_ns(ns: int) -> datetime:
    return EPOCH + timedelta(microseconds=ns // 1000)


def to_decimal(value: Decimal | float) -> Decimal:
    return value if isinstance(value, Decimal) else Decimal(str(value))


class Bar(msgspec.Struct, frozen=True):
    symbol: str
    start: datetime
    open: float
    high: float
    low: float
    close: float
    volume: float


class Session(msgspec.Struct, frozen=True):
    date: date
    open: datetime
    close: datetime


class Portfolio(msgspec.Struct, frozen=True):
    cash: Decimal
    equity: Decimal
    positions: dict[str, Position]
    working: list[WorkingOrder]


class History(msgspec.Struct, frozen=True):
    symbol: str
    start: npt.NDArray[np.int64]
    open: npt.NDArray[np.float64]
    high: npt.NDArray[np.float64]
    low: npt.NDArray[np.float64]
    close: npt.NDArray[np.float64]
    volume: npt.NDArray[np.float64]


class FundamentalPoint(msgspec.Struct, frozen=True):
    symbol: str
    metric: str
    period_type: str
    period_start: date | None
    period_end: date
    value: Decimal
    form: str
    filed: date
    available_at: datetime


def column(raw: bytes, dtype: str) -> list[Any]:
    return np.frombuffer(raw, dtype).tolist()


def last_bars(buffer: BarBuffer) -> dict[str, Bar]:
    symbols = buffer.symbols
    rows = zip(
        column(buffer.symbol_index, "<u2"),
        column(buffer.start, "<i8"),
        column(buffer.open, "<f8"),
        column(buffer.high, "<f8"),
        column(buffer.low, "<f8"),
        column(buffer.close, "<f8"),
        column(buffer.volume, "<f8"),
        strict=True,
    )
    bars: dict[str, Bar] = {}
    for index, start, open_, high, low, close, volume in rows:
        symbol = symbols[int(index)]
        bars[symbol] = Bar(symbol, from_ns(start), open_, high, low, close, volume)
    return bars


def split_history(buffer: BarBuffer, requested: list[str]) -> dict[str, History]:
    index = np.frombuffer(buffer.symbol_index, "<u2")
    order = np.argsort(index, kind="stable")
    counts = np.bincount(index, minlength=len(buffer.symbols))
    ends = np.cumsum(counts)
    columns = {
        "start": np.frombuffer(buffer.start, "<i8")[order],
        "open": np.frombuffer(buffer.open, "<f8")[order],
        "high": np.frombuffer(buffer.high, "<f8")[order],
        "low": np.frombuffer(buffer.low, "<f8")[order],
        "close": np.frombuffer(buffer.close, "<f8")[order],
        "volume": np.frombuffer(buffer.volume, "<f8")[order],
    }
    present = {symbol: i for i, symbol in enumerate(buffer.symbols)}
    history: dict[str, History] = {}
    for symbol in requested:
        i = present.get(symbol)
        lo, hi = (0, 0) if i is None else (int(ends[i] - counts[i]), int(ends[i]))
        history[symbol] = History(symbol, **{name: column[lo:hi] for name, column in columns.items()})
    return history


class Context:
    def __init__(self, ready: Ready, query: QueryFn) -> None:
        self._query = query
        self._timeframe: Timeframe = ready.timeframe
        self._params = ready.params
        self._universe = ready.universe
        self._cash = ready.cash
        self._equity = ready.equity
        self._positions = {p.symbol: p for p in ready.holdings.positions}
        self._working = ready.holdings.working
        self._persisted = ready.state
        self._state: dict[str, Any] = {} if ready.state is None else msgspec.json.decode(ready.state)
        self._step: Step | None = None
        self._bars: dict[str, Bar] | None = None
        self._orders: list[OrderRequest] = []
        self._cancels: list[str] = []
        self._logs: list[LogLine] = []
        self._issued: dict[str, int] = {}

    @property
    def params(self) -> dict[str, Any]:
        return self._params

    @property
    def clock(self) -> datetime:
        return from_ns(self._current().clock)

    @property
    def session(self) -> Session:
        session = self._current().session
        return Session(session.date, from_ns(session.open), from_ns(session.close))

    @property
    def universe(self) -> list[str]:
        return self._universe

    @property
    def bars(self) -> dict[str, Bar]:
        if self._bars is None:
            self._bars = last_bars(self._current().bars)
        return self._bars

    @property
    def events(self) -> list[Event]:
        return self._current().events

    @property
    def portfolio(self) -> Portfolio:
        return Portfolio(self._cash, self._equity, dict(self._positions), self._working)

    @property
    def state(self) -> dict[str, Any]:
        return self._state

    def position(self, symbol: str) -> Position:
        return self._positions.get(symbol) or Position(symbol, 0, Decimal(0))

    def history(
        self, symbols: list[str], tf: Timeframe = "1d", bars: int = 1, adjust: Adjust = "raw"
    ) -> dict[str, History]:
        result = self._query(HistoryRequest(symbols, tf, bars, adjust))
        if not isinstance(result, HistoryResult):
            raise ProtocolError(f"history query answered with {type(result).__name__}")
        return split_history(result.bars, symbols)

    def fundamentals(self, symbols: list[str], metrics: list[str]) -> list[FundamentalPoint]:
        result = self._query(FundamentalsRequest(symbols, metrics))
        if not isinstance(result, FundamentalsResult):
            raise ProtocolError(f"fundamentals query answered with {type(result).__name__}")
        return [
            FundamentalPoint(
                f.symbol,
                f.metric,
                f.period_type,
                f.period_start,
                f.period_end,
                f.value,
                f.form,
                f.filed,
                from_ns(f.available_at),
            )
            for f in result.points
        ]

    def order(
        self,
        symbol: str,
        qty: int,
        *,
        reason: str,
        limit_price: Decimal | float | None = None,
        client_id: str | None = None,
    ) -> str:
        return self._submit(symbol, QtySize(qty), reason, limit_price, client_id)

    def order_target_weight(
        self,
        symbol: str,
        weight: Decimal | float,
        *,
        reason: str,
        limit_price: Decimal | float | None = None,
        client_id: str | None = None,
    ) -> str:
        return self._submit(symbol, WeightSize(to_decimal(weight)), reason, limit_price, client_id)

    def cancel(self, client_id: str) -> None:
        self._cancels.append(client_id)

    def log(self, message: str, level: LogLevel = "info") -> None:
        self._logs.append(LogLine(level, message))

    def begin(self, step: Step) -> None:
        self._step = step
        self._bars = None
        self._cash = step.cash
        self._equity = step.equity
        if step.universe is not None:
            self._universe = step.universe
        if step.holdings is not None:
            self._positions = {p.symbol: p for p in step.holdings.positions}
            self._working = step.holdings.working
        self._issued = {}

    def finish(self) -> Ack:
        ack = Ack(self._current().seq, self._orders, self._cancels, self._changed_state(), self._logs)
        self._orders, self._cancels, self._logs = [], [], []
        return ack

    def _current(self) -> Step:
        if self._step is None:
            raise ProtocolError("step data is only available inside on_step")
        return self._step

    def _changed_state(self) -> bytes | None:
        encoded = msgspec.json.encode(self._state)
        if encoded == (b"{}" if self._persisted is None else self._persisted):
            return None
        if len(encoded) > STATE_LIMIT:
            raise StateTooLarge(len(encoded), STATE_LIMIT)
        self._persisted = encoded
        return encoded

    def _submit(
        self,
        symbol: str,
        size: QtySize | WeightSize,
        reason: str,
        limit_price: Decimal | float | None,
        client_id: str | None,
    ) -> str:
        client_id = client_id or self._next_client_id(symbol)
        order_type = Market() if limit_price is None else Limit(to_decimal(limit_price))
        self._orders.append(OrderRequest(client_id, symbol, size, order_type, "day", reason))
        return client_id

    def _next_client_id(self, symbol: str) -> str:
        step = self._current()
        stamp = (
            step.session.date.isoformat() if self._timeframe == "1d" else from_ns(step.clock).strftime(INTRADAY_STAMP)
        )
        base = f"{symbol}-{stamp}"
        count = self._issued.get(base, 0)
        self._issued[base] = count + 1
        return base if count == 0 else f"{base}-{count}"
