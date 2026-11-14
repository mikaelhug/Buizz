from collections.abc import Callable, Iterable
from datetime import date, timedelta
from typing import Literal, NamedTuple

import numpy as np

from buizz import Context, MacroSeries, Metric
from buizz_bots.fundamentals import DOMESTIC_FORMS, latest_filing

Trigger = Literal["thesis", "valuation", "swap", "timer", "price"]
TRIGGER_LABELS: dict[Trigger, str] = {
    "thesis": "thesis broke",
    "valuation": "valuation",
    "swap": "better use",
    "timer": "timer",
    "price": "price",
}
RESTATEMENT_ITEM = "4.02"
TEMPERATURE_YEARS = 10
YEAR_SESSIONS = 252
DAYS_PER_YEAR = 365.25
CASH_BAND = 0.05


class Ladder(NamedTuple):
    start: float
    add_step: float
    add_on_drop: float
    cap: float


class Sell(NamedTuple):
    symbol: str
    trigger: Trigger
    detail: str
    weight: float = 0.0

    @property
    def reason(self) -> str:
        return f"{TRIGGER_LABELS[self.trigger]}: {self.detail}"


class Buy(NamedTuple):
    symbol: str
    weight: float
    reason: str


class Temperature(NamedTuple):
    heat: float
    parts: dict[str, float]
    drawdown: float | None

    @property
    def why(self) -> str:
        shown = ", ".join(f"{name} {value:.0%}" for name, value in self.parts.items())
        return f"market temperature {self.heat:.0%} ({shown})"


def new_filings(ctx: Context, symbols: Iterable[str], seen: dict[str, str], metric: Metric) -> list[str]:
    wanted = sorted(set(symbols))
    if not wanted:
        return []
    points = ctx.fundamentals(wanted, [metric])
    fresh: list[str] = []
    for symbol in wanted:
        filed = latest_filing((point for point in points if point.symbol == symbol), DOMESTIC_FORMS)
        if filed is not None and seen.get(symbol) != filed.isoformat():
            seen[symbol] = filed.isoformat()
            fresh.append(symbol)
    return fresh


def restated(ctx: Context, symbols: Iterable[str], since: date) -> set[str]:
    wanted = sorted(set(symbols))
    if not wanted:
        return set()
    return {filing.symbol for filing in ctx.filings(wanted, since, [RESTATEMENT_ITEM])}


def candidates(symbols: Iterable[str], gate: Callable[[str], float | None]) -> list[tuple[str, float]]:
    passed = [(symbol, score) for symbol in symbols if (score := gate(symbol)) is not None]
    return sorted(passed, key=lambda pair: (-pair[1], pair[0]))


def held_stocks(ctx: Context, cash_symbol: str) -> list[str]:
    return sorted(
        symbol for symbol, position in ctx.portfolio.positions.items() if position.qty > 0 and symbol != cash_symbol
    )


def latest_macro(ctx: Context, series: MacroSeries, lookback_days: int) -> float | None:
    today = ctx.session.date
    points = ctx.macro([series], today - timedelta(days=lookback_days), today)
    return float(max(points, key=lambda point: point.date).value) if points else None


def stock_weights(ctx: Context, cash_symbol: str) -> dict[str, float]:
    equity = float(ctx.portfolio.equity)
    return {
        symbol: position.qty * ctx.bars[symbol].close / equity
        for symbol, position in ctx.portfolio.positions.items()
        if position.qty > 0 and symbol != cash_symbol and symbol in ctx.bars and equity > 0
    }


def ladder_weight(ctx: Context, ladder: Ladder, symbol: str, weight: float) -> float | None:
    if weight <= 0:
        return ladder.start
    position = ctx.position(symbol)
    bar = ctx.bars.get(symbol)
    if bar is None or position.qty <= 0 or weight + ladder.add_step > ladder.cap:
        return None
    if bar.close > float(position.avg_cost) * (1 - ladder.add_on_drop):
        return None
    return weight + ladder.add_step


def trade(ctx: Context, sells: Iterable[Sell], buys: Iterable[Buy], cash_symbol: str, share: float = 1.0) -> None:
    weights = stock_weights(ctx, cash_symbol)
    sold = False
    for sell in sells:
        if ctx.position(sell.symbol).qty <= 0:
            continue
        sold = True
        if sell.weight > 0:
            weights[sell.symbol] = sell.weight
        else:
            weights.pop(sell.symbol, None)
        ctx.order_target_weight(sell.symbol, sell.weight, reason=sell.reason)
    free = share - sum(weights.values())
    chosen: list[Buy] = []
    for buy in buys:
        added = buy.weight - weights.get(buy.symbol, 0)
        if 0 < added <= free:
            free -= added
            weights[buy.symbol] = buy.weight
            chosen.append(buy)
    cash = ctx.position(cash_symbol)
    bar = ctx.bars.get(cash_symbol)
    held = 0 if bar is None else cash.qty * bar.close / float(ctx.portfolio.equity)
    rest = max(1 - sum(weights.values()), 0)
    if not sold and not chosen and abs(rest - held) < CASH_BAND:
        return
    park = f"cash residual: {rest:.1%} of equity not used by {len(weights)} holdings"
    if rest < held:
        ctx.order_target_weight(cash_symbol, rest, reason=park)
    for buy in chosen:
        ctx.order_target_weight(buy.symbol, buy.weight, reason=buy.reason)
    if rest > held:
        ctx.order_target_weight(cash_symbol, rest, reason=park)


def rank_of(history: list[float], latest: float) -> float:
    return sum(value <= latest for value in history) / len(history)


def temperature(ctx: Context, market: str = "SPY", years: int = TEMPERATURE_YEARS) -> Temperature | None:
    today = ctx.session.date
    since = today - timedelta(days=round(years * DAYS_PER_YEAR))
    series: dict[str, dict[date, float]] = {}
    for point in ctx.macro(["sp500_forward_pe", "sp500_forward_pe_10y_avg", "baa10y", "vix"], since, today):
        series.setdefault(point.series, {})[point.date] = float(point.value)
    parts: dict[str, float] = {}
    drawdown: float | None = None
    pe, average = series.get("sp500_forward_pe", {}), series.get("sp500_forward_pe_10y_avg", {})
    ratios = [pe[day] / average[day] for day in sorted(pe.keys() & average.keys()) if average[day] > 0]
    if ratios:
        parts["valuation"] = rank_of(ratios, ratios[-1])
    for name, fear in (("baa10y", "credit"), ("vix", "volatility")):
        if found := series.get(name):
            values = [found[day] for day in sorted(found)]
            parts[fear] = 1 - rank_of(values, values[-1])
    closes = ctx.history([market], "1d", years * YEAR_SESSIONS).get(market)
    if closes is not None and len(closes.close) > YEAR_SESSIONS:
        prices = np.asarray(closes.close, dtype=np.float64)
        drawdowns = 1 - prices / np.maximum.accumulate(prices)
        drawdown = float(drawdowns[-1])
        parts["drawdown"] = 1 - rank_of(drawdowns.tolist(), drawdown)
    if not parts:
        return None
    return Temperature(float(np.mean(list(parts.values()))), parts, drawdown)


def stock_share(heat: Temperature, low: float, high: float) -> float:
    return high - (high - low) * heat.heat
