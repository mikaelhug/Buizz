from __future__ import annotations

from datetime import date, datetime
from decimal import Decimal
from typing import Any, Literal

import msgspec
import numpy as np
import numpy.typing as npt

type Timeframe = Literal["1d", "1m"]
type Adjust = Literal["raw", "split", "all"]
type LogLevel = Literal["debug", "info", "warn", "error"]


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


class Position(msgspec.Struct, frozen=True):
    symbol: str
    qty: int
    avg_cost: Decimal


class Portfolio(msgspec.Struct, frozen=True):
    cash: Decimal
    equity: Decimal
    positions: dict[str, Position]


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
    period_type: Literal["FY", "Q"]
    period_start: date | None
    period_end: date
    value: Decimal
    form: str
    filed: date
    available_at: datetime


class Context:
    @property
    def params(self) -> dict[str, Any]:
        raise NotImplementedError

    @property
    def clock(self) -> datetime:
        raise NotImplementedError

    @property
    def session(self) -> Session:
        raise NotImplementedError

    @property
    def universe(self) -> list[str]:
        raise NotImplementedError

    @property
    def bars(self) -> dict[str, Bar]:
        raise NotImplementedError

    @property
    def events(self) -> list[Any]:
        raise NotImplementedError

    @property
    def portfolio(self) -> Portfolio:
        raise NotImplementedError

    @property
    def state(self) -> dict[str, Any]:
        raise NotImplementedError

    def position(self, symbol: str) -> Position:
        raise NotImplementedError

    def history(
        self, symbols: list[str], tf: Timeframe = "1d", bars: int = 1, adjust: Adjust = "raw"
    ) -> dict[str, History]:
        raise NotImplementedError

    def fundamentals(self, symbols: list[str], metrics: list[str]) -> list[FundamentalPoint]:
        raise NotImplementedError

    def order(self, symbol: str, qty: int, *, reason: str, limit_price: Decimal | float | None = None) -> str:
        raise NotImplementedError

    def order_target_weight(
        self, symbol: str, weight: Decimal | float, *, reason: str, limit_price: Decimal | float | None = None
    ) -> str:
        raise NotImplementedError

    def cancel(self, client_id: str) -> None:
        raise NotImplementedError

    def log(self, message: str, level: LogLevel = "info") -> None:
        raise NotImplementedError
