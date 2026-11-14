from collections import defaultdict
from datetime import date
from itertools import pairwise
from typing import NamedTuple

from buizz_bots.caps import market_caps
from buizz_bots.fundamentals import (
    DOMESTIC_FORMS,
    Annual,
    amount_or_zero,
    by_symbol,
    current_debt,
    histories_since,
    in_dollars,
    latest_shares,
    total_debt,
    with_fourth_quarters,
)
from buizz_bots.ranks import highest_first, percentiles

from buizz import Bot, Context, FundamentalPoint, Metric, run

METRICS: list[Metric] = [
    "operating_income",
    "current_assets",
    "current_liabilities",
    "cash_and_equivalents",
    "short_term_investments",
    "ppe_net",
    "short_term_debt",
    "short_term_borrowings",
    "current_long_term_debt",
    "long_term_debt",
    "long_term_debt_noncurrent",
    "preferred_stock",
    "shares_outstanding",
]
FINANCIALS = range(6000, 7000)
UTILITIES = range(4900, 4950)
CHUNK = 200
LOOKBACK_DAYS = 730
MAX_QUARTER_AGE_DAYS = 200
QUARTER_GAP_DAYS = range(80, 101)
QUARTERS_PER_YEAR = 4
MONTHS_PER_YEAR = 12


class Accounts(NamedTuple):
    ebit: float
    capital: float
    net_debt: float
    shares: FundamentalPoint


class Score(NamedTuple):
    roc: float
    earnings_yield: float
    market_cap: float


class Lot(NamedTuple):
    symbol: str
    age: int
    gain: float


def trailing_ebit(points: list[FundamentalPoint], today: date) -> float | None:
    durations = [point for point in points if point.metric == "operating_income" and point.period_start is not None]
    quarters = with_fourth_quarters(
        {point.period_end: float(point.value) for point in durations if point.period_type == "Q"},
        [
            (point.period_start, point.period_end, float(point.value))
            for point in durations
            if point.period_type == "FY" and point.period_start is not None
        ],
    )
    ends = sorted(quarters, reverse=True)[:QUARTERS_PER_YEAR]
    if len(ends) < QUARTERS_PER_YEAR or (today - ends[0]).days > MAX_QUARTER_AGE_DAYS:
        return None
    if any((later - earlier).days not in QUARTER_GAP_DAYS for later, earlier in pairwise(ends)):
        return None
    return sum(quarters[end] for end in ends)


def balance_sheet(points: list[FundamentalPoint]) -> Annual:
    sheet: Annual = defaultdict(dict)
    for point in points:
        if point.period_start is None:
            sheet[point.symbol, point.metric][point.period_end] = point
    return sheet


def accounts_of(symbol: str, points: list[FundamentalPoint], today: date) -> Accounts | None:
    ebit = trailing_ebit(points, today)
    shares = latest_shares(points, today)
    sheet = balance_sheet(points)
    dated = set(sheet.get((symbol, "current_assets"), {})) & set(sheet.get((symbol, "current_liabilities"), {}))
    end = max(dated & set(sheet.get((symbol, "ppe_net"), {})), default=None)
    if ebit is None or shares is None or end is None or (today - end).days > MAX_QUARTER_AGE_DAYS:
        return None
    cash = amount_or_zero(sheet, symbol, "cash_and_equivalents", end) + amount_or_zero(
        sheet, symbol, "short_term_investments", end
    )
    current_assets = amount_or_zero(sheet, symbol, "current_assets", end) - cash
    current_liabilities = amount_or_zero(sheet, symbol, "current_liabilities", end) - current_debt(sheet, symbol, end)
    capital = current_assets - current_liabilities + amount_or_zero(sheet, symbol, "ppe_net", end)
    if capital <= 0:
        return None
    net_debt = total_debt(sheet, symbol, end) + amount_or_zero(sheet, symbol, "preferred_stock", end) - cash
    return Accounts(ebit, capital, net_debt, shares)


def operating_companies(ctx: Context, symbols: list[str]) -> list[str]:
    profiles = ctx.instruments(symbols)
    return [
        symbol
        for symbol in symbols
        if symbol in profiles
        and (sic := profiles[symbol].sic) is not None
        and sic not in FINANCIALS
        and sic not in UTILITIES
    ]


def scores_of(ctx: Context, symbols: list[str]) -> dict[str, Score]:
    today = ctx.session.date
    points = by_symbol(
        point
        for point in sorted(ctx.fundamentals(symbols, METRICS), key=lambda point: point.filed)
        if point.form in DOMESTIC_FORMS and in_dollars(point) and (today - point.period_end).days <= LOOKBACK_DAYS
    )
    found = {symbol: seen for symbol in symbols if (seen := accounts_of(symbol, points[symbol], today)) is not None}
    if not found:
        return {}
    raw, adjusted = histories_since(ctx, sorted(found), min(seen.shares.filed for seen in found.values()))
    floor = float(ctx.params["min_market_cap"])
    caps = market_caps(ctx, {symbol: seen.shares for symbol, seen in found.items()}, raw, adjusted)
    return {
        symbol: Score(found[symbol].ebit / found[symbol].capital, found[symbol].ebit / enterprise_value, cap)
        for symbol, cap in caps.items()
        if cap >= floor and (enterprise_value := cap + found[symbol].net_debt) > 0
    }


def magic_formula(ctx: Context) -> tuple[list[str], dict[str, Score]]:
    universe = sorted(ctx.universe)
    scores: dict[str, Score] = {}
    for offset in range(0, len(universe), CHUNK):
        scores.update(scores_of(ctx, operating_companies(ctx, universe[offset : offset + CHUNK])))
    if not scores:
        return [], {}
    roc = percentiles({symbol: score.roc for symbol, score in scores.items()})
    earnings_yield = percentiles({symbol: score.earnings_yield for symbol, score in scores.items()})
    return highest_first({symbol: roc[symbol] + earnings_yield[symbol] for symbol in scores}), scores


def lots_of(ctx: Context, lots: dict[str, str]) -> list[Lot]:
    today = ctx.session.date
    held: list[Lot] = []
    for symbol, bought in lots.items():
        position = ctx.position(symbol)
        if symbol in ctx.bars and position.qty > 0:
            gain = ctx.bars[symbol].close / float(position.avg_cost) - 1
            held.append(Lot(symbol, (today - date.fromisoformat(bought)).days, gain))
    return held


def due(ctx: Context, lot: Lot) -> str | None:
    if lot.gain < 0 and lot.age >= int(ctx.params["loser_days"]):
        return f"held {lot.age} days at {lot.gain:.1%}: a loss, sold just before one year"
    if lot.age >= int(ctx.params["winner_days"]):
        return f"held {lot.age} days at {lot.gain:+.1%}: sold just after one year"
    return None


class Greenblatt(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        month = today.year * MONTHS_PER_YEAR + today.month - 1
        tranche = int(ctx.params["tranche_size"])
        holdings = tranche * MONTHS_PER_YEAR // int(ctx.params["tranche_months"])
        target = int(ctx.state.get("target", 0))
        if target < holdings and month >= ctx.state.get("next_tranche", month):
            target = min(holdings, target + tranche)
            ctx.state["target"] = target
            ctx.state["next_tranche"] = month + int(ctx.params["tranche_months"])
        lots: dict[str, str] = ctx.state.get("lots", {})
        sold: list[str] = []
        for lot in lots_of(ctx, lots):
            reason = due(ctx, lot)
            if reason is not None:
                ctx.order_target_weight(lot.symbol, 0, reason=reason)
                sold.append(lot.symbol)
        gone = sorted(lots[symbol] for symbol in lots if symbol not in sold and ctx.position(symbol).qty <= 0)
        lots = {
            symbol: bought for symbol, bought in lots.items() if symbol not in sold and ctx.position(symbol).qty > 0
        }
        renamed = sorted(
            symbol
            for symbol, position in ctx.portfolio.positions.items()
            if position.qty > 0 and symbol not in lots and symbol not in sold
        )
        lots |= {
            symbol: gone[index] if index < len(gone) else today.isoformat() for index, symbol in enumerate(renamed)
        }
        ctx.state["lots"] = lots
        slots = target - len(lots)
        if slots <= 0:
            return
        ranked, scores = magic_formula(ctx)
        chosen = [symbol for symbol in ranked if symbol not in lots and symbol not in sold][:slots]
        proceeds = sum(ctx.position(symbol).qty * ctx.bars[symbol].close for symbol in sold)
        free = (float(ctx.portfolio.cash) + proceeds) / float(ctx.portfolio.equity)
        if not chosen or free <= 0:
            return
        weight = min(1 / holdings, free / len(chosen))
        for symbol in chosen:
            score = scores[symbol]
            ctx.order_target_weight(
                symbol,
                weight,
                reason=(
                    f"magic formula rank {ranked.index(symbol) + 1} of {len(ranked)}: return on capital "
                    f"{score.roc:.0%}, EBIT/EV {score.earnings_yield:.1%}, market cap ${score.market_cap / 1e9:.1f}B"
                ),
            )
            lots[symbol] = today.isoformat()


if __name__ == "__main__":
    run(Greenblatt)
