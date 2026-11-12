import math
from collections import defaultdict
from collections.abc import Iterable
from datetime import date, timedelta
from typing import NamedTuple

import numpy as np
import numpy.typing as npt
from buizz_bots.beta import HISTORY_BARS, bab_weights, frazzini_pedersen_beta
from buizz_bots.drift import rebalance_within_band
from buizz_bots.fundamentals import (
    DOMESTIC_ANNUAL_FORMS,
    DOMESTIC_FORMS,
    MAX_STALENESS_DAYS,
    Annual,
    adjusted_shares,
    amount,
    amount_or_zero,
    current_debt,
    domestic_annual,
    gross_profit,
    in_dollars,
    total_debt,
    traded_today,
    with_fourth_quarters,
    year_end_shares,
)
from buizz_bots.monthly import month_end_closes
from buizz_bots.sessions import last_session_of_month
from scipy.stats import rankdata

from buizz import Bot, Context, FundamentalPoint, History, MacroPoint, Metric, run

QUARTERLY_METRICS: list[Metric] = ["net_income", "equity", "preferred_stock", "shares_outstanding"]
ANNUAL_METRICS: list[Metric] = [
    "revenue",
    "cost_of_revenue",
    "gross_profit",
    "operating_income",
    "pretax_income",
    "depreciation_amortization",
    "capex",
    "total_assets",
    "current_assets",
    "cash_and_equivalents",
    "short_term_investments",
    "current_liabilities",
    "short_term_debt",
    "short_term_borrowings",
    "current_long_term_debt",
    "income_taxes_payable",
    "long_term_debt",
    "long_term_debt_noncurrent",
    "total_liabilities",
    "minority_interest",
    "retained_earnings",
]
FACTORS = ("mkt", "smb", "hml", "umd", "bab", "qmj")
PROFITABILITY = ("gpoa", "roe", "roa", "cfoa", "gmar", "acc")
GROWTH = ("growth_gpoa", "growth_roe", "growth_roa", "growth_cfoa", "growth_gmar")
SAFETY = ("bab", "lev", "ohlson", "altman", "evol")
MARKET = "VTI"
CHUNK = 200
DOLLAR_VOLUME_SESSIONS = 20
YEAR_TOLERANCE_DAYS = 45
MOMENTUM_MONTHS = 12
EVOL_QUARTERS = 60
EVOL_MIN_QUARTERS = 12
GROWTH_YEARS = 5
YEAR_LAGS = (0, 1, GROWTH_YEARS, GROWTH_YEARS + 1)
BREAKPOINTS = (0.3, 0.7)
MIN_CROSS_SECTION = 2

Quarterly = dict[date, float]
AnnualCache = dict[str, tuple[date | None, Annual]]


class Year(NamedTuple):
    end: date
    shares: float | None
    assets: float
    book: float | None
    gross_profit: float | None
    income: float | None
    sales: float | None
    working_capital: float | None
    depreciation: float | None
    capex: float | None


class RateHistory(NamedTuple):
    days: npt.NDArray[np.datetime64]
    cumulative: npt.NDArray[np.float64]


class Firm(NamedTuple):
    symbol: str
    identity: tuple[date, date, str]
    market_equity: float
    dollar_volume: float
    book: float | None
    momentum: float | None
    beta: float | None
    measures: dict[str, float]


class Holding(NamedTuple):
    symbol: str
    active: float
    contributions: dict[str, float]


def years_back(end: date, years: int) -> date:
    return end - timedelta(days=round(365.25 * years))


def nearest_end(ends: Iterable[date], target: date) -> date | None:
    close = [end for end in ends if abs((end - target).days) <= YEAR_TOLERANCE_DAYS]
    return min(close, key=lambda end: abs((end - target).days)) if close else None


def shareholders_equity(annual: Annual, symbol: str, end: date) -> float | None:
    reported = amount(annual, symbol, "equity", end)
    if reported is not None:
        return reported
    assets = amount(annual, symbol, "total_assets", end)
    liabilities = amount(annual, symbol, "total_liabilities", end)
    if assets is None or liabilities is None:
        return None
    return assets - liabilities - amount_or_zero(annual, symbol, "minority_interest", end)


def total_liabilities(annual: Annual, symbol: str, end: date) -> float | None:
    reported = amount(annual, symbol, "total_liabilities", end)
    if reported is not None:
        return reported
    assets = amount(annual, symbol, "total_assets", end)
    equity = amount(annual, symbol, "equity", end)
    if assets is None or equity is None:
        return None
    return assets - equity - amount_or_zero(annual, symbol, "minority_interest", end)


def book_equity(annual: Annual, symbol: str, end: date) -> float | None:
    equity = shareholders_equity(annual, symbol, end)
    if equity is None:
        return None
    return equity - amount_or_zero(annual, symbol, "preferred_stock", end)


def working_capital(annual: Annual, symbol: str, end: date) -> float | None:
    current_assets = amount(annual, symbol, "current_assets", end)
    current_liabilities = amount(annual, symbol, "current_liabilities", end)
    if current_assets is None or current_liabilities is None:
        return None
    return (
        current_assets
        - current_liabilities
        - amount_or_zero(annual, symbol, "cash_and_equivalents", end)
        - amount_or_zero(annual, symbol, "short_term_investments", end)
        + current_debt(annual, symbol, end)
        + amount_or_zero(annual, symbol, "income_taxes_payable", end)
    )


def latest_shares(points: list[FundamentalPoint], today: date) -> FundamentalPoint | None:
    counts = [
        point
        for point in points
        if point.metric == "shares_outstanding"
        and point.period_start is None
        and point.form in DOMESTIC_FORMS
        and in_dollars(point)
        and point.value > 0
    ]
    if not counts:
        return None
    newest = max(counts, key=lambda point: (point.period_end, point.filed))
    return newest if (today - newest.period_end).days <= MAX_STALENESS_DAYS else None


def fiscal_year(annual: Annual, symbol: str, end: date, shares: float | None) -> Year | None:
    assets = amount(annual, symbol, "total_assets", end)
    if assets is None or assets <= 0:
        return None
    return Year(
        end,
        shares,
        assets,
        book_equity(annual, symbol, end),
        gross_profit(annual, symbol, end),
        amount(annual, symbol, "net_income", end),
        amount(annual, symbol, "revenue", end),
        working_capital(annual, symbol, end),
        amount(annual, symbol, "depreciation_amortization", end),
        amount(annual, symbol, "capex", end),
    )


def cash_flow(year: Year, prior: Year | None) -> float | None:
    if (
        prior is None
        or year.income is None
        or year.depreciation is None
        or year.capex is None
        or year.working_capital is None
        or prior.working_capital is None
    ):
        return None
    return year.income + year.depreciation - (year.working_capital - prior.working_capital) - year.capex


def ratio(numerator: float | None, denominator: float | None) -> float | None:
    if numerator is None or denominator is None or denominator <= 0:
        return None
    return numerator / denominator


def per_share(value: float | None, year: Year | None) -> float | None:
    return None if year is None else ratio(value, year.shares)


def residual(measure: float | None, charge_base: float | None, rate: float | None) -> float | None:
    if measure is None or charge_base is None or rate is None:
        return None
    return measure - rate * charge_base


def change(now: float | None, old: float | None, scale: float | None) -> float | None:
    if now is None or old is None:
        return None
    return ratio(now - old, scale)


def growth_measures(years: dict[int, Year | None], rates: RateHistory) -> dict[str, float]:
    now, prior, old, older = (years[0], years[1], years[GROWTH_YEARS], years[GROWTH_YEARS + 1])
    if now is None or old is None:
        return {}
    rate_now, rate_old = annual_rate(rates, now.end), annual_rate(rates, old.end)
    assets_prior = per_share(prior.assets if prior else None, prior)
    assets_older = per_share(older.assets if older else None, older)
    assets_old = per_share(old.assets, old)

    def growth(
        measure_now: float | None,
        measure_old: float | None,
        base_prior: float | None,
        base_older: float | None,
        scale: float | None,
    ) -> float | None:
        return change(
            residual(per_share(measure_now, now), base_prior, rate_now),
            residual(per_share(measure_old, old), base_older, rate_old),
            scale,
        )

    candidates = {
        "growth_gpoa": growth(now.gross_profit, old.gross_profit, assets_prior, assets_older, assets_old),
        "growth_roe": growth(
            now.income,
            old.income,
            per_share(prior.book if prior else None, prior),
            per_share(older.book if older else None, older),
            per_share(old.book, old),
        ),
        "growth_roa": growth(now.income, old.income, assets_prior, assets_older, assets_old),
        "growth_cfoa": growth(cash_flow(now, prior), cash_flow(old, older), assets_prior, assets_older, assets_old),
        "growth_gmar": change(
            per_share(now.gross_profit, now), per_share(old.gross_profit, old), per_share(old.sales, old)
        ),
    }
    return {name: value for name, value in candidates.items() if value is not None}


def profitability_measures(now: Year, prior: Year | None) -> dict[str, float]:
    accruals = None
    if (
        prior is not None
        and now.working_capital is not None
        and prior.working_capital is not None
        and now.depreciation is not None
    ):
        accruals = -((now.working_capital - prior.working_capital) - now.depreciation) / now.assets
    candidates = {
        "gpoa": ratio(now.gross_profit, now.assets),
        "roe": ratio(now.income, now.book),
        "roa": ratio(now.income, now.assets),
        "cfoa": ratio(cash_flow(now, prior), now.assets),
        "gmar": ratio(now.gross_profit, now.sales),
        "acc": accruals,
    }
    return {name: value for name, value in candidates.items() if value is not None}


def ohlson(annual: Annual, symbol: str, now: Year, prior: Year | None, market_equity: float) -> float | None:
    end = now.end
    current_assets = amount(annual, symbol, "current_assets", end)
    current_liabilities = amount(annual, symbol, "current_liabilities", end)
    liabilities = total_liabilities(annual, symbol, end)
    pretax = amount(annual, symbol, "pretax_income", end)
    if (
        now.book is None
        or now.income is None
        or prior is None
        or prior.income is None
        or current_assets is None
        or current_assets <= 0
        or current_liabilities is None
        or liabilities is None
        or liabilities <= 0
        or pretax is None
    ):
        return None
    adjusted_assets = now.assets + 0.1 * (market_equity - now.book)
    if adjusted_assets <= 0:
        return None
    debt = total_debt(annual, symbol, end)
    income_swing = abs(now.income) + abs(prior.income)
    income_change = 0.0 if income_swing == 0 else (now.income - prior.income) / income_swing
    score = (
        -1.32
        - 0.407 * math.log(adjusted_assets)
        + 6.03 * debt / adjusted_assets
        - 1.43 * (current_assets - current_liabilities) / adjusted_assets
        + 0.076 * current_liabilities / current_assets
        - 1.72 * float(liabilities > now.assets)
        - 2.37 * now.income / now.assets
        - 1.83 * pretax / liabilities
        + 0.285 * float(max(now.income, prior.income) < 0)
        - 0.521 * income_change
    )
    return -score


def altman(annual: Annual, symbol: str, now: Year, market_equity: float) -> float | None:
    current_assets = amount(annual, symbol, "current_assets", now.end)
    current_liabilities = amount(annual, symbol, "current_liabilities", now.end)
    retained = amount(annual, symbol, "retained_earnings", now.end)
    operating = amount(annual, symbol, "operating_income", now.end)
    if (
        current_assets is None
        or current_liabilities is None
        or retained is None
        or operating is None
        or now.sales is None
    ):
        return None
    working_capital = current_assets - current_liabilities
    return (1.2 * working_capital + 1.4 * retained + 3.3 * operating + 0.6 * market_equity + now.sales) / now.assets


def leverage(annual: Annual, symbol: str, now: Year) -> float:
    end = now.end
    debt = (
        total_debt(annual, symbol, end)
        + amount_or_zero(annual, symbol, "minority_interest", end)
        + amount_or_zero(annual, symbol, "preferred_stock", end)
    )
    return -debt / now.assets


def quarterly_income(points: list[FundamentalPoint]) -> Quarterly:
    durations = [
        point
        for point in points
        if point.metric == "net_income"
        and point.form in DOMESTIC_FORMS
        and in_dollars(point)
        and point.period_start is not None
    ]
    return with_fourth_quarters(
        {point.period_end: float(point.value) for point in durations if point.period_type == "Q"},
        [
            (point.period_start, point.period_end, float(point.value))
            for point in durations
            if point.period_type == "FY" and point.period_start is not None
        ],
    )


def quarter_end_books(points: list[FundamentalPoint]) -> dict[date, float]:
    equity: dict[date, float] = {}
    preferred: dict[date, float] = {}
    for point in sorted(points, key=lambda point: point.filed):
        if point.form not in DOMESTIC_FORMS or point.period_start is not None or not in_dollars(point):
            continue
        if point.metric == "equity":
            equity[point.period_end] = float(point.value)
        elif point.metric == "preferred_stock":
            preferred[point.period_end] = float(point.value)
    return {end: value - preferred.get(end, 0.0) for end, value in equity.items()}


def earnings_volatility(points: list[FundamentalPoint], today: date) -> float | None:
    books = quarter_end_books(points)
    returns = [
        income / books[end]
        for end, income in sorted(quarterly_income(points).items())
        if end <= today and books.get(end, 0.0) > 0
    ][-EVOL_QUARTERS:]
    if len(returns) < EVOL_MIN_QUARTERS:
        return None
    return float(np.std(returns, ddof=1))


def rate_history(points: list[MacroPoint]) -> RateHistory:
    ordered = sorted(points, key=lambda point: point.date)
    days = np.array([point.date for point in ordered], dtype="datetime64[D]")
    cumulative = np.concatenate(([0.0], np.cumsum([float(point.value) for point in ordered])))
    return RateHistory(days, cumulative)


def annual_rate(rates: RateHistory, end: date) -> float | None:
    upper = int(np.searchsorted(rates.days, np.datetime64(end), side="right"))
    lower = int(np.searchsorted(rates.days, np.datetime64(end - timedelta(days=365)), side="right"))
    if upper == lower:
        return None
    return float(rates.cumulative[upper] - rates.cumulative[lower]) / (upper - lower) / 100


def momentum(history: History, today: date) -> float | None:
    closes = month_end_closes(history, today)
    if closes.size < MOMENTUM_MONTHS + 1 or closes[-MOMENTUM_MONTHS - 1] <= 0:
        return None
    return float(closes[-2] / closes[-MOMENTUM_MONTHS - 1] - 1)


def fiscal_year_ends(annual: Annual, symbol: str, today: date) -> dict[int, date]:
    ends = [end for end in annual.get((symbol, "total_assets"), {}) if end <= today]
    recent = max(ends, default=None)
    if recent is None or (today - recent).days > MAX_STALENESS_DAYS:
        return {}
    found = {lag: nearest_end(ends, years_back(recent, lag)) for lag in YEAR_LAGS}
    return {lag: end for lag, end in found.items() if end is not None}


def oldest_filed(by_symbol: dict[str, list[FundamentalPoint]], annual: Annual, today: date) -> dict[str, date]:
    oldest: dict[str, date] = {}
    for symbol, own in by_symbol.items():
        latest = latest_shares(own, today)
        if latest is None:
            continue
        counts = [year_end_shares(annual, symbol, end) for end in fiscal_year_ends(annual, symbol, today).values()]
        oldest[symbol] = min([latest.filed, *(count.filed for count in counts if count is not None)])
    return oldest


def year_at(annual: Annual, symbol: str, end: date | None, raw: History, split: History) -> Year | None:
    if end is None:
        return None
    count = year_end_shares(annual, symbol, end)
    shares = None if count is None else adjusted_shares(count, raw, split)
    return fiscal_year(annual, symbol, end, shares)


def firm_of(
    ctx: Context,
    symbol: str,
    own: list[FundamentalPoint],
    annual: Annual,
    raw: History,
    split: History,
    total_return: History,
    market: History,
    rates: RateHistory,
) -> Firm | None:
    today = ctx.session.date
    latest = latest_shares(own, today)
    if latest is None or not traded_today(ctx, raw):
        return None
    shares = adjusted_shares(latest, raw, split)
    if shares is None:
        return None
    market_equity = shares * float(raw.close[-1])
    dollar_volume = float(np.mean(raw.close[-DOLLAR_VOLUME_SESSIONS:] * raw.volume[-DOLLAR_VOLUME_SESSIONS:]))
    beta = frazzini_pedersen_beta(total_return, market)
    measures: dict[str, float] = {}
    if beta is not None:
        measures["bab"] = -beta
    volatility = earnings_volatility(own, today)
    if volatility is not None:
        measures["evol"] = -volatility
    ends = fiscal_year_ends(annual, symbol, today)
    years = {lag: year_at(annual, symbol, ends.get(lag), raw, split) for lag in YEAR_LAGS}
    now = years[0]
    identity = (latest.period_end, latest.filed, str(latest.value))
    if now is not None:
        assets = annual[symbol, "total_assets"][now.end]
        identity = (now.end, assets.filed, str(assets.value))
        measures |= profitability_measures(now, years[1])
        measures |= growth_measures(years, rates)
        measures["lev"] = leverage(annual, symbol, now)
        o_score = ohlson(annual, symbol, now, years[1], market_equity)
        if o_score is not None:
            measures["ohlson"] = o_score
        z_score = altman(annual, symbol, now, market_equity)
        if z_score is not None:
            measures["altman"] = z_score
    return Firm(
        symbol,
        identity,
        market_equity,
        dollar_volume,
        None if now is None else now.book,
        momentum(total_return, today),
        beta,
        measures,
    )


def rank_z(values: dict[str, float]) -> dict[str, float]:
    if len(values) < MIN_CROSS_SECTION:
        return {}
    symbols = sorted(values)
    ranks = rankdata([values[symbol] for symbol in symbols])
    spread = float(np.std(ranks))
    if spread == 0:
        return {}
    scores = (ranks - ranks.mean()) / spread
    return dict(zip(symbols, scores.tolist(), strict=True))


def composite(parts: list[dict[str, float]]) -> dict[str, float]:
    collected: dict[str, list[float]] = defaultdict(list)
    for part in parts:
        for symbol, score in part.items():
            collected[symbol].append(score)
    return rank_z({symbol: sum(scores) / len(scores) for symbol, scores in collected.items()})


def quality_scores(firms: list[Firm]) -> dict[str, float]:
    def measure_z(name: str) -> dict[str, float]:
        return rank_z({firm.symbol: firm.measures[name] for firm in firms if name in firm.measures})

    components = [composite([measure_z(name) for name in group]) for group in (PROFITABILITY, GROWTH, SAFETY)]
    return composite(components)


Buckets = dict[tuple[bool, int], list[str]]


def conditional_sort(values: dict[str, float], big: set[str], nyse: set[str]) -> Buckets:
    buckets: Buckets = defaultdict(list)
    for is_big in (False, True):
        members = sorted(symbol for symbol in values if (symbol in big) == is_big)
        reference = [values[symbol] for symbol in members if symbol in nyse]
        if not reference:
            continue
        low, high = np.quantile(reference, BREAKPOINTS)
        for symbol in members:
            value = values[symbol]
            buckets[is_big, 0 if value <= low else 2 if value >= high else 1].append(symbol)
    return buckets


def value_weighted(symbols: list[str], market_equity: dict[str, float], scale: float) -> dict[str, float]:
    size = sum(market_equity[symbol] for symbol in symbols)
    return {symbol: scale * market_equity[symbol] / size for symbol in symbols} if size > 0 else {}


def combine(parts: Iterable[dict[str, float]]) -> dict[str, float]:
    combined: dict[str, float] = defaultdict(float)
    for part in parts:
        for symbol, weight in part.items():
            combined[symbol] += weight
    return dict(combined)


def high_minus_low(buckets: Buckets, market_equity: dict[str, float]) -> dict[str, float]:
    return combine(
        value_weighted(buckets.get((is_big, tercile), []), market_equity, sign / 2)
        for is_big in (False, True)
        for tercile, sign in ((2, 1.0), (0, -1.0))
    )


def small_minus_big(buckets: Buckets, market_equity: dict[str, float]) -> dict[str, float]:
    return combine(
        value_weighted(buckets.get((is_big, tercile), []), market_equity, (-1.0 if is_big else 1.0) / 3)
        for is_big in (False, True)
        for tercile in (0, 1, 2)
    )


def factor_weights(firms: list[Firm], nyse: set[str]) -> dict[str, dict[str, float]]:
    market_equity = {firm.symbol: firm.market_equity for firm in firms}
    nyse_sizes = [firm.market_equity for firm in firms if firm.symbol in nyse]
    median = float(np.median(nyse_sizes))
    big = {firm.symbol for firm in firms if firm.market_equity > median}
    book_to_market = conditional_sort(
        {firm.symbol: firm.book / firm.market_equity for firm in firms if firm.book is not None and firm.book > 0},
        big,
        nyse,
    )
    winners = conditional_sort({firm.symbol: firm.momentum for firm in firms if firm.momentum is not None}, big, nyse)
    quality = conditional_sort(quality_scores(firms), big, nyse)
    return {
        "mkt": value_weighted(sorted(market_equity), market_equity, 1.0),
        "smb": small_minus_big(book_to_market, market_equity),
        "hml": high_minus_low(book_to_market, market_equity),
        "umd": high_minus_low(winners, market_equity),
        "bab": bab_weights({firm.symbol: firm.beta for firm in firms if firm.beta is not None}),
        "qmj": high_minus_low(quality, market_equity),
    }


def one_line_per_company(firms: list[Firm]) -> list[Firm]:
    lines: dict[tuple[date, date, str], Firm] = {}
    for firm in firms:
        kept = lines.get(firm.identity)
        if kept is None or (firm.dollar_volume, firm.symbol) > (kept.dollar_volume, kept.symbol):
            lines[firm.identity] = firm
    return sorted(lines.values(), key=lambda firm: firm.symbol)


def latest_annual_filing(points: list[FundamentalPoint]) -> date | None:
    return max((point.filed for point in points if point.form in DOMESTIC_ANNUAL_FORMS), default=None)


def years_in_use(fresh: Annual, symbol: str, today: date) -> Annual:
    ends = set(fiscal_year_ends(fresh, symbol, today).values())
    return {
        (symbol, metric): {end: point for end, point in fresh[symbol, metric].items() if end in ends}
        for metric in ANNUAL_METRICS
        if (symbol, metric) in fresh
    }


def annual_of(
    ctx: Context, points: list[FundamentalPoint], filed: dict[str, date | None], cache: AnnualCache
) -> Annual:
    today = ctx.session.date
    stale = sorted(symbol for symbol, latest in filed.items() if symbol not in cache or cache[symbol][0] != latest)
    if stale:
        fresh = domestic_annual(ctx.fundamentals(stale, ANNUAL_METRICS))
        cache.update({symbol: (filed[symbol], years_in_use(fresh, symbol, today)) for symbol in stale})
    annual = domestic_annual(points)
    for symbol in filed:
        annual.update(cache[symbol][1])
    return annual


def firms_of(ctx: Context, cache: AnnualCache) -> list[Firm]:
    today = ctx.session.date
    market = ctx.history([MARKET], "1d", HISTORY_BARS, "all")[MARKET]
    rates = rate_history(ctx.macro(["treasury_3m"], years_back(today, GROWTH_YEARS + 3), today))
    firms: list[Firm] = []
    universe = sorted(ctx.universe)
    for offset in range(0, len(universe), CHUNK):
        chunk = universe[offset : offset + CHUNK]
        points = ctx.fundamentals(chunk, QUARTERLY_METRICS)
        by_symbol: dict[str, list[FundamentalPoint]] = defaultdict(list)
        for point in points:
            by_symbol[point.symbol].append(point)
        annual = annual_of(ctx, points, {symbol: latest_annual_filing(by_symbol[symbol]) for symbol in chunk}, cache)
        oldest = oldest_filed(by_symbol, annual, today)
        if not oldest:
            continue
        bars = max(len(ctx.calendar(min(oldest.values()), today)) + 1, DOLLAR_VOLUME_SESSIONS)
        listed = sorted(oldest)
        raw = ctx.history(listed, "1d", bars, "raw")
        split = ctx.history(listed, "1d", bars, "split")
        total_return = ctx.history(listed, "1d", HISTORY_BARS, "all")
        for symbol in listed:
            firm = firm_of(
                ctx,
                symbol,
                by_symbol[symbol],
                annual,
                raw[symbol],
                split[symbol],
                total_return[symbol],
                market,
                rates,
            )
            if firm is not None:
                firms.append(firm)
    return one_line_per_company(firms)


def active_book(firms: list[Firm], nyse: set[str], loadings: dict[str, float]) -> list[Holding]:
    weights = factor_weights(firms, nyse)
    holdings = [
        Holding(
            firm.symbol,
            sum(loadings[factor] * weights[factor].get(firm.symbol, 0.0) for factor in FACTORS),
            {factor: loadings[factor] * weights[factor].get(firm.symbol, 0.0) for factor in FACTORS},
        )
        for firm in firms
    ]
    return sorted(holdings, key=lambda holding: (-holding.active, holding.symbol))


def reason_for(holding: Holding, rank: int, ranked: int) -> str:
    parts = sorted(holding.contributions.items(), key=lambda item: -abs(item[1]))
    tilts = ", ".join(f"{factor} {weight * 1e4:+.2f}" for factor, weight in parts)
    return f"active Buffett tilt {holding.active * 1e4:+.2f} bp ranks {rank} of {ranked}: {tilts} bp"


class Buffett(Bot):
    def __init__(self) -> None:
        self.cache: AnnualCache = {}

    def on_start(self, ctx: Context) -> None:
        variant = ctx.params["variant"]
        if variant != "long_only":
            raise ValueError(f"variant {variant!r} is not available: the long/short book needs per-name borrow (G10)")

    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        universe = set(ctx.universe)
        self.cache = {symbol: cached for symbol, cached in self.cache.items() if symbol in universe}
        firms = firms_of(ctx, self.cache)
        profiles = ctx.instruments([firm.symbol for firm in firms])
        nyse = {symbol for symbol, profile in profiles.items() if profile.exchange == "NYSE"}
        if len([firm for firm in firms if firm.symbol in nyse]) < MIN_CROSS_SECTION:
            ctx.log(f"{len(firms)} stocks with market equity, too few on the NYSE for breakpoints", level="warn")
            return
        loadings = {factor: float(ctx.params["loadings"][factor]) for factor in FACTORS}
        ranked = active_book(firms, nyse, loadings)
        top = ranked[: int(ctx.params["holdings"])]
        rebalance_within_band(
            ctx,
            dict.fromkeys((holding.symbol for holding in top), 1 / len(top)),
            float(ctx.params["drift_band"]),
            reason={holding.symbol: reason_for(holding, rank, len(ranked)) for rank, holding in enumerate(top, 1)},
            exit_reason=f"outside the top {len(top)} of {len(ranked)} by active Buffett tilt",
        )


if __name__ == "__main__":
    run(Buffett)
