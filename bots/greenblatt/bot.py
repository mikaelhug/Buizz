from collections import defaultdict
from datetime import date
from typing import NamedTuple

from buizz_bots.caps import market_caps
from buizz_bots.fundamentals import (
    DOMESTIC_FORMS,
    MAX_QUARTER_AGE_DAYS,
    Annual,
    amount_or_zero,
    by_symbol,
    current_debt,
    histories_since,
    in_dollars,
    latest_shares,
    quarterly,
    trailing_year,
)
from buizz_bots.ranks import highest_first, percentiles
from buizz_bots.value import NET_DEBT_METRICS, excess_cash, net_debt, operating_companies

from buizz import Bot, Context, FundamentalPoint, Metric, run

METRICS: list[Metric] = [
    "operating_income",
    "current_assets",
    "current_liabilities",
    "ppe_net",
    "shares_outstanding",
    *NET_DEBT_METRICS,
]
CHUNK = 200
LOOKBACK_DAYS = 730
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
    durations = [
        (point, float(point.value))
        for point in points
        if point.metric == "operating_income" and point.period_start is not None
    ]
    return trailing_year(quarterly(durations), today)


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
    current_assets = amount_or_zero(sheet, symbol, "current_assets", end) - excess_cash(sheet, symbol, end)
    current_liabilities = amount_or_zero(sheet, symbol, "current_liabilities", end) - current_debt(sheet, symbol, end)
    capital = current_assets - current_liabilities + amount_or_zero(sheet, symbol, "ppe_net", end)
    if capital <= 0:
        return None
    return Accounts(ebit, capital, net_debt(sheet, symbol, end), shares)


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


def lots_of(ctx: Context) -> list[Lot]:
    today = ctx.session.date
    return [
        Lot(symbol, (today - opened.date()).days, ctx.bars[symbol].close / float(position.avg_cost) - 1)
        for symbol, position in sorted(ctx.portfolio.positions.items())
        if position.qty > 0 and symbol in ctx.bars and (opened := position.opened_at) is not None
    ]


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
        sold: list[str] = []
        for lot in lots_of(ctx):
            reason = due(ctx, lot)
            if reason is not None:
                ctx.order_target_weight(lot.symbol, 0, reason=reason)
                sold.append(lot.symbol)
        held = {symbol for symbol, position in ctx.portfolio.positions.items() if position.qty > 0}
        slots = target - len(held - set(sold))
        if slots <= 0:
            return
        ranked, scores = magic_formula(ctx)
        chosen = [symbol for symbol in ranked if symbol not in held][:slots]
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


if __name__ == "__main__":
    run(Greenblatt)
