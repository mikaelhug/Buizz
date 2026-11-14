import math
from datetime import date, timedelta

from buizz import Context, FundamentalPoint, History, Metric
from buizz_bots.caps import MIN_HISTORY_DAYS, market_caps, missing_caps
from buizz_bots.drift import Book
from buizz_bots.fundamentals import (
    DOMESTIC_ANNUAL_FORMS,
    TWO_FISCAL_YEARS_DAYS,
    Annual,
    PointCache,
    adjusted_shares,
    amount,
    amount_or_zero,
    by_symbol,
    consecutive_fiscal_ends,
    domestic_annual,
    gross_profit,
    histories_since,
    latest_filing,
    latest_shares,
    recent,
    refreshed,
    total_debt,
    year_end_shares,
)
from buizz_bots.monthly import momentum, momentum_sessions
from buizz_bots.ranks import highest_first, percentiles
from buizz_bots.sessions import REBALANCED, first_step_or_month_end

ANNUAL_METRICS: list[Metric] = [
    "gross_profit",
    "revenue",
    "cost_of_revenue",
    "total_assets",
    "net_income",
    "operating_cash_flow",
    "current_assets",
    "current_liabilities",
    "total_liabilities",
    "cash_and_equivalents",
    "short_term_investments",
    "equity",
    "minority_interest",
    "preferred_stock",
    "long_term_debt",
    "long_term_debt_noncurrent",
    "short_term_debt",
    "short_term_borrowings",
    "current_long_term_debt",
    "ppe_gross",
    "ppe_net",
    "inventory",
    "pretax_income",
    "depreciation_amortization",
]
SCHEDULES = {"monthly": range(1, 13), "quarterly": (3, 6, 9, 12)}
FUNDAMENTAL = (
    "profitability",
    "accruals",
    "issuance",
    "roa",
    "asset_growth",
    "investment",
    "net_operating_assets",
    "o_score",
)
MEASURES = (*FUNDAMENTAL, "momentum")
GROWTH_YEARS = 2


def fiscal_ends(annual: Annual, symbol: str, today: date, years: int) -> list[date] | None:
    return consecutive_fiscal_ends(annual.get((symbol, "total_assets"), {}), today, years)


def year_end_counts(annual: Annual, symbol: str, today: date) -> list[FundamentalPoint]:
    ends = fiscal_ends(annual, symbol, today, GROWTH_YEARS) or []
    return [count for end in ends if (count := year_end_shares(annual, symbol, end)) is not None]


def share_growth(annual: Annual, symbol: str, today: date, raw: History, split: History) -> float | None:
    counts = year_end_counts(annual, symbol, today)
    if len(counts) < GROWTH_YEARS:
        return None
    now, prior = (adjusted_shares(count, raw, split) for count in counts)
    if now is None or prior is None or now <= 0 or prior <= 0:
        return None
    return math.log(now / prior)


def property_plant(annual: Annual, symbol: str, now: date, prior: date) -> tuple[float, float] | None:
    for metric in ("ppe_gross", "ppe_net"):
        pair = amount(annual, symbol, metric, now), amount(annual, symbol, metric, prior)
        if pair[0] is not None and pair[1] is not None:
            return pair[0], pair[1]
    return None


def investment(annual: Annual, symbol: str, now: date, prior: date, lagged_assets: float) -> float | None:
    if (plant := property_plant(annual, symbol, now, prior)) is None:
        return None
    stock = amount_or_zero(annual, symbol, "inventory", now) - amount_or_zero(annual, symbol, "inventory", prior)
    return (plant[0] - plant[1] + stock) / lagged_assets


def net_operating_assets(annual: Annual, symbol: str, end: date, assets: float, lagged_assets: float) -> float | None:
    cash = amount(annual, symbol, "cash_and_equivalents", end)
    equity = amount(annual, symbol, "equity", end)
    if cash is None or equity is None:
        return None
    operating_assets = assets - cash - amount_or_zero(annual, symbol, "short_term_investments", end)
    operating_liabilities = (
        assets
        - total_debt(annual, symbol, end)
        - amount_or_zero(annual, symbol, "minority_interest", end)
        - amount_or_zero(annual, symbol, "preferred_stock", end)
        - equity
    )
    return (operating_assets - operating_liabilities) / lagged_assets


def o_score(annual: Annual, symbol: str, now: date, prior: date, assets: float) -> float | None:
    def value(metric: Metric, end: date = now) -> float | None:
        return amount(annual, symbol, metric, end)

    liabilities, current_assets, current_liabilities = (
        value("total_liabilities"),
        value("current_assets"),
        value("current_liabilities"),
    )
    income, last_income, pretax = value("net_income"), value("net_income", prior), value("pretax_income")
    if (
        liabilities is None
        or liabilities <= 0
        or current_assets is None
        or current_assets <= 0
        or current_liabilities is None
        or income is None
        or last_income is None
        or pretax is None
    ):
        return None
    funds = pretax + amount_or_zero(annual, symbol, "depreciation_amortization", now)
    swing = abs(income) + abs(last_income)
    return (
        -1.32
        - 0.407 * math.log(assets)
        + 6.03 * liabilities / assets
        - 1.43 * (current_assets - current_liabilities) / assets
        + 0.0757 * current_liabilities / current_assets
        - 1.72 * (liabilities > assets)
        - 2.37 * income / assets
        - 1.83 * funds / liabilities
        + 0.285 * (income < 0 and last_income < 0)
        - 0.521 * ((income - last_income) / swing if swing > 0 else 0.0)
    )


def junk_measures(annual: Annual, symbol: str, today: date, raw: History, split: History) -> dict[str, float]:
    latest = fiscal_ends(annual, symbol, today, 1)
    if latest is None:
        return {}
    end = latest[0]
    assets = amount(annual, symbol, "total_assets", end)
    if assets is None or assets <= 0:
        return {}
    measures: dict[str, float] = {}
    profit = gross_profit(annual, symbol, end)
    if profit is not None:
        measures["profitability"] = -profit / assets
    income = amount(annual, symbol, "net_income", end)
    cash = amount(annual, symbol, "operating_cash_flow", end)
    if income is not None:
        measures["roa"] = -income / assets
        if cash is not None:
            measures["accruals"] = (income - cash) / assets
    growth = share_growth(annual, symbol, today, raw, split)
    if growth is not None:
        measures["issuance"] = growth
    both = fiscal_ends(annual, symbol, today, GROWTH_YEARS)
    lagged = None if both is None else amount(annual, symbol, "total_assets", both[1])
    if both is not None and lagged is not None and lagged > 0:
        prior = both[1]
        measures["asset_growth"] = assets / lagged - 1
        if (invested := investment(annual, symbol, end, prior, lagged)) is not None:
            measures["investment"] = invested
        if (operating := net_operating_assets(annual, symbol, end, assets, lagged)) is not None:
            measures["net_operating_assets"] = operating
        if (distress := o_score(annual, symbol, end, prior, assets)) is not None:
            measures["o_score"] = distress
    return measures


def junk_scores(measures: dict[str, dict[str, float]]) -> dict[str, float]:
    ranks = {
        name: percentiles({symbol: own[name] for symbol, own in measures.items() if name in own}) for name in MEASURES
    }
    return {
        symbol: sum(ranks[name][symbol] for name in own) / len(own)
        for symbol, own in measures.items()
        if any(name in own for name in FUNDAMENTAL)
    }


class ExJunk:
    def __init__(self) -> None:
        self.annual: PointCache = {}

    def due(self, ctx: Context) -> bool:
        return first_step_or_month_end(ctx, SCHEDULES[ctx.params["rebalance"]])

    def book(self, ctx: Context) -> Book | None:
        today = ctx.session.date
        ctx.state[REBALANCED] = today.isoformat()
        members = sorted(ctx.universe)
        own = by_symbol(ctx.fundamentals(members, ["shares_outstanding"]))
        counts = {symbol: count for symbol in members if (count := latest_shares(own[symbol], today)) is not None}
        filings = {symbol: latest_filing(own[symbol], DOMESTIC_ANNUAL_FORMS) for symbol in members}
        self.annual = {symbol: self.annual[symbol] for symbol in members if symbol in self.annual}
        cached = refreshed(
            ctx, ANNUAL_METRICS, filings, self.annual, recent(today, TWO_FISCAL_YEARS_DAYS, "FY", DOMESTIC_ANNUAL_FORMS)
        )
        annual = domestic_annual([point for symbol in members for point in (*cached[symbol], *own[symbol])])
        filed = [
            point.filed
            for symbol, count in counts.items()
            for point in (count, *year_end_counts(annual, symbol, today))
        ]
        raw, split = histories_since(ctx, sorted(counts), min([*filed, today - timedelta(days=MIN_HISTORY_DAYS)]))
        caps = market_caps(ctx, counts, raw, split)
        if not caps:
            ctx.log(f"no S&P 500 member with a share count and a close on {today}", level="warn")
            return None
        total_return = ctx.history(sorted(caps), "1d", momentum_sessions(ctx), "all")
        measures = {symbol: junk_measures(annual, symbol, today, raw[symbol], split[symbol]) for symbol in caps}
        for symbol, own_measures in measures.items():
            winner = momentum(total_return[symbol], today)
            if winner is not None:
                own_measures["momentum"] = -winner
        scores = junk_scores(measures)
        count = round(float(ctx.params["exclude"]) * len(scores))
        junk = highest_first(scores)[:count]
        kept = {symbol: cap for symbol, cap in caps.items() if symbol not in junk}
        total = sum(kept.values())
        targets = {symbol: cap / total for symbol, cap in kept.items()}
        reasons = {
            symbol: f"cap weight {weight:.3%}, "
            + (
                f"junk score {scores[symbol]:.2f} outside the worst {count} of {len(scores)}"
                if symbol in scores
                else "no fundamentals to score, kept at cap weight"
            )
            for symbol, weight in targets.items()
        }
        for symbol in junk:
            targets[symbol] = 0.0
            reasons[symbol] = f"junk score {scores[symbol]:.2f} in the worst {count} of {len(scores)}"
        ctx.log(f"{len(scores)} scored, {count} excluded as junk; {missing_caps(members, caps)}")
        return Book(targets, reasons, "no longer an S&P 500 member with a share count and a close")
