from collections import defaultdict
from collections.abc import Mapping
from datetime import date, timedelta

import numpy as np
from buizz_bots.beta import ols_beta
from buizz_bots.caps import market_caps
from buizz_bots.fundamentals import (
    DOMESTIC_ANNUAL_FORMS,
    TWO_FISCAL_YEARS_DAYS,
    Annual,
    PointCache,
    amount,
    amount_or_zero,
    by_symbol,
    consecutive_fiscal_ends,
    domestic_annual,
    histories_since,
    latest_filing,
    latest_shares,
    recent,
    refreshed,
)
from buizz_bots.industries import fama_french_49
from buizz_bots.monthly import momentum
from buizz_bots.quality_momentum import profitability
from buizz_bots.ranks import percentiles, rank_z
from buizz_bots.sessions import REBALANCED, first_step_or_month_end, week_ends
from buizz_bots.value import NET_DEBT_METRICS, net_debt
from buizz_bots.weekly import completed_week_closes

from buizz import Bot, Context, History, Metric, run

METRICS: list[Metric] = [
    "operating_income",
    "depreciation_amortization",
    *NET_DEBT_METRICS,
    "equity",
    "gross_profit",
    "revenue",
    "cost_of_revenue",
    "total_assets",
    "net_income",
    "operating_cash_flow",
]
STYLES: dict[str, tuple[str, ...]] = {
    "value": ("ebitda_ev", "book_to_market"),
    "momentum": ("momentum",),
    "quality": ("gross_profitability", "accruals"),
    "low_risk": ("low_volatility",),
}
VOL_WEEKS = 156
SESSIONS_PER_WEEK = 5
HISTORY_BARS = SESSIONS_PER_WEEK * (VOL_WEEKS + 4)
POOLED = "pooled"

Measures = dict[str, dict[str, float]]


def ebitda_to_ev(annual: Annual, symbol: str, end: date, cap: float) -> float | None:
    income = amount(annual, symbol, "operating_income", end)
    depreciation = amount(annual, symbol, "depreciation_amortization", end)
    if income is None or depreciation is None:
        return None
    value = cap + net_debt(annual, symbol, end)
    return None if value <= 0 else (income + depreciation) / value


def accruals(annual: Annual, symbol: str, ends: list[date]) -> float | None:
    income = amount(annual, symbol, "net_income", ends[0])
    cash = amount(annual, symbol, "operating_cash_flow", ends[0])
    assets = [amount(annual, symbol, "total_assets", end) for end in ends]
    if income is None or cash is None or None in assets:
        return None
    average = sum(value or 0.0 for value in assets) / 2
    return None if average <= 0 else -(income - cash) / average


def weekly_volatility(history: History, today: date, week_ends_today: bool) -> float | None:
    returns = np.diff(np.log(completed_week_closes(history, today, week_ends_today)))[-VOL_WEEKS:]
    return float(np.std(returns, ddof=1)) if returns.size == VOL_WEEKS else None


def industry_z(values: Mapping[str, float], industries: Mapping[str, str], minimum: int) -> dict[str, float]:
    groups: dict[str, dict[str, float]] = defaultdict(dict)
    for symbol, value in values.items():
        groups[industries[symbol]][symbol] = value
    pooled = {symbol: value for group in groups.values() if len(group) < minimum for symbol, value in group.items()}
    scored: dict[str, float] = {}
    for group in [*(group for group in groups.values() if len(group) >= minimum), pooled]:
        scored |= rank_z(group)
    return scored


def style_scores(measures: Measures, industries: Mapping[str, str], minimum: int) -> dict[str, dict[str, float]]:
    z = {name: industry_z(values, industries, minimum) for name, values in measures.items()}
    styles: dict[str, dict[str, float]] = {}
    for style, names in STYLES.items():
        symbols = {symbol for name in names for symbol in z[name]}
        styles[style] = {
            symbol: float(np.mean([z[name][symbol] for name in names if symbol in z[name]])) for symbol in symbols
        }
    return styles


def legs(
    score: Mapping[str, float],
    quantile: float,
    buffer: float,
    held: Mapping[str, int],
    caps: Mapping[str, float],
    value: bool,
) -> dict[str, float]:
    rank = percentiles(score)
    long = {
        symbol
        for symbol, pct in rank.items()
        if pct > 1 - quantile or (held.get(symbol) == 1 and pct > 1 - quantile - buffer)
    }
    short = {
        symbol
        for symbol, pct in rank.items()
        if pct <= quantile or (held.get(symbol) == -1 and pct <= quantile + buffer)
    }
    weights: dict[str, float] = {}
    for side, members in ((1, long), (-1, short)):
        size = {symbol: caps[symbol] if value else 1.0 for symbol in members}
        total = sum(size.values())
        weights |= {symbol: side * size[symbol] / total for symbol in members}
    return weights


class MultiFactorComposite(Bot):
    def __init__(self) -> None:
        self.annual: PointCache = {}

    def on_step(self, ctx: Context) -> None:
        if not first_step_or_month_end(ctx, range(1, 13)):
            return
        today = ctx.session.date
        ctx.state[REBALANCED] = today.isoformat()
        hedge: str = ctx.params["hedge"]
        members = sorted(ctx.universe)
        own = by_symbol(ctx.fundamentals(members, ["shares_outstanding"]))
        counts = {symbol: count for symbol in members if (count := latest_shares(own[symbol], today)) is not None}
        filings = {symbol: latest_filing(own[symbol], DOMESTIC_ANNUAL_FORMS) for symbol in counts}
        self.annual = {symbol: self.annual[symbol] for symbol in filings if symbol in self.annual}
        points = refreshed(
            ctx, METRICS, filings, self.annual, recent(today, TWO_FISCAL_YEARS_DAYS, "FY", DOMESTIC_ANNUAL_FORMS)
        )
        annual = domestic_annual([point for symbol in sorted(counts) for point in points[symbol]])
        oldest = min([*(count.filed for count in counts.values()), today - timedelta(days=40)])
        raw, split = histories_since(ctx, sorted(counts), oldest)
        caps = market_caps(ctx, counts, raw, split)
        symbols = sorted(caps)
        total = ctx.history([*symbols, hedge], "1d", HISTORY_BARS, "all")
        measures = self.measures(ctx, annual, caps, total)
        profiles = ctx.instruments(symbols)
        industries = {
            symbol: fama_french_49(profiles[symbol].sic if symbol in profiles else None) or POOLED for symbol in symbols
        }
        styles = style_scores(measures, industries, int(ctx.params["min_industry"]))
        weights: dict[str, float] = ctx.params["styles"]
        scored = [symbol for symbol in symbols if all(symbol in styles[style] for style in STYLES)]
        if len(scored) < 1 / float(ctx.params["quantile"]):
            ctx.log(f"only {len(scored)} members with all four styles on {today}", level="warn")
            return
        held = {
            symbol: 1 if position.qty > 0 else -1
            for symbol, position in ctx.portfolio.positions.items()
            if position.qty != 0 and symbol != hedge
        }
        quantile, buffer = float(ctx.params["quantile"]), float(ctx.params["buffer"])
        value_weighted = ctx.params["weighting"] == "value"
        composite = {symbol: sum(weights[style] * styles[style][symbol] for style in STYLES) for symbol in scored}
        if ctx.params["construction"] == "mix":
            book: dict[str, float] = defaultdict(float)
            for style in STYLES:
                sleeve = legs({s: styles[style][s] for s in scored}, quantile, buffer, held, caps, value_weighted)
                for symbol, weight in sleeve.items():
                    book[symbol] += weights[style] * weight
        else:
            book = defaultdict(float, legs(composite, quantile, buffer, held, caps, value_weighted))
        betas = {
            symbol: beta
            for symbol in book
            if (beta := ols_beta(total[symbol], total[hedge], int(ctx.params["beta_days"]))) is not None
        }
        book_beta = sum(weight * betas.get(symbol, 1.0) for symbol, weight in book.items())
        targets = {symbol: weight for symbol, weight in book.items() if weight != 0}
        targets[hedge] = -book_beta
        reasons = {
            symbol: f"composite z {composite[symbol]:+.2f}: "
            + ", ".join(f"{style} {styles[style][symbol]:+.2f}" for style in STYLES)
            + f", beta {betas.get(symbol, 1.0):.2f}"
            for symbol in targets
            if symbol in composite
        }
        reasons[hedge] = f"hedges the stock book's beta of {book_beta:+.2f} over {ctx.params['beta_days']} sessions"
        longs, shorts = sum(w > 0 for w in book.values()), sum(w < 0 for w in book.values())
        ctx.log(f"{len(symbols)} members with a market cap, {len(scored)} scored, {longs} long, {shorts} short")
        ctx.rebalance(
            targets,
            reason=reasons,
            exit_reason="left its quintile of the composite score",
            band=float(ctx.params["drift_band"]),
        )

    def measures(
        self, ctx: Context, annual: Annual, caps: Mapping[str, float], total: Mapping[str, History]
    ) -> Measures:
        today = ctx.session.date
        week_ends_today = week_ends(ctx)
        found: Measures = {name: {} for names in STYLES.values() for name in names}
        for symbol, cap in caps.items():
            ends = consecutive_fiscal_ends(annual.get((symbol, "total_assets"), {}), today, 2)
            history = total[symbol]
            values = {
                "momentum": momentum(history, today),
                "low_volatility": None if (vol := weekly_volatility(history, today, week_ends_today)) is None else -vol,
            }
            if ends is not None:
                equity = amount(annual, symbol, "equity", ends[0])
                book = None if equity is None else equity - amount_or_zero(annual, symbol, "preferred_stock", ends[0])
                values |= {
                    "ebitda_ev": ebitda_to_ev(annual, symbol, ends[0], cap),
                    "book_to_market": None if book is None else book / cap,
                    "gross_profitability": profitability(annual, symbol, today),
                    "accruals": accruals(annual, symbol, ends),
                }
            for name, value in values.items():
                if value is not None:
                    found[name][symbol] = value
        return found


if __name__ == "__main__":
    run(MultiFactorComposite)
