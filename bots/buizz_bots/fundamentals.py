from collections import defaultdict
from datetime import date

from buizz import FundamentalPoint, History, Metric, split_ratio

DOMESTIC_ANNUAL_FORMS = {"10-K", "10-K/A"}
DOMESTIC_FORMS = DOMESTIC_ANNUAL_FORMS | {"10-Q", "10-Q/A"}

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


def adjusted_shares(point: FundamentalPoint, raw: History, adjusted: History) -> float | None:
    ratio = split_ratio(raw, adjusted, point.filed)
    return None if ratio is None else float(point.value) * ratio
