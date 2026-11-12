import math
from datetime import date, timedelta

from buizz_bots.caps import MIN_HISTORY_DAYS, market_caps
from buizz_bots.drift import rebalance_within_band
from buizz_bots.fundamentals import (
    DOMESTIC_ANNUAL_FORMS,
    TWO_FISCAL_YEARS_DAYS,
    Annual,
    PointCache,
    adjusted_shares,
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
    year_end_shares,
)
from buizz_bots.monthly import momentum, momentum_sessions
from buizz_bots.ranks import highest_first, percentiles
from buizz_bots.sessions import REBALANCED, first_step_or_month_end

from buizz import Bot, Context, FundamentalPoint, History, Metric, run

ANNUAL_METRICS: list[Metric] = [
    "gross_profit",
    "revenue",
    "cost_of_revenue",
    "total_assets",
    "net_income",
    "operating_cash_flow",
]
SCHEDULES = {"monthly": range(1, 13), "quarterly": (3, 6, 9, 12)}
FUNDAMENTAL = ("profitability", "accruals", "issuance")
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
    if income is not None and cash is not None:
        measures["accruals"] = (income - cash) / assets
    growth = share_growth(annual, symbol, today, raw, split)
    if growth is not None:
        measures["issuance"] = growth
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


class SpyExJunk(Bot):
    def __init__(self) -> None:
        self.annual: PointCache = {}

    def on_step(self, ctx: Context) -> None:
        if not first_step_or_month_end(ctx, SCHEDULES[ctx.params["rebalance"]]):
            return
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
            return
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
        ctx.log(f"{len(caps)} members with a market cap, {len(scores)} scored, {count} excluded as junk")
        rebalance_within_band(
            ctx,
            targets,
            float(ctx.params["drift_band"]),
            reason=reasons,
            exit_reason="no longer an S&P 500 member with a share count and a close",
        )


if __name__ == "__main__":
    run(SpyExJunk)
