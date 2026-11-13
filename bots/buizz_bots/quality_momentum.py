from datetime import date, timedelta

import numpy as np

from buizz import Context, FundamentalPoint, History, Metric, split_ratio
from buizz_bots.caps import MIN_HISTORY_DAYS, capped_weights, market_caps, missing_caps, parked
from buizz_bots.drift import Book
from buizz_bots.fundamentals import (
    DOMESTIC_ANNUAL_FORMS,
    DOMESTIC_FORMS,
    QUARTER_TOLERANCE_DAYS,
    TWO_FISCAL_YEARS_DAYS,
    Annual,
    PointCache,
    amount,
    by_symbol,
    consecutive_fiscal_ends,
    domestic_annual,
    gross_profit,
    histories_since,
    latest_filing,
    latest_shares,
    recent,
    refreshed,
)
from buizz_bots.monthly import momentum, momentum_sessions
from buizz_bots.ranks import highest_first, rank_z
from buizz_bots.sessions import REBALANCED, first_step_or_month_end

ANNUAL_METRICS: list[Metric] = ["gross_profit", "revenue", "cost_of_revenue", "total_assets"]
FINANCIALS = range(6000, 6800)
YEAR_DAYS = 365
SUE_MIN_QUARTERS = 6
SUE_MAX_AGE_DAYS = 240
SUE_WINDOW_DAYS = 2 * YEAR_DAYS - QUARTER_TOLERANCE_DAYS
EPS_KEEP_DAYS = SUE_MAX_AGE_DAYS + 3 * YEAR_DAYS + 2 * QUARTER_TOLERANCE_DAYS
COMPONENTS = ("profitability", "momentum", "earnings")


def profitability(annual: Annual, symbol: str, today: date) -> float | None:
    latest = consecutive_fiscal_ends(annual.get((symbol, "total_assets"), {}), today, 1)
    if latest is None:
        return None
    assets = amount(annual, symbol, "total_assets", latest[0])
    profit = gross_profit(annual, symbol, latest[0])
    return None if assets is None or assets <= 0 or profit is None else profit / assets


def quarterly_eps(points: list[FundamentalPoint], raw: History, split: History) -> dict[date, float]:
    found: dict[date, float] = {}
    for point in points:
        if point.period_start is None:
            continue
        ratio = split_ratio(raw, split, point.filed)
        if ratio is not None and ratio > 0:
            found[point.period_end] = float(point.value) / ratio
    return found


def innovations(eps: dict[date, float]) -> dict[date, float]:
    found: dict[date, float] = {}
    for end, value in eps.items():
        prior = [earlier for earlier in eps if abs((end - earlier).days - YEAR_DAYS) <= QUARTER_TOLERANCE_DAYS]
        if prior:
            found[end] = value - eps[min(prior, key=lambda earlier: abs((end - earlier).days - YEAR_DAYS))]
    return found


def standardized_surprise(eps: dict[date, float], today: date) -> float | None:
    surprises = innovations(eps)
    if not surprises:
        return None
    latest = max(surprises)
    if (today - latest).days > SUE_MAX_AGE_DAYS:
        return None
    window = [value for end, value in surprises.items() if (latest - end).days < SUE_WINDOW_DAYS]
    if len(window) < SUE_MIN_QUARTERS:
        return None
    spread = float(np.std(window, ddof=1))
    return None if spread == 0 else surprises[latest] / spread


def book_weights(ctx: Context, chosen: list[str], caps: dict[str, float]) -> dict[str, float]:
    if ctx.params["weighting"] == "equal":
        return dict.fromkeys(chosen, 1 / len(chosen))
    total = sum(caps.values())
    multiple = float(ctx.params["index_multiple"])
    weights = capped_weights(
        {symbol: caps[symbol] for symbol in chosen},
        {symbol: multiple * caps[symbol] / total for symbol in chosen},
    )
    return parked(weights, ctx.params["cash"])


class QualityMomentum:
    def __init__(self) -> None:
        self.annual: PointCache = {}
        self.quarterly: PointCache = {}

    def due(self, ctx: Context) -> bool:
        return first_step_or_month_end(ctx, range(1, 13))

    def book(self, ctx: Context) -> Book | None:
        today = ctx.session.date
        ctx.state[REBALANCED] = today.isoformat()
        members = sorted(ctx.universe)
        profiles = ctx.instruments(members)
        eligible = [
            symbol
            for symbol in members
            if symbol not in profiles or (sic := profiles[symbol].sic) is None or sic not in FINANCIALS
        ]
        own = by_symbol(ctx.fundamentals(members, ["shares_outstanding"]))
        counts = {symbol: count for symbol in members if (count := latest_shares(own[symbol], today)) is not None}
        self.annual = {symbol: self.annual[symbol] for symbol in eligible if symbol in self.annual}
        self.quarterly = {symbol: self.quarterly[symbol] for symbol in eligible if symbol in self.quarterly}
        annual_points = refreshed(
            ctx,
            ANNUAL_METRICS,
            {symbol: latest_filing(own[symbol], DOMESTIC_ANNUAL_FORMS) for symbol in eligible},
            self.annual,
            recent(today, TWO_FISCAL_YEARS_DAYS, "FY", DOMESTIC_ANNUAL_FORMS),
        )
        eps_points = refreshed(
            ctx,
            ["eps_diluted"],
            {symbol: latest_filing(own[symbol], DOMESTIC_FORMS) for symbol in eligible},
            self.quarterly,
            recent(today, EPS_KEEP_DAYS, "Q", DOMESTIC_FORMS),
        )
        filed = [count.filed for count in counts.values()]
        filed += [point.filed for symbol in eligible if symbol in counts for point in eps_points[symbol]]
        raw, split = histories_since(ctx, sorted(counts), min([*filed, today - timedelta(days=MIN_HISTORY_DAYS)]))
        caps = market_caps(ctx, counts, raw, split)
        candidates = sorted(symbol for symbol in eligible if symbol in caps)
        if not candidates:
            ctx.log(f"no non-financial S&P 500 member with a share count and a close on {today}", level="warn")
            return None
        annual = domestic_annual([point for symbol in candidates for point in annual_points[symbol]])
        total_return = ctx.history(candidates, "1d", momentum_sessions(ctx), "all")
        raw_measures: dict[str, dict[str, float]] = {name: {} for name in COMPONENTS}
        for symbol in candidates:
            found = {
                "profitability": profitability(annual, symbol, today),
                "momentum": momentum(total_return[symbol], today),
                "earnings": standardized_surprise(quarterly_eps(eps_points[symbol], raw[symbol], split[symbol]), today),
            }
            for name, value in found.items():
                if value is not None:
                    raw_measures[name][symbol] = value
        z = {name: rank_z(values) for name, values in raw_measures.items()}
        composite = {
            symbol: sum(z[name][symbol] for name in COMPONENTS) / len(COMPONENTS)
            for symbol in candidates
            if all(symbol in z[name] for name in COMPONENTS)
        }
        ranked = highest_first(composite)
        chosen = ranked[: int(ctx.params["holdings"])]
        if not chosen:
            ctx.log(f"no member with all of gross profitability, 12-1 momentum and SUE on {today}", level="warn")
            return None
        targets = book_weights(ctx, chosen, caps)
        place = {symbol: rank for rank, symbol in enumerate(ranked, 1)}
        reasons = {
            symbol: f"weight {weight:.3%}, composite {composite[symbol]:+.2f} ranks {place[symbol]} of {len(ranked)}: "
            + ", ".join(f"{name} z {z[name][symbol]:+.2f}" for name in COMPONENTS)
            for symbol, weight in targets.items()
            if symbol in composite
        }
        cash = ctx.params["cash"]
        if cash in targets:
            reasons[cash] = f"weight {targets[cash]:.3%} left over by the index-weight caps"
        ctx.log(
            f"{len(candidates)} non-financial members with a market cap, {len(ranked)} scored, {len(chosen)} held; "
            + missing_caps(members, caps)
        )
        return Book(targets, reasons, f"outside the top {len(chosen)} of {len(ranked)} by quality and momentum")
