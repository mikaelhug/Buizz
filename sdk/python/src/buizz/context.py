from __future__ import annotations

import math
from datetime import UTC, date, datetime, time, timedelta
from decimal import Decimal
from numbers import Integral, Real
from typing import TYPE_CHECKING, Any, cast, get_args

import msgspec
import numpy as np
import numpy.typing as npt

from buizz.errors import ProtocolError, StateTooLarge, UsageError
from buizz.wire import (
    STATE_LIMIT,
    Ack,
    Adjust,
    BarBuffer,
    CalendarRequest,
    CalendarResult,
    Event,
    FundamentalsRequest,
    FundamentalsResult,
    HistoryRequest,
    HistoryResult,
    Limit,
    LogLevel,
    LogLine,
    Market,
    Metric,
    OrderRequest,
    Position,
    QtySize,
    Ready,
    Request,
    Result,
    SessionFrame,
    Step,
    Timeframe,
    UniverseRequest,
    UniverseResult,
    WeightSize,
    WorkingOrder,
)

if TYPE_CHECKING:
    from collections.abc import Callable, Mapping

type QueryFn = Callable[[Request], Result]

EPOCH = datetime(1970, 1, 1, tzinfo=UTC)
INTRADAY_STAMP = "%Y-%m-%dT%H:%M"
MAX_HISTORY_BARS = 10_000
MAX_UNIVERSE_SYMBOLS = 10_000
SHARE_LIMIT = 2**63
DECIMAL_MAX = Decimal(2**96 - 1)
DECIMAL_MIN = Decimal("1e-28")
METRICS: frozenset[str] = frozenset(get_args(Metric.__value__))
TIMEFRAMES: frozenset[str] = frozenset(get_args(Timeframe.__value__))
ADJUSTMENTS: frozenset[str] = frozenset(get_args(Adjust.__value__))
STATE_SCALARS = (str, bool)
STATE_INT_RANGE = range(-(2**63), 2**64)


def from_ns(ns: int) -> datetime:
    return EPOCH + timedelta(microseconds=ns // 1000)


def to_ns(moment: datetime) -> int:
    if moment.tzinfo is None:
        raise ValueError(f"to_ns needs a timezone-aware datetime, got naive {moment}")
    return (moment - EPOCH) // timedelta(microseconds=1) * 1000


def integer(name: str, value: object) -> int:
    if isinstance(value, bool) or not isinstance(value, Integral):
        raise TypeError(f"{name} must be an int, got {type(value).__name__} {value!r}")
    return int(value)


def wire_decimal(name: str, value: object) -> Decimal:
    if isinstance(value, Decimal):
        number = value
    elif isinstance(value, bool) or not isinstance(value, Real):
        raise TypeError(f"{name} must be a number, got {type(value).__name__} {value!r}")
    elif isinstance(value, Integral):
        number = Decimal(int(value))
    else:
        number = Decimal(str(float(value)))
    if not number.is_finite():
        raise ValueError(f"{name} must be finite, got {value}")
    if number != 0 and not DECIMAL_MIN <= abs(number) <= DECIMAL_MAX:
        raise ValueError(f"{name} {value} is outside the decimal range the spine accepts")
    return number


def day(name: str, value: object) -> date:
    if isinstance(value, datetime) or not isinstance(value, date):
        raise TypeError(f"{name} must be a date, got {type(value).__name__} {value!r}")
    return value


def whole_shares(qty: object) -> int:
    shares = integer("qty", qty)
    if not -SHARE_LIMIT < shares < SHARE_LIMIT:
        raise ValueError(f"qty {shares} does not fit a signed 64-bit share count")
    return shares


def history_bars(bars: object) -> int:
    count = integer("bars", bars)
    if not 1 <= count <= MAX_HISTORY_BARS:
        raise ValueError(f"bars must be between 1 and {MAX_HISTORY_BARS}, got {count}")
    return count


def check_state(value: object, path: str) -> None:
    kind = type(value)
    if kind is dict:
        for key, item in cast("dict[object, object]", value).items():
            if type(key) is not str:
                raise TypeError(f"{path} has the {type(key).__name__} key {key!r}; ctx.state keys must be str")
            check_state(item, f"{path}[{key!r}]")
    elif kind is list:
        for index, item in enumerate(cast("list[object]", value)):
            check_state(item, f"{path}[{index}]")
    elif kind is float:
        if not math.isfinite(cast("float", value)):
            raise ValueError(f"{path} is {value}; ctx.state floats must be finite")
    elif kind is int:
        if cast("int", value) not in STATE_INT_RANGE:
            raise ValueError(f"{path} is {value}, too large for ctx.state")
    elif value is not None and kind not in STATE_SCALARS:
        raise TypeError(
            f"{path} is a {kind.__name__}; ctx.state holds only dict (str keys), list, str, int, float, "
            "bool and None, so that it reads back unchanged after a restart"
        )


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


def session_of(frame: SessionFrame) -> Session:
    return Session(frame.date, from_ns(frame.open), from_ns(frame.close))


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
    metric: Metric
    period_type: str
    period_start: date | None
    period_end: date
    value: Decimal
    form: str
    filed: date
    available_at: datetime


def split_ratio(raw: History, split_adjusted: History, day: date) -> float | None:
    end_of_day = to_ns(datetime.combine(day + timedelta(days=1), time.min, tzinfo=UTC))
    index = int(np.searchsorted(raw.start, end_of_day, side="left")) - 1
    if index < 0 or index >= len(split_adjusted.close) or split_adjusted.close[index] <= 0:
        return None
    return float(raw.close[index] / split_adjusted.close[index])


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
        history[symbol] = History(symbol, **{name: values[lo:hi] for name, values in columns.items()})
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
        return from_ns(self._current("ctx.clock").clock)

    @property
    def session(self) -> Session:
        return session_of(self._current("ctx.session").session)

    @property
    def universe(self) -> list[str]:
        return self._universe

    @property
    def bars(self) -> dict[str, Bar]:
        if self._bars is None:
            self._bars = last_bars(self._current("ctx.bars").bars)
        return self._bars

    @property
    def events(self) -> list[Event]:
        return self._current("ctx.events").events

    @property
    def portfolio(self) -> Portfolio:
        return Portfolio(self._cash, self._equity, dict(self._positions), self._working)

    @property
    def state(self) -> dict[str, Any]:
        return self._state

    def position(self, symbol: str) -> Position:
        return self._positions.get(symbol) or Position(symbol, 0, Decimal(0))

    def history(
        self, symbols: list[str], tf: Timeframe = "1d", bars: int | np.integer[Any] = 1, adjust: Adjust = "raw"
    ) -> dict[str, History]:
        self._current("ctx.history")
        if tf not in TIMEFRAMES:
            raise ValueError(f"tf must be one of {sorted(TIMEFRAMES)}, got {tf!r}")
        if adjust not in ADJUSTMENTS:
            raise ValueError(f"adjust must be one of {sorted(ADJUSTMENTS)}, got {adjust!r}")
        result = self._query(HistoryRequest(symbols, tf, history_bars(bars), adjust))
        if not isinstance(result, HistoryResult):
            raise ProtocolError(f"history query answered with {type(result).__name__}")
        return split_history(result.bars, symbols)

    def fundamentals(self, symbols: list[str], metrics: list[Metric]) -> list[FundamentalPoint]:
        self._current("ctx.fundamentals")
        unknown = [metric for metric in metrics if metric not in METRICS]
        if unknown:
            raise ValueError(f"unknown fundamental metrics {unknown}; the metrics are {sorted(METRICS)}")
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

    def calendar(self, start: date, end: date) -> list[Session]:
        self._current("ctx.calendar")
        result = self._query(CalendarRequest(day("start", start), day("end", end)))
        if not isinstance(result, CalendarResult):
            raise ProtocolError(f"calendar query answered with {type(result).__name__}")
        return [session_of(frame) for frame in result.sessions]

    def universe_at(self, on: date, top_dollar_volume: int | np.integer[Any]) -> list[str]:
        self._current("ctx.universe_at")
        count = integer("top_dollar_volume", top_dollar_volume)
        if not 1 <= count <= MAX_UNIVERSE_SYMBOLS:
            raise ValueError(f"top_dollar_volume must be between 1 and {MAX_UNIVERSE_SYMBOLS}, got {count}")
        result = self._query(UniverseRequest(day("on", on), count))
        if not isinstance(result, UniverseResult):
            raise ProtocolError(f"universe query answered with {type(result).__name__}")
        return result.symbols

    def order(
        self,
        symbol: str,
        qty: int | np.integer[Any],
        *,
        reason: str,
        limit_price: Decimal | float | None = None,
        client_id: str | None = None,
    ) -> str:
        return self._submit(symbol, QtySize(whole_shares(qty)), reason, limit_price, client_id)

    def order_target_weight(
        self,
        symbol: str,
        weight: Decimal | float,
        *,
        reason: str,
        limit_price: Decimal | float | None = None,
        client_id: str | None = None,
    ) -> str:
        return self._submit(symbol, WeightSize(wire_decimal("weight", weight)), reason, limit_price, client_id)

    def rebalance(
        self,
        weights: Mapping[str, Decimal | float],
        *,
        exit_reason: str,
        reason: str | Mapping[str, str] | None = None,
    ) -> None:
        self._current("ctx.rebalance")
        targets = {symbol: wire_decimal(f"weight of {symbol}", weight) for symbol, weight in weights.items()}
        reasons = {} if reason is None else dict.fromkeys(targets, reason) if isinstance(reason, str) else dict(reason)
        if missing := sorted(targets.keys() - reasons.keys()):
            raise ValueError(f"rebalance needs a reason for {missing}")
        for order in self._working:
            self.cancel(order.client_id)
        for symbol, weight in targets.items():
            if weight != 0 or self.position(symbol).qty != 0:
                self.order_target_weight(symbol, weight, reason=reasons[symbol])
        for symbol, position in self._positions.items():
            if position.qty != 0 and symbol not in targets:
                self.order_target_weight(symbol, 0, reason=exit_reason)

    def cancel(self, client_id: str) -> None:
        self._current("ctx.cancel")
        self._cancels.append(client_id)

    def log(self, message: str, level: LogLevel = "info") -> None:
        self._current("ctx.log")
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
        ack = Ack(self._current("the ack").seq, self._orders, self._cancels, self._changed_state(), self._logs)
        self.close()
        return ack

    def close(self) -> None:
        self._step = None
        self._bars = None
        self._orders, self._cancels, self._logs = [], [], []

    def _current(self, member: str) -> Step:
        if self._step is None:
            raise UsageError(f"{member} is only available inside on_step")
        return self._step

    def _changed_state(self) -> bytes | None:
        check_state(self._state, "ctx.state")
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
        self._current("placing an order")
        client_id = client_id or self._next_client_id(symbol)
        order_type = Market() if limit_price is None else Limit(wire_decimal("limit_price", limit_price))
        self._orders.append(OrderRequest(client_id, symbol, size, order_type, "day", reason))
        return client_id

    def _next_client_id(self, symbol: str) -> str:
        step = self._current("placing an order")
        stamp = (
            step.session.date.isoformat() if self._timeframe == "1d" else from_ns(step.clock).strftime(INTRADAY_STAMP)
        )
        base = f"{symbol}-{stamp}"
        count = self._issued.get(base, 0)
        self._issued[base] = count + 1
        return base if count == 0 else f"{base}-{count}"
