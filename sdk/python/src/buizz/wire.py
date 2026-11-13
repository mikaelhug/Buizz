from __future__ import annotations

from datetime import date
from decimal import Decimal
from typing import Any, Literal

import msgspec

PROTOCOL_VERSION = 1
STATE_LIMIT = 64 * 1024

type Timeframe = Literal["1d", "1m"]
type Adjust = Literal["raw", "split", "all"]
type AssetClass = Literal["stock", "etf"]
type StockIndex = Literal["sp500"]
type LogLevel = Literal["debug", "info", "warn", "error"]
type EndReason = Literal["completed", "paused", "stopped", "risk_limit", "failed", "session_end", "disconnected"]
type TimeInForce = Literal["day", "gtc"]
type Auction = Literal["open", "close"]
type MacroSeries = Literal[
    "treasury_3m",
    "tb3ms",
    "gs10",
    "aaa",
    "baa",
    "cpiaucns",
    "unrate",
    "sahmrealtime",
    "dgs10",
    "t10y3m",
    "baa10y",
    "icsa",
    "ccsa",
    "indpro",
    "permit",
    "rsafs",
    "vix",
    "vix3m",
    "sp500_forward_pe",
    "sp500_forward_pe_5y_avg",
    "sp500_forward_pe_10y_avg",
    "sp500_forward_eps",
    "sp500_trailing_pe",
    "sp500_energy_forward_pe",
    "sp500_materials_forward_pe",
    "sp500_industrials_forward_pe",
    "sp500_consumer_discretionary_forward_pe",
    "sp500_consumer_staples_forward_pe",
    "sp500_health_care_forward_pe",
    "sp500_financials_forward_pe",
    "sp500_information_technology_forward_pe",
    "sp500_communication_services_forward_pe",
    "sp500_telecom_services_forward_pe",
    "sp500_utilities_forward_pe",
    "sp500_real_estate_forward_pe",
]
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
    "short_term_debt",
    "short_term_borrowings",
    "current_long_term_debt",
    "long_term_debt_noncurrent",
    "preferred_stock",
    "minority_interest",
    "retained_earnings",
    "income_taxes_payable",
    "short_term_investments",
    "convertible_debt",
    "secured_debt",
    "land",
    "buildings",
    "ppe_gross",
    "advertising",
]


class Position(msgspec.Struct, frozen=True):
    symbol: str
    qty: int
    avg_cost: Decimal


class Market(msgspec.Struct, frozen=True, tag_field="type", tag="market"):
    pass


class Limit(msgspec.Struct, frozen=True, tag_field="type", tag="limit"):
    limit_price: Decimal


class Stop(msgspec.Struct, frozen=True, tag_field="type", tag="stop"):
    stop_price: Decimal


class TrailAmount(msgspec.Struct, frozen=True, tag_field="by", tag="amount"):
    amount: Decimal


class TrailPercent(msgspec.Struct, frozen=True, tag_field="by", tag="percent"):
    percent: Decimal


class TrailingStop(msgspec.Struct, frozen=True, tag_field="type", tag="trailing_stop"):
    trail: TrailAmount | TrailPercent
    stop_price: Decimal | None = None


class MarketOnOpen(msgspec.Struct, frozen=True, tag_field="type", tag="market_on_open"):
    pass


class MarketOnClose(msgspec.Struct, frozen=True, tag_field="type", tag="market_on_close"):
    pass


type OrderType = Market | Limit | Stop | TrailingStop | MarketOnOpen | MarketOnClose


class WorkingOrder(msgspec.Struct, frozen=True):
    order_id: int
    client_id: str
    symbol: str
    side: Literal["buy", "sell"]
    qty: int
    filled_qty: int
    order_type: OrderType


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

    @property
    def share_ratio(self) -> Decimal | None:
        match self.kind["type"]:
            case "split":
                return Decimal(self.kind["new_rate"]) / Decimal(self.kind["old_rate"])
            case "stock_dividend":
                return 1 + Decimal(self.kind["rate"])
            case _:
                return None


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
    unit: str
    form: str
    filed: date
    available_at: int


class HistoryResult(msgspec.Struct, frozen=True, tag_field="kind", tag="history"):
    bars: BarBuffer


class FundamentalsResult(msgspec.Struct, frozen=True, tag_field="kind", tag="fundamentals"):
    points: list[FundamentalFact]


class InstrumentProfile(msgspec.Struct, frozen=True):
    symbol: str
    name: str
    exchange: str
    asset_class: AssetClass
    sic: int | None
    sic_description: str | None


class InstrumentsResult(msgspec.Struct, frozen=True, tag_field="kind", tag="instruments"):
    instruments: list[InstrumentProfile]


class EarningsReleaseFrame(msgspec.Struct, frozen=True):
    symbol: str
    cik: int
    accession: str
    form: str
    filed: date
    report_date: date | None
    accepted_at: int


class EarningsResult(msgspec.Struct, frozen=True, tag_field="kind", tag="earnings"):
    releases: list[EarningsReleaseFrame]


class FilingFrame(msgspec.Struct, frozen=True):
    symbol: str
    cik: int
    accession: str
    form: str
    filed: date
    report_date: date | None
    items: list[str]
    accepted_at: int
    available_at: int


class FilingsResult(msgspec.Struct, frozen=True, tag_field="kind", tag="filings"):
    filings: list[FilingFrame]


class PressReleaseFrame(msgspec.Struct, frozen=True):
    symbol: str
    accession: str
    accepted_at: int
    available_at: int
    exhibit: str
    text: str


class PressReleasesResult(msgspec.Struct, frozen=True, tag_field="kind", tag="press_releases"):
    releases: list[PressReleaseFrame]


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


class InsiderOwner(msgspec.Struct, frozen=True):
    cik: int
    name: str
    director: bool
    officer: bool
    ten_percent_owner: bool
    other: bool
    officer_title: str | None


class InsiderTransactionFrame(msgspec.Struct, frozen=True):
    security: str
    date: date
    code: str
    acquired: bool
    shares: Decimal | None
    price: Decimal | None
    shares_after: Decimal | None
    direct: bool


class InsiderTradeFrame(msgspec.Struct, frozen=True):
    symbol: str
    accession: str
    form: str
    filed: date
    accepted_at: int
    available_at: int
    plan_10b5_1: bool | None
    owners: list[InsiderOwner]
    transaction: InsiderTransactionFrame


class InsiderTradesResult(msgspec.Struct, frozen=True, tag_field="kind", tag="insider_trades"):
    trades: list[InsiderTradeFrame]


class NewsArticleFrame(msgspec.Struct, frozen=True):
    id: int
    symbols: list[str]
    headline: str
    summary: str
    source: str
    created_at: int
    updated_at: int
    received_at: int
    backfilled: bool


class NewsResult(msgspec.Struct, frozen=True, tag_field="kind", tag="news"):
    articles: list[NewsArticleFrame]


class FomcFrame(msgspec.Struct, frozen=True):
    date: date
    scheduled: bool
    statement_at: int | None
    available_at: int


class FomcResult(msgspec.Struct, frozen=True, tag_field="kind", tag="fomc"):
    announcements: list[FomcFrame]


class ShortInterestFrame(msgspec.Struct, frozen=True):
    symbol: str
    settlement: date
    shares: int
    previous_shares: int
    average_daily_volume: int
    days_to_cover: Decimal
    split: bool
    revised: bool
    published: date
    available_at: int


class ShortInterestResult(msgspec.Struct, frozen=True, tag_field="kind", tag="short_interest"):
    positions: list[ShortInterestFrame]


class ShortVolumeFrame(msgspec.Struct, frozen=True):
    symbol: str
    date: date
    short_volume: Decimal
    short_exempt_volume: Decimal
    total_volume: Decimal
    available_at: int


class ShortVolumeResult(msgspec.Struct, frozen=True, tag_field="kind", tag="short_volume"):
    volumes: list[ShortVolumeFrame]


class BorrowFrame(msgspec.Struct, frozen=True):
    symbol: str
    at: int
    rebate_rate: Decimal | None
    fee_rate: Decimal | None
    available: int
    available_or_more: bool
    available_at: int


class BorrowResult(msgspec.Struct, frozen=True, tag_field="kind", tag="borrow"):
    snapshots: list[BorrowFrame]


type Result = (
    HistoryResult
    | FundamentalsResult
    | CalendarResult
    | UniverseResult
    | MacroResult
    | FomcResult
    | InstrumentsResult
    | EarningsResult
    | FilingsResult
    | PressReleasesResult
    | InsiderTradesResult
    | NewsResult
    | ShortInterestResult
    | ShortVolumeResult
    | BorrowResult
)


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


class InstrumentsRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="instruments"):
    symbols: list[str]


class EarningsRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="earnings"):
    symbols: list[str]


class FilingsRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="filings"):
    symbols: list[str]
    since: date
    items: list[str] | None


class PressReleasesRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="press_releases"):
    symbols: list[str]
    since: date
    accessions: list[str] | None


class InsiderTradesRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="insider_trades"):
    symbols: list[str]
    since: date


class NewsRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="news"):
    symbols: list[str]
    since: date


class ShortInterestRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="short_interest"):
    symbols: list[str]
    since: date


class ShortVolumeRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="short_volume"):
    symbols: list[str]
    since: date


class BorrowRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="borrow"):
    symbols: list[str]
    since: date


class CalendarRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="calendar"):
    start: date = msgspec.field(name="from")
    end: date = msgspec.field(name="to")


class UniverseRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="universe"):
    date: date
    universe: dict[str, str | int]


class MacroRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="macro"):
    series: list[MacroSeries]
    start: date = msgspec.field(name="from")
    end: date = msgspec.field(name="to")


class FomcRequest(msgspec.Struct, frozen=True, tag_field="kind", tag="fomc"):
    start: date = msgspec.field(name="from")
    end: date = msgspec.field(name="to")


type Request = (
    HistoryRequest
    | FundamentalsRequest
    | CalendarRequest
    | UniverseRequest
    | MacroRequest
    | FomcRequest
    | InstrumentsRequest
    | EarningsRequest
    | FilingsRequest
    | PressReleasesRequest
    | InsiderTradesRequest
    | NewsRequest
    | ShortInterestRequest
    | ShortVolumeRequest
    | BorrowRequest
)


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
    order_type: OrderType
    tif: TimeInForce
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
