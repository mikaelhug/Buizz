from __future__ import annotations

from datetime import date
from decimal import Decimal
from typing import Any, Literal

import msgspec

PROTOCOL_VERSION = 1
STATE_LIMIT = 64 * 1024

type Timeframe = Literal["1d", "1m"]
type Adjust = Literal["raw", "split", "all"]
type LogLevel = Literal["debug", "info", "warn", "error"]
type EndReason = Literal["completed", "paused", "stopped", "risk_limit", "failed", "disconnected"]
type MacroSeries = Literal["treasury_3m", "tb3ms", "gs10", "aaa", "baa", "cpiaucns"]
type Metric = Literal[
    "revenue",
    "cost_of_revenue",
    "gross_profit",
    "research_development",
    "sga_expense",
    "operating_expenses",
    "operating_income",
    "interest_expense",
    "pretax_income",
    "income_tax",
    "net_income",
    "eps_diluted",
    "total_assets",
    "current_assets",
    "cash_and_equivalents",
    "inventory",
    "receivables",
    "ppe_net",
    "goodwill",
    "current_liabilities",
    "long_term_debt",
    "total_liabilities",
    "equity",
    "operating_cash_flow",
    "depreciation_amortization",
    "stock_based_comp",
    "capex",
    "investing_cash_flow",
    "financing_cash_flow",
    "dividends_paid",
    "share_repurchases",
    "acquisitions",
    "shares_outstanding",
]


class Position(msgspec.Struct, frozen=True):
    symbol: str
    qty: int
    avg_cost: Decimal


class Market(msgspec.Struct, frozen=True, tag_field="type", tag="market"):
    pass


class Limit(msgspec.Struct, frozen=True, tag_field="type", tag="limit"):
    limit_price: Decimal


class WorkingOrder(msgspec.Struct, frozen=True):
    order_id: int
    client_id: str
    symbol: str
    side: Literal["buy", "sell"]
    qty: int
    filled_qty: int
    order_type: Market | Limit


class Holdings(msgspec.Struct, frozen=True):
    positions: list[Position]
    working: list[WorkingOrder]


class BarBuffer(msgspec.Struct, frozen=True):
    symbols: list[str]
    symbol_index: bytes
    start: bytes
    open: bytes
    high: bytes
    low: bytes
    close: bytes
    volume: bytes


class SessionFrame(msgspec.Struct, frozen=True):
    date: date
    open: int
    close: int


class Fill(msgspec.Struct, frozen=True, tag_field="type", tag="fill"):
    order_id: int
    client_id: str
    symbol: str
    at_ns: int = msgspec.field(name="at")
    qty: int
    price: Decimal
    commission: Decimal
    feed: str


class OrderUpdate(msgspec.Struct, frozen=True, tag_field="type", tag="order"):
    order_id: int
    client_id: str
    symbol: str
    filled_qty: int
    status: Literal["working", "filled", "cancelled", "expired", "rejected"]
    reject_reason: str | None


class CorporateAction(msgspec.Struct, frozen=True, tag_field="type", tag="corporate_action"):
    symbol: str
    ex_date: date
    kind: dict[str, Any]


type Event = Fill | OrderUpdate | CorporateAction


class Ready(msgspec.Struct, frozen=True, tag_field="type", tag="ready"):
    run_id: int
    bot: str
    mode: Literal["backtest", "live"]
    timeframe: Timeframe
    wake: str
    params: dict[str, Any]
    universe: list[str]
    start: date
    end: date | None
    seed: int
    state: bytes | None
    cash: Decimal
    equity: Decimal
    holdings: Holdings


class Step(msgspec.Struct, frozen=True, tag_field="type", tag="step"):
    seq: int
    clock: int
    session: SessionFrame
    universe: list[str] | None
    bars: BarBuffer
    events: list[Event]
    cash: Decimal
    equity: Decimal
    holdings: Holdings | None


class FundamentalFact(msgspec.Struct, frozen=True):
    symbol: str
    metric: Metric
    period_type: Literal["FY", "Q"]
    period_start: date | None
    period_end: date
    value: Decimal
    form: str
    filed: date
    available_at: int


class HistoryResult(msgspec.Struct, frozen=True, tag_field="kind", tag="history"):
    bars: BarBuffer


class FundamentalsResult(msgspec.Struct, frozen=True, tag_field="kind", tag="fundamentals"):
    points: list[FundamentalFact]


class CalendarResult(msgspec.Struct, frozen=True, tag_field="kind", tag="calendar"):
    sessions: list[SessionFrame]


class UniverseResult(msgspec.Struct, frozen=True, tag_field="kind", tag="universe"):
    symbols: list[str]


class MacroFact(msgspec.Struct, frozen=True):
    series: MacroSeries
    date: date
    value: Decimal
    published: date
    available_at: int


class MacroResult(msgspec.Struct, frozen=True, tag_field="kind", tag="macro"):
    points: list[MacroFact]


type Result = HistoryResult | FundamentalsResult | CalendarResult | UniverseResult | MacroResult


class Data(msgspec.Struct, frozen=True, tag_field="type", tag="data"):
    id: int
    result: Result


class Error(msgspec.Struct, frozen=True, tag_field="type", tag="error"):
    id: int | None
    code: str
    message: str


class End(msgspec.Struct, frozen=True, tag_field="type", tag="end"):
    reason: EndReason
    message: str | None


class Hello(msgspec.Struct, frozen=True, tag_field="type", tag="hello"):
    protocol: int
    sdk: str


class HistoryRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="history"):
    symbols: list[str]
    timeframe: Timeframe
    bars: int
    adjust: Adjust


class FundamentalsRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="fundamentals"):
    symbols: list[str]
    metrics: list[Metric]


class CalendarRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="calendar"):
    start: date = msgspec.field(name="from")
    end: date = msgspec.field(name="to")


class UniverseRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="universe"):
    date: date
    top_dollar_volume: int


class MacroRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="macro"):
    series: list[MacroSeries]
    start: date = msgspec.field(name="from")
    end: date = msgspec.field(name="to")


type Request = HistoryRequest | FundamentalsRequest | CalendarRequest | UniverseRequest | MacroRequest


class Query(msgspec.Struct, frozen=True, tag_field="type", tag="query"):
    id: int
    request: Request


class QtySize(msgspec.Struct, frozen=True, tag_field="by", tag="qty"):
    qty: int


class WeightSize(msgspec.Struct, frozen=True, tag_field="by", tag="target_weight"):
    weight: Decimal


class OrderRequest(msgspec.Struct, frozen=True):
    client_id: str
    symbol: str
    size: QtySize | WeightSize
    order_type: Market | Limit
    tif: Literal["day"]
    reason: str


class LogLine(msgspec.Struct, frozen=True):
    level: LogLevel
    message: str


class Ack(msgspec.Struct, frozen=True, tag_field="type", tag="ack"):
    seq: int
    orders: list[OrderRequest]
    cancels: list[str]
    state: bytes | None
    logs: list[LogLine]


type Inbound = Ready | Step | Data | Error | End

decode_inbound = msgspec.msgpack.Decoder[Inbound](Inbound).decode
encode = msgspec.msgpack.Encoder().encode
