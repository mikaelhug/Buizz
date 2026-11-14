from datetime import date, timedelta
from typing import NamedTuple

import numpy as np
from buizz_bots.caps import market_caps
from buizz_bots.fundamentals import (
    DOMESTIC_ANNUAL_FORMS,
    MAX_GAP_DAYS,
    MAX_STALENESS_DAYS,
    Annual,
    PointCache,
    adjusted_shares,
    amount,
    amount_or_zero,
    by_symbol,
    consecutive_fiscal_ends,
    domestic_annual,
    gross_margin,
    histories_since,
    latest_filing,
    latest_shares,
    recent,
    refreshed,
    year_end_shares,
)
from buizz_bots.insiders import sessions_since
from buizz_bots.sessions import last_session_of_month

from buizz import Bot, Context, History, Metric, run

METRICS: list[Metric] = [
    "net_income",
    "operating_cash_flow",
    "total_assets",
    "long_term_debt",
    "current_assets",
    "current_liabilities",
    "revenue",
    "gross_profit",
    "cost_of_revenue",
    "equity",
    "preferred_stock",
    "shares_outstanding",
]
YEARS = 3
KEEP_DAYS = MAX_STALENESS_DAYS + (YEARS - 1) * MAX_GAP_DAYS
FIRST_FORMATION_DAYS = 365
SIDES = {1: "long", -1: "short"}
FORMED = "formed"


class Score(NamedTuple):
    symbol: str
    filed: date
    fiscal_end: date
    book: float
    signals: dict[str, bool]

    @property
    def f(self) -> int:
        return sum(self.signals.values())


def ratio(numerator: float | None, denominator: float | None) -> float | None:
    if numerator is None or denominator is None or denominator <= 0:
        return None
    return numerator / denominator


def leverage(annual: Annual, symbol: str, end: date, start_assets: float) -> float | None:
    assets = amount(annual, symbol, "total_assets", end)
    if assets is None:
        return None
    return ratio(amount_or_zero(annual, symbol, "long_term_debt", end), (assets + start_assets) / 2)


def score(annual: Annual, symbol: str, today: date, raw: History, split: History) -> Score | None:
    ends = consecutive_fiscal_ends(annual.get((symbol, "total_assets"), {}), today, YEARS)
    if ends is None:
        return None
    latest, prior, before = ends
    assets = {end: amount(annual, symbol, "total_assets", end) or 0.0 for end in ends}

    def per_asset(metric: Metric, end: date, start: date) -> float | None:
        return ratio(amount(annual, symbol, metric, end), assets[start])

    roa, roa_prior = per_asset("net_income", latest, prior), per_asset("net_income", prior, before)
    cfo = per_asset("operating_cash_flow", latest, prior)
    lever, lever_prior = (
        leverage(annual, symbol, latest, assets[prior]),
        leverage(annual, symbol, prior, assets[before]),
    )
    liquid = ratio(
        amount(annual, symbol, "current_assets", latest), amount(annual, symbol, "current_liabilities", latest)
    )
    liquid_prior = ratio(
        amount(annual, symbol, "current_assets", prior), amount(annual, symbol, "current_liabilities", prior)
    )
    margin, margin_prior = gross_margin(annual, symbol, latest), gross_margin(annual, symbol, prior)
    turn, turn_prior = per_asset("revenue", latest, prior), per_asset("revenue", prior, before)
    counts = [year_end_shares(annual, symbol, end) for end in (latest, prior)]
    shares = [None if count is None else adjusted_shares(count, raw, split) for count in counts]
    equity = amount(annual, symbol, "equity", latest)
    pairs = {
        "roa": (roa, 0.0),
        "cfo": (cfo, 0.0),
        "delta_roa": (roa, roa_prior),
        "accrual": (cfo, roa),
        "delta_lever": (lever_prior, lever),
        "delta_liquid": (liquid, liquid_prior),
        "eq_offer": (shares[0], shares[1]),
        "delta_margin": (margin, margin_prior),
        "delta_turn": (turn, turn_prior),
    }
    known = {
        name: (first, second) for name, (first, second) in pairs.items() if first is not None and second is not None
    }
    if equity is None or len(known) < len(pairs):
        return None
    signals = {name: first > second for name, (first, second) in known.items()}
    signals["eq_offer"] = not signals["eq_offer"]
    book = equity - amount_or_zero(annual, symbol, "preferred_stock", latest)
    filed = annual[symbol, "total_assets"][latest].filed
    return Score(symbol, filed, latest, book, signals)


class PiotroskiFscore(Bot):
    def __init__(self) -> None:
        self.annual: PointCache = {}

    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        held: dict[str, list[str | int | float]] = ctx.state.setdefault("held", {})
        first = FORMED not in ctx.state
        if not first and not last_session_of_month(ctx):
            return
        since = today - timedelta(days=FIRST_FORMATION_DAYS) if first else date.fromisoformat(ctx.state[FORMED])
        ctx.state[FORMED] = today.isoformat()
        hold = int(ctx.params["hold_sessions"])
        held_for = sessions_since(ctx, {symbol: date.fromisoformat(str(entry[1])) for symbol, entry in held.items()})
        for symbol in [symbol for symbol, sessions in held_for.items() if sessions >= hold]:
            del held[symbol]
        self.form(ctx, held, since)
        self.trade(ctx, held)

    def form(self, ctx: Context, held: dict[str, list[str | int | float]], since: date) -> None:
        today = ctx.session.date
        members = sorted(ctx.universe)
        own = by_symbol(ctx.fundamentals(members, ["shares_outstanding"]))
        counts = {symbol: count for symbol in members if (count := latest_shares(own[symbol], today)) is not None}
        filings = {symbol: latest_filing(own[symbol], DOMESTIC_ANNUAL_FORMS) for symbol in counts}
        self.annual = {symbol: self.annual[symbol] for symbol in filings if symbol in self.annual}
        points = refreshed(ctx, METRICS, filings, self.annual, recent(today, KEEP_DAYS, "FY", DOMESTIC_ANNUAL_FORMS))
        annual = domestic_annual([point for symbol in sorted(counts) for point in points[symbol]])
        oldest = min([*(count.filed for count in counts.values()), today - timedelta(days=2 * MAX_GAP_DAYS)])
        raw, split = histories_since(ctx, sorted(counts), oldest)
        caps = market_caps(ctx, counts, raw, split)
        scores = {
            symbol: found
            for symbol in sorted(caps)
            if (found := score(annual, symbol, today, raw[symbol], split[symbol])) is not None
        }
        book_to_market = {symbol: found.book / caps[symbol] for symbol, found in scores.items()}
        if not book_to_market:
            ctx.log(f"no member with three fiscal years of F-score inputs and a market cap on {today}", level="warn")
            return
        cutoff = float(np.quantile(list(book_to_market.values()), 1 - float(ctx.params["bm_quintile"])))
        long_min, short_max = int(ctx.params["long_min"]), int(ctx.params["short_max"])
        formed = 0
        for symbol, found in scores.items():
            if not since < found.filed <= today:
                continue
            formed += 1
            value = book_to_market[symbol] >= cutoff
            side = 1 if value and found.f >= long_min else -1 if value and found.f <= short_max else 0
            if side:
                held[symbol] = [side, found.filed.isoformat(), found.f, round(book_to_market[symbol], 4)]
            elif symbol in held:
                del held[symbol]
        sides = [entry[0] for entry in held.values()]
        ctx.log(
            f"{len(scores)} members scored, B/M cutoff {cutoff:.3f}, {formed} with a 10-K filed since {since}, "
            f"{sides.count(1)} long, {sides.count(-1)} short"
        )

    def trade(self, ctx: Context, held: dict[str, list[str | int | float]]) -> None:
        cap = float(ctx.params["max_weight"])
        legs = {side: [symbol for symbol, entry in held.items() if entry[0] == side] for side in SIDES}
        targets: dict[str, float] = {}
        reasons: dict[str, str] = {}
        for side, symbols in legs.items():
            for symbol in symbols:
                _, filed, f, book_to_market = held[symbol]
                targets[symbol] = side * min(cap, 1 / len(symbols))
                reasons[symbol] = f"F={f}, BM quintile 1 (B/M {float(book_to_market):.2f}), 10-K filed {filed}"
        ctx.rebalance(
            targets,
            reason=reasons,
            exit_reason=f"held {ctx.params['hold_sessions']} sessions since its 10-K, or no longer qualifies",
            band=float(ctx.params["drift_band"]),
        )


if __name__ == "__main__":
    run(PiotroskiFscore)
