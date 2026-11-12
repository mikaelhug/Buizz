from collections import defaultdict
from collections.abc import Iterable
from datetime import date
from itertools import pairwise

from buizz import Context, FundamentalPoint, History, Metric, split_ratio, to_ns

DOMESTIC_ANNUAL_FORMS = {"10-K", "10-K/A"}
MIN_GAP_DAYS = 300
MAX_GAP_DAYS = 430
MAX_STALENESS_DAYS = 548

Annual = dict[tuple[str, Metric], dict[date, FundamentalPoint]]


def domestic_annual(points: list[FundamentalPoint]) -> Annual:
    annual: Annual = defaultdict(dict)
    for point in points:
        if point.period_type != "FY" or point.form not in DOMESTIC_ANNUAL_FORMS:
            continue
        if point.metric == "shares_outstanding" and point.period_start is not None:
            continue
        annual[point.symbol, point.metric][point.period_end] = point
    return annual


def amount(annual: Annual, symbol: str, metric: Metric, period_end: date) -> float | None:
    point = annual.get((symbol, metric), {}).get(period_end)
    return None if point is None else float(point.value)


def consecutive_fiscal_ends(period_ends: Iterable[date], today: date, years: int) -> list[date] | None:
    ends = sorted(set(period_ends), reverse=True)[:years]
    if len(ends) < years or (today - ends[0]).days > MAX_STALENESS_DAYS:
        return None
    if not all(MIN_GAP_DAYS <= (later - earlier).days <= MAX_GAP_DAYS for later, earlier in pairwise(ends)):
        return None
    return ends


def adjusted_shares(point: FundamentalPoint, raw: History, adjusted: History) -> float | None:
    ratio = split_ratio(raw, adjusted, point.filed)
    return None if ratio is None else float(point.value) * ratio


def histories_since(ctx: Context, symbols: list[str], since: date) -> tuple[dict[str, History], dict[str, History]]:
    bars = len(ctx.calendar(since, ctx.session.date)) + 1
    return ctx.history(symbols, "1d", bars, "raw"), ctx.history(symbols, "1d", bars, "split")


def traded_today(ctx: Context, history: History) -> bool:
    return len(history.start) > 0 and history.start[-1] >= to_ns(ctx.session.open)
