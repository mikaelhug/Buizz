import math
from datetime import date
from typing import NamedTuple

import numpy as np
import numpy.typing as npt
from buizz_bots.fundamentals import (
    Annual,
    adjusted_shares,
    amount,
    consecutive_fiscal_ends,
    domestic_annual,
    histories_since,
    traded_today,
)

from buizz import Bot, Context, Metric, run

RECORD_METRICS: list[Metric] = [
    "operating_income",
    "net_income",
    "total_assets",
    "current_assets",
    "current_liabilities",
    "ppe_net",
    "operating_cash_flow",
    "capex",
]
MARGIN_METRICS: list[Metric] = ["revenue", "gross_profit", "cost_of_revenue"]
PRICE_METRICS: list[Metric] = [
    "shares_outstanding",
    "cash_and_equivalents",
    "short_term_investments",
    "long_term_debt",
    "long_term_debt_noncurrent",
    "short_term_debt",
    "short_term_borrowings",
    "current_long_term_debt",
    "preferred_stock",
]
METRICS = RECORD_METRICS + MARGIN_METRICS + PRICE_METRICS
FINANCIALS = range(6000, 7000)
UTILITIES = range(4900, 4950)

Floats = npt.NDArray[np.float64]


class Year(NamedTuple):
    roa: float
    roc: float
    fcf: float
    gross_margin: float
    total_assets: float


class Franchise(NamedTuple):
    roa: float
    roc: float
    cfoa: float
    margin_growth: float
    margin_stability: float


class Assessment(NamedTuple):
    fiscal_end: date
    latest_roc: float | None
    franchise: Franchise | None
    ebit_tev: float | None


class Board(NamedTuple):
    assessments: dict[str, Assessment]
    power: dict[str, float]
    yields: Floats


def gross_margin(annual: Annual, symbol: str, end: date) -> float | None:
    revenue = amount(annual, symbol, "revenue", end)
    gross = amount(annual, symbol, "gross_profit", end)
    cost = amount(annual, symbol, "cost_of_revenue", end)
    if revenue is None or revenue <= 0:
        return None
    if gross is None and cost is not None:
        gross = revenue - cost
    return None if gross is None else gross / revenue


def year_of(annual: Annual, symbol: str, end: date) -> Year | None:
    found = [amount(annual, symbol, metric, end) for metric in RECORD_METRICS]
    margin = gross_margin(annual, symbol, end)
    values = [value for value in found if value is not None]
    if margin is None or len(values) < len(found):
        return None
    ebit, net_income, assets, current_assets, current_liabilities, ppe, operating_cash_flow, capex = values
    capital = ppe + current_assets - current_liabilities
    if capital <= 0 or assets <= 0:
        return None
    return Year(net_income / assets, ebit / capital, operating_cash_flow - capex, margin, assets)


def geometric_mean(returns: list[float]) -> float | None:
    if min(returns) <= -1:
        return None
    return math.expm1(float(np.mean(np.log1p(returns))))


def franchise_of(record: list[Year]) -> Franchise | None:
    margins = np.array([year.gross_margin for year in record])
    roa = geometric_mean([year.roa for year in record])
    roc = geometric_mean([year.roc for year in record])
    if roa is None or roc is None or margins.min() <= 0:
        return None
    growth = (margins[-1] / margins[0]) ** (1 / (len(margins) - 1)) - 1
    spread = float(margins.std())
    stability = math.inf if spread == 0 else float(margins.mean()) / spread
    cfoa = sum(year.fcf for year in record) / record[-1].total_assets
    return Franchise(roa, roc, cfoa, float(growth), stability)


def total_debt(annual: Annual, symbol: str, end: date) -> float:
    def part(metric: Metric) -> float:
        return amount(annual, symbol, metric, end) or 0.0

    current = amount(annual, symbol, "short_term_debt", end)
    if current is None:
        current = part("short_term_borrowings") + part("current_long_term_debt")
    noncurrent = amount(annual, symbol, "long_term_debt_noncurrent", end)
    if noncurrent is not None:
        return noncurrent + current
    reported_total = amount(annual, symbol, "long_term_debt", end)
    if reported_total is None:
        return current
    return reported_total + part("short_term_borrowings")


def enterprise_extras(annual: Annual, symbol: str, end: date) -> float:
    def part(metric: Metric) -> float:
        return amount(annual, symbol, metric, end) or 0.0

    cash = part("cash_and_equivalents") + part("short_term_investments")
    return total_debt(annual, symbol, end) + part("preferred_stock") - cash


def percentile(reference: Floats, value: float) -> float:
    return float(np.searchsorted(reference, value, side="right")) / len(reference)


def eligible(ctx: Context, symbols: list[str]) -> list[str]:
    profiles = ctx.instruments(symbols)
    return [
        symbol
        for symbol in symbols
        if symbol in profiles
        and (sic := profiles[symbol].sic) is not None
        and sic not in FINANCIALS
        and sic not in UTILITIES
    ]


def assess(ctx: Context, symbols: list[str]) -> dict[str, Assessment]:
    today = ctx.session.date
    years = int(ctx.params["record_years"])
    annual = domestic_annual(ctx.fundamentals(symbols, METRICS))
    found: dict[str, Assessment] = {}
    for symbol in symbols:
        reported = annual.get((symbol, "operating_income"), {})
        latest = consecutive_fiscal_ends(reported, today, 1)
        if latest is None:
            continue
        record = [
            year_of(annual, symbol, end) for end in reversed(consecutive_fiscal_ends(reported, today, years) or [])
        ]
        complete = [year for year in record if year is not None]
        franchise = franchise_of(complete) if len(complete) == years else None
        newest = year_of(annual, symbol, latest[0])
        found[symbol] = Assessment(latest[0], None if newest is None else newest.roc, franchise, None)
    shares = {
        symbol: point
        for symbol, seen in found.items()
        if (point := annual.get((symbol, "shares_outstanding"), {}).get(seen.fiscal_end)) is not None
        and point.value > 0
    }
    if not shares:
        return found
    raw, adjusted = histories_since(ctx, sorted(shares), min(point.filed for point in shares.values()))
    for symbol, point in shares.items():
        end = found[symbol].fiscal_end
        ebit = amount(annual, symbol, "operating_income", end)
        if symbol not in raw or ebit is None or not traded_today(ctx, raw[symbol]):
            continue
        count = adjusted_shares(point, raw[symbol], adjusted[symbol])
        if count is None:
            continue
        tev = count * float(raw[symbol].close[-1]) + enterprise_extras(annual, symbol, end)
        if tev > 0:
            found[symbol] = found[symbol]._replace(ebit_tev=ebit / tev)
    return found


def franchise_power(assessments: dict[str, Assessment], universe: set[str]) -> dict[str, float]:
    franchises = {symbol: seen.franchise for symbol, seen in assessments.items() if seen.franchise is not None}
    members = [franchise for symbol, franchise in franchises.items() if symbol in universe]
    if not members:
        return {}
    references = [np.sort(np.array(column)) for column in zip(*members, strict=True)]

    def score(franchise: Franchise) -> float:
        roa, roc, cfoa, growth, stability = (
            percentile(reference, value) for reference, value in zip(references, franchise, strict=True)
        )
        return (roa + roc + cfoa + max(growth, stability)) / 4

    scores = {symbol: score(franchise) for symbol, franchise in franchises.items()}
    power_reference = np.sort(np.array([scores[symbol] for symbol in scores if symbol in universe]))
    return {symbol: percentile(power_reference, value) for symbol, value in scores.items()}


def board_of(ctx: Context, held: list[str]) -> Board:
    universe = eligible(ctx, sorted(set(ctx.universe)))
    symbols = sorted(set(universe) | set(held))
    assessments = assess(ctx, symbols)
    members = set(universe)
    yields = np.sort(
        np.array(
            [seen.ebit_tev for symbol, seen in assessments.items() if symbol in members and seen.ebit_tev is not None]
        )
    )
    return Board(assessments, franchise_power(assessments, members), yields)


def latest_reports(ctx: Context, symbols: list[str]) -> dict[str, str]:
    annual = domestic_annual(ctx.fundamentals(symbols, ["operating_income"]))
    return {symbol: max(points).isoformat() for (symbol, _), points in annual.items() if points}


def sell_reason(ctx: Context, board: Board, symbol: str) -> str | None:
    seen = board.assessments.get(symbol)
    if seen is None:
        return None
    quality_floor = 1 - float(ctx.params["sell_quality_floor"])
    power = board.power.get(symbol)
    if seen.latest_roc is not None and seen.latest_roc < 0:
        return f"quality broke: return on capital {seen.latest_roc:.1%} in FY ending {seen.fiscal_end}"
    if power is not None and power < quality_floor:
        return f"quality broke: franchise power pct {power:.0%}, below the top {1 - quality_floor:.0%}"
    if seen.ebit_tev is not None and len(board.yields):
        bottom = float(ctx.params["sell_value_floor"])
        cut = float(np.quantile(board.yields, bottom))
        if seen.ebit_tev < cut:
            return (
                f"price absurd: EBIT/TEV {seen.ebit_tev:.1%}, in the universe's bottom {bottom:.0%} (below {cut:.1%})"
            )
    return None


def buy(ctx: Context, board: Board, held: list[str], kept: int) -> None:
    holdings = int(ctx.params["holdings"])
    if kept >= holdings or not len(board.yields):
        return
    floor = float(np.quantile(board.yields, float(ctx.params["value_floor"])))
    quality_floor = 1 - float(ctx.params["quality_top"])
    universe = set(ctx.universe)
    candidates = sorted(
        (
            (ebit_tev, symbol, power, franchise.roc)
            for symbol, power in board.power.items()
            if symbol in universe
            and symbol not in held
            and power > quality_floor
            and (franchise := board.assessments[symbol].franchise) is not None
            and (ebit_tev := board.assessments[symbol].ebit_tev) is not None
            and ebit_tev >= floor
        ),
        reverse=True,
    )
    years = int(ctx.params["record_years"])
    for ebit_tev, symbol, power, roc in candidates[: holdings - kept]:
        ctx.order_target_weight(
            symbol,
            1 / holdings,
            reason=(
                f"franchise power pct {power:.0%} (top {1 - quality_floor:.0%}), {years}y ROC {roc:.1%}, "
                f"EBIT/TEV {ebit_tev:.1%} at or above the universe median {floor:.1%}"
            ),
        )


class Munger(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        held = sorted(symbol for symbol, position in ctx.portfolio.positions.items() if position.qty > 0)
        known: dict[str, str] = ctx.state.get("reports", {})
        reports = latest_reports(ctx, held) if held else {}
        fresh = [symbol for symbol in held if symbol in known and reports.get(symbol, known[symbol]) != known[symbol]]
        ctx.state["reports"] = {symbol: reports.get(symbol, known.get(symbol, "")) for symbol in held}
        rerank = today.month == int(ctx.params["rerank_month"]) and ctx.state.get("ranked") != today.year
        if not fresh and not rerank:
            return
        board = board_of(ctx, held)
        kept = list(held)
        for symbol in fresh:
            reason = sell_reason(ctx, board, symbol)
            if reason is not None:
                ctx.order_target_weight(symbol, 0, reason=reason)
                kept.remove(symbol)
        if rerank:
            ctx.state["ranked"] = today.year
            buy(ctx, board, held, len(kept))


if __name__ == "__main__":
    run(Munger)
