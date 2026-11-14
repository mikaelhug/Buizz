from collections import defaultdict
from collections.abc import Callable, Iterable, Mapping
from datetime import date, timedelta
from itertools import pairwise

from buizz import Context, FundamentalPoint, History, Metric, split_ratio, to_ns

DOMESTIC_ANNUAL_FORMS = {"10-K", "10-K/A"}
DOMESTIC_FORMS = DOMESTIC_ANNUAL_FORMS | {"10-Q", "10-Q/A"}
MIN_GAP_DAYS = 300
MAX_GAP_DAYS = 430
MAX_STALENESS_DAYS = 548
TWO_FISCAL_YEARS_DAYS = MAX_STALENESS_DAYS + MAX_GAP_DAYS
SHARES_AFTER_PERIOD_END_DAYS = 120
QUARTER_TOLERANCE_DAYS = 7
QUARTERS_BEFORE_THE_FOURTH = 3
QUARTERS_PER_YEAR = 4
MAX_QUARTER_AGE_DAYS = 200
QUARTER_GAP_DAYS = range(80, 101)
NON_MONETARY_UNITS: dict[Metric, str] = {"shares_outstanding": "shares", "eps_diluted": "USD/shares"}

Annual = dict[tuple[str, Metric], dict[date, FundamentalPoint]]
PointCache = dict[str, tuple[date | None, list[FundamentalPoint]]]


def in_dollars(point: FundamentalPoint) -> bool:
    return point.unit == NON_MONETARY_UNITS.get(point.metric, "USD")


def domestic_annual(points: list[FundamentalPoint]) -> Annual:
    annual: Annual = defaultdict(dict)
    for point in points:
        if point.period_type != "FY" or point.form not in DOMESTIC_ANNUAL_FORMS or not in_dollars(point):
            continue
        if point.metric == "shares_outstanding" and point.period_start is not None:
            continue
        annual[point.symbol, point.metric][point.period_end] = point
    return annual


def amount(annual: Annual, symbol: str, metric: Metric, period_end: date) -> float | None:
    point = annual.get((symbol, metric), {}).get(period_end)
    return None if point is None else float(point.value)


def amount_or_zero(annual: Annual, symbol: str, metric: Metric, period_end: date) -> float:
    return amount(annual, symbol, metric, period_end) or 0.0


def current_debt(annual: Annual, symbol: str, period_end: date) -> float:
    reported = amount(annual, symbol, "short_term_debt", period_end)
    if reported is not None:
        return reported
    return amount_or_zero(annual, symbol, "short_term_borrowings", period_end) + amount_or_zero(
        annual, symbol, "current_long_term_debt", period_end
    )


def total_debt(annual: Annual, symbol: str, period_end: date) -> float:
    noncurrent = amount(annual, symbol, "long_term_debt_noncurrent", period_end)
    if noncurrent is not None:
        return noncurrent + current_debt(annual, symbol, period_end)
    reported_total = amount(annual, symbol, "long_term_debt", period_end)
    if reported_total is None:
        return current_debt(annual, symbol, period_end)
    return reported_total + amount_or_zero(annual, symbol, "short_term_borrowings", period_end)


def gross_profit(annual: Annual, symbol: str, period_end: date) -> float | None:
    reported = amount(annual, symbol, "gross_profit", period_end)
    if reported is not None:
        return reported
    revenue = amount(annual, symbol, "revenue", period_end)
    cost = amount(annual, symbol, "cost_of_revenue", period_end)
    return None if revenue is None or cost is None else revenue - cost


def gross_margin(annual: Annual, symbol: str, period_end: date) -> float | None:
    revenue = amount(annual, symbol, "revenue", period_end)
    profit = gross_profit(annual, symbol, period_end)
    return None if revenue is None or revenue <= 0 or profit is None else profit / revenue


def shares_after(counts: Mapping[date, FundamentalPoint], period_end: date) -> FundamentalPoint | None:
    after = [day for day in counts if 0 <= (day - period_end).days <= SHARES_AFTER_PERIOD_END_DAYS]
    return counts[min(after)] if after else None


def year_end_shares(annual: Annual, symbol: str, period_end: date) -> FundamentalPoint | None:
    return shares_after(annual.get((symbol, "shares_outstanding"), {}), period_end)


def with_fourth_quarters(
    quarters: Mapping[date, float], years: Iterable[tuple[date, date, float]]
) -> dict[date, float]:
    tolerance = timedelta(days=QUARTER_TOLERANCE_DAYS)
    filled = dict(quarters)
    for start, year_end, value in years:
        if any(abs(end - year_end) <= tolerance for end in quarters):
            continue
        inside = [end for end in quarters if start + tolerance < end < year_end - tolerance]
        if len(inside) == QUARTERS_BEFORE_THE_FOURTH:
            filled[year_end] = value - sum(quarters[end] for end in inside)
    return filled


def quarterly(durations: Iterable[tuple[FundamentalPoint, float]]) -> dict[date, float]:
    found = list(durations)
    return with_fourth_quarters(
        {point.period_end: value for point, value in found if point.period_type == "Q"},
        [
            (point.period_start, point.period_end, value)
            for point, value in found
            if point.period_type == "FY" and point.period_start is not None
        ],
    )


def trailing_year(quarters: Mapping[date, float], today: date) -> float | None:
    ends = sorted(quarters, reverse=True)[:QUARTERS_PER_YEAR]
    if len(ends) < QUARTERS_PER_YEAR or (today - ends[0]).days > MAX_QUARTER_AGE_DAYS:
        return None
    if any((later - earlier).days not in QUARTER_GAP_DAYS for later, earlier in pairwise(ends)):
        return None
    return sum(quarters[end] for end in ends)


def ratio(numerator: float | None, denominator: float | None) -> float | None:
    if numerator is None or denominator is None or denominator <= 0:
        return None
    return numerator / denominator


def consecutive_fiscal_ends(period_ends: Iterable[date], today: date, years: int) -> list[date] | None:
    ends = sorted(set(period_ends), reverse=True)[:years]
    if len(ends) < years or (today - ends[0]).days > MAX_STALENESS_DAYS:
        return None
    if not all(MIN_GAP_DAYS <= (later - earlier).days <= MAX_GAP_DAYS for later, earlier in pairwise(ends)):
        return None
    return ends


def latest_shares(points: Iterable[FundamentalPoint], today: date) -> FundamentalPoint | None:
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


def latest_filing(points: Iterable[FundamentalPoint], forms: set[str]) -> date | None:
    return max((point.filed for point in points if point.form in forms), default=None)


def by_symbol(points: Iterable[FundamentalPoint]) -> defaultdict[str, list[FundamentalPoint]]:
    grouped: dict[str, list[FundamentalPoint]] = defaultdict(list)
    for point in points:
        grouped[point.symbol].append(point)
    return grouped


def recent(today: date, days: int, period_type: str, forms: set[str]) -> Callable[[FundamentalPoint], bool]:
    return lambda point: (
        point.period_type == period_type
        and point.form in forms
        and in_dollars(point)
        and (today - point.period_end).days <= days
    )


def refreshed(
    ctx: Context,
    metrics: list[Metric],
    filings: Mapping[str, date | None],
    cache: PointCache,
    keep: Callable[[FundamentalPoint], bool],
) -> dict[str, list[FundamentalPoint]]:
    stale = sorted(symbol for symbol, filed in filings.items() if symbol not in cache or cache[symbol][0] != filed)
    if stale:
        fresh = by_symbol(point for point in ctx.fundamentals(stale, metrics) if keep(point))
        cache.update({symbol: (filings[symbol], fresh.get(symbol, [])) for symbol in stale})
    return {symbol: cache[symbol][1] for symbol in filings}


def adjusted_shares(point: FundamentalPoint, raw: History, adjusted: History) -> float | None:
    ratio = split_ratio(raw, adjusted, point.filed)
    return None if ratio is None else float(point.value) * ratio


def histories_since(ctx: Context, symbols: list[str], since: date) -> tuple[dict[str, History], dict[str, History]]:
    bars = len(ctx.calendar(since, ctx.session.date)) + 1
    return ctx.history(symbols, "1d", bars, "raw"), ctx.history(symbols, "1d", bars, "split")


def traded_today(ctx: Context, history: History) -> bool:
    return len(history.start) > 0 and history.start[-1] >= to_ns(ctx.session.open)
