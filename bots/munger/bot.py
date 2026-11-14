import math
from datetime import date, timedelta
from typing import NamedTuple

import numpy as np
import numpy.typing as npt
from buizz_bots.fundamentals import (
    Annual,
    adjusted_shares,
    amount,
    consecutive_fiscal_ends,
    domestic_annual,
    gross_margin,
    histories_since,
    traded_today,
    year_end_shares,
)
from buizz_bots.persona import (
    DAYS_PER_YEAR,
    Buy,
    Ladder,
    Sell,
    Temperature,
    candidates,
    ladder_weight,
    new_filings,
    restated,
    stock_weights,
    temperature,
    trade,
)
from buizz_bots.value import EBIT_METRICS, NET_DEBT_METRICS, ebit, net_debt, operating_companies

from buizz import Bot, Context, Metric, run

RECORD_METRICS: list[Metric] = [
    "net_income",
    "total_assets",
    "current_assets",
    "current_liabilities",
    "ppe_net",
    "operating_cash_flow",
    "capex",
]
MARGIN_METRICS: list[Metric] = ["revenue", "gross_profit", "cost_of_revenue"]
METRICS = EBIT_METRICS + RECORD_METRICS + MARGIN_METRICS + ["shares_outstanding", *NET_DEBT_METRICS]
FILING_METRIC: Metric = "total_assets"
IDLE_CASH = 0.01

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


class Mood(NamedTuple):
    panic: bool
    hot: bool
    why: str


def year_of(annual: Annual, symbol: str, end: date) -> Year | None:
    found = [amount(annual, symbol, metric, end) for metric in RECORD_METRICS]
    earnings = ebit(annual, symbol, end)
    margin = gross_margin(annual, symbol, end)
    values = [value for value in found if value is not None]
    if earnings is None or margin is None or len(values) < len(found):
        return None
    (net_income, assets, current_assets, current_liabilities, ppe, operating_cash_flow, capex) = values
    capital = ppe + current_assets - current_liabilities
    if capital <= 0 or assets <= 0:
        return None
    return Year(net_income / assets, earnings / capital, operating_cash_flow - capex, margin, assets)


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


def percentile(reference: Floats, value: float) -> float:
    return float(np.searchsorted(reference, value, side="right")) / len(reference)


def assess(ctx: Context, symbols: list[str]) -> dict[str, Assessment]:
    today = ctx.session.date
    years = int(ctx.params["record_years"])
    annual = domestic_annual(ctx.fundamentals(symbols, METRICS))
    found: dict[str, Assessment] = {}
    for symbol in symbols:
        reported = set(annual.get((symbol, "operating_income"), {})) | set(annual.get((symbol, "pretax_income"), {}))
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
        if (point := year_end_shares(annual, symbol, seen.fiscal_end)) is not None and point.value > 0
    }
    if not shares:
        return found
    raw, adjusted = histories_since(ctx, sorted(shares), min(point.filed for point in shares.values()))
    for symbol, point in shares.items():
        end = found[symbol].fiscal_end
        earnings = ebit(annual, symbol, end)
        if symbol not in raw or earnings is None or not traded_today(ctx, raw[symbol]):
            continue
        count = adjusted_shares(point, raw[symbol], adjusted[symbol])
        if count is None:
            continue
        tev = count * float(raw[symbol].close[-1]) + net_debt(annual, symbol, end)
        if tev > 0:
            found[symbol] = found[symbol]._replace(ebit_tev=earnings / tev)
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
    universe = operating_companies(ctx, sorted(set(ctx.universe)))
    symbols = sorted(set(universe) | set(held))
    assessments = assess(ctx, symbols)
    members = set(universe)
    yields = np.sort(
        np.array(
            [seen.ebit_tev for symbol, seen in assessments.items() if symbol in members and seen.ebit_tev is not None]
        )
    )
    return Board(assessments, franchise_power(assessments, members), yields)


def thesis_break(ctx: Context, board: Board, symbol: str) -> str | None:
    seen = board.assessments.get(symbol)
    if seen is None:
        return None
    quality_floor = 1 - float(ctx.params["sell_quality_floor"])
    power = board.power.get(symbol)
    if seen.latest_roc is not None and seen.latest_roc < 0:
        return f"return on capital {seen.latest_roc:.1%} in FY ending {seen.fiscal_end}"
    if power is not None and power < quality_floor:
        return f"franchise power pct {power:.0%}, below the top {1 - quality_floor:.0%}"
    return None


def absurd_price(ctx: Context, board: Board, symbol: str) -> str | None:
    seen = board.assessments.get(symbol)
    if seen is None or seen.ebit_tev is None or not len(board.yields):
        return None
    bottom = float(ctx.params["sell_value_floor"])
    cut = float(np.quantile(board.yields, bottom))
    if seen.ebit_tev >= cut:
        return None
    return f"EBIT/TEV {seen.ebit_tev:.1%}, in the universe's bottom {bottom:.0%} (below {cut:.1%})"


def sell_of(ctx: Context, board: Board, symbol: str) -> Sell | None:
    broken = thesis_break(ctx, board, symbol)
    if broken is not None:
        return Sell(symbol, "thesis", broken)
    absurd = absurd_price(ctx, board, symbol)
    return None if absurd is None else Sell(symbol, "valuation", absurd)


def mood_of(ctx: Context, heat: Temperature | None) -> Mood:
    if heat is None:
        return Mood(False, False, "market temperature unknown")
    fall = heat.drawdown or 0.0
    panic = fall >= float(ctx.params["panic_drawdown"]) or heat.heat <= float(ctx.params["cold"])
    hot = not panic and heat.heat >= float(ctx.params["hot"])
    return Mood(panic, hot, f"{heat.why}, S&P 500 {fall:.0%} below its high")


def ladder_of(ctx: Context) -> Ladder:
    return Ladder(*(float(ctx.params[name]) for name in Ladder._fields))


def adds(ctx: Context, board: Board, ladder: Ladder, weights: dict[str, float]) -> list[Buy]:
    found: list[Buy] = []
    for symbol, weight in sorted(weights.items()):
        target = ladder_weight(ctx, ladder, symbol, weight)
        power = board.power.get(symbol)
        if target is None or power is None or thesis_break(ctx, board, symbol) is not None:
            continue
        fall = 1 - ctx.bars[symbol].close / float(ctx.position(symbol).avg_cost)
        found.append(
            Buy(
                symbol,
                target,
                f"add on weakness: {fall:.0%} below average cost while franchise power pct is still {power:.0%}, "
                f"{weight:.1%} to {target:.1%} of equity",
            )
        )
    return found


def opens(ctx: Context, board: Board, mood: Mood, ladder: Ladder, slots: int, skip: set[str]) -> list[Buy]:
    if slots <= 0 or not len(board.yields):
        return []
    level = float(ctx.params["panic_value_floor" if mood.panic else "value_floor"])
    floor = float(np.quantile(board.yields, level))
    quality_floor = 1 - float(ctx.params["quality_top"])
    universe = set(ctx.universe)

    def gate(symbol: str) -> float | None:
        seen = board.assessments[symbol]
        if symbol not in universe or symbol in skip or board.power[symbol] <= quality_floor:
            return None
        if seen.franchise is None or seen.ebit_tev is None or seen.ebit_tev < floor:
            return None
        return seen.ebit_tev

    passed = candidates(board.power, gate)
    since = ctx.session.date - timedelta(days=round(float(ctx.params["restatement_years"]) * DAYS_PER_YEAR))
    unreliable = restated(ctx, [symbol for symbol, _ in passed], since)
    years = int(ctx.params["record_years"])
    price = "panic price" if mood.panic else "fair price"
    return [
        Buy(
            symbol,
            ladder.start,
            f"{price}: franchise power pct {board.power[symbol]:.0%} (top {1 - quality_floor:.0%}), "
            f"{years}y ROC {franchise.roc:.1%}, EBIT/TEV {ebit_tev:.1%} at or above the universe's "
            f"{level * 100:.0f}th percentile {floor:.1%}; {mood.why}",
        )
        for symbol, ebit_tev in [pair for pair in passed if pair[0] not in unreliable][:slots]
        if (franchise := board.assessments[symbol].franchise) is not None
    ]


class Munger(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        cash_symbol = str(ctx.params["cash_symbol"])
        held = sorted(
            symbol for symbol, position in ctx.portfolio.positions.items() if position.qty > 0 and symbol != cash_symbol
        )
        renamed = {ctx.renames.get(symbol, symbol): filed for symbol, filed in ctx.state.get("filings", {}).items()}
        seen = {symbol: renamed[symbol] for symbol in held if symbol in renamed}
        fresh = new_filings(ctx, held, seen, FILING_METRIC)
        broken = restated(ctx, held, date.fromisoformat(ctx.state.get("checked", today.isoformat())))
        ctx.state["checked"] = today.isoformat()
        sells = [
            Sell(symbol, "thesis", "8-K Item 4.02, past financial statements no longer to be relied on")
            for symbol in sorted(broken)
        ]
        board = board_of(ctx, held) if fresh else None
        if board is not None:
            sells += [sell for symbol in fresh if symbol not in broken and (sell := sell_of(ctx, board, symbol))]
        sold = {sell.symbol for sell in sells}
        weights = {symbol: weight for symbol, weight in stock_weights(ctx, cash_symbol).items() if symbol not in sold}
        buys = self.buys(ctx, board, weights, sold)
        if sells or buys or float(ctx.portfolio.cash) > IDLE_CASH * float(ctx.portfolio.equity):
            trade(ctx, sells, buys, cash_symbol)
        new_filings(ctx, [buy.symbol for buy in buys if buy.symbol not in seen], seen, FILING_METRIC)
        ctx.state["filings"] = seen

    def buys(self, ctx: Context, board: Board | None, weights: dict[str, float], sold: set[str]) -> list[Buy]:
        ladder = ladder_of(ctx)
        slots = int(ctx.params["max_names"]) - len(weights)
        opening = slots > 0 and 1 - sum(weights.values()) >= ladder.start
        adding = any(ladder_weight(ctx, ladder, symbol, weight) is not None for symbol, weight in weights.items())
        if not opening and not adding:
            return []
        mood = mood_of(ctx, temperature(ctx))
        month = f"{ctx.session.date:%Y-%m}"
        if not mood.panic and ctx.state.get("scanned") == month:
            return []
        ctx.state["scanned"] = month
        if mood.hot and not adding:
            return []
        board = board or board_of(ctx, sorted(weights))
        found = adds(ctx, board, ladder, weights)
        if opening and not mood.hot:
            found += opens(ctx, board, mood, ladder, slots, set(weights) | sold)
        return found


if __name__ == "__main__":
    run(Munger)
