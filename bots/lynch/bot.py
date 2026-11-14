import math
from collections.abc import Callable, Mapping
from datetime import date, timedelta
from itertools import pairwise, zip_longest
from typing import Any, Literal, NamedTuple

import numpy as np
from buizz_bots.caps import market_caps
from buizz_bots.fundamentals import (
    DOMESTIC_ANNUAL_FORMS,
    DOMESTIC_FORMS,
    Annual,
    amount,
    amount_or_zero,
    by_symbol,
    consecutive_fiscal_ends,
    domestic_annual,
    histories_since,
    in_dollars,
    latest_shares,
    quarterly,
    total_debt,
    trailing_year,
)
from buizz_bots.insiders import Cluster, clusters
from buizz_bots.monthly import session_days
from buizz_bots.persona import (
    DAYS_PER_YEAR,
    Buy,
    Ladder,
    Sell,
    Trigger,
    held_stocks,
    ladder_weight,
    new_filings,
    restated,
    stock_weights,
    trade,
)
from buizz_bots.tickers import renamed
from buizz_bots.value import NET_DEBT_METRICS, net_debt, operating_companies

from buizz import Bot, Context, FundamentalPoint, History, Metric, run, split_ratio

Category = Literal["fast_grower", "stalwart", "slow_grower", "cyclical", "turnaround", "asset_play"]
CATEGORIES: tuple[Category, ...] = ("fast_grower", "stalwart", "slow_grower", "cyclical", "turnaround", "asset_play")
PRICE_EXITS: set[Category] = {"stalwart", "slow_grower"}
METRICS: list[Metric] = [
    "eps_diluted",
    "revenue",
    "dividends_paid",
    "equity",
    "goodwill",
    "shares_outstanding",
    *NET_DEBT_METRICS,
]
FILING_METRIC: Metric = "eps_diluted"
CHUNK = 200
YEAR_AGO_DAYS = 300
PERCENT = 100
IDLE_CASH = 0.01


class Profile(NamedTuple):
    category: Category | None
    fiscal_end: date
    eps: list[float]
    growth: float | None
    ttm: float | None
    prior_ttm: float | None
    price: float
    pe: float | None
    median_pe: float | None
    dividend_yield: float
    dividends: list[float]
    debt_equity: float
    net_cash: float
    tangible_book: float
    market_cap: float

    @property
    def pegy(self) -> float | None:
        if self.growth is None or self.pe is None:
            return None
        return (self.growth + self.dividend_yield) * PERCENT / self.pe

    @property
    def yearly_growth(self) -> float | None:
        if self.ttm is None or self.prior_ttm is None or self.prior_ttm <= 0:
            return None
        return self.ttm / self.prior_ttm - 1

    @property
    def peak_eps(self) -> float:
        return max(self.eps)


class Pick(NamedTuple):
    symbol: str
    category: Category
    pluses: int
    score: float
    reason: str


def cagr(first: float, last: float, years: int) -> float | None:
    return (last / first) ** (1 / years) - 1 if first > 0 and last > 0 else None


def close_by(history: History, day: date) -> float | None:
    index = int(np.searchsorted(session_days(history), np.datetime64(day), side="right")) - 1
    return float(history.close[index]) if index >= 0 else None


def per_share_today(points: list[FundamentalPoint], raw: History, adjusted: History) -> dict[date, float]:
    return {
        point.period_end: float(point.value) / ratio
        for point in points
        if point.period_type == "FY"
        and point.form in DOMESTIC_ANNUAL_FORMS
        and (ratio := split_ratio(raw, adjusted, point.filed)) is not None
    }


def trailing_pair(
    points: list[FundamentalPoint], raw: History, adjusted: History, today: date
) -> tuple[float | None, float | None]:
    quarters = quarterly(
        (point, float(point.value) / ratio)
        for point in points
        if point.period_start is not None and (ratio := split_ratio(raw, adjusted, point.filed)) is not None
    )
    if not quarters:
        return None, None
    year_ago = max(quarters) - timedelta(days=YEAR_AGO_DAYS)
    earlier = {end: value for end, value in quarters.items() if end <= year_ago}
    return trailing_year(quarters, today), trailing_year(earlier, year_ago)


def cyclical(params: Mapping[str, Any], revenue: list[float | None]) -> bool:
    known = [value for value in revenue if value is not None]
    if len(known) < len(revenue):
        return False
    falls = [later / earlier - 1 for earlier, later in pairwise(known) if earlier > 0 and later < earlier]
    return len(falls) >= int(params["cyclical_falls"]) and min(falls) <= -float(params["cyclical_drop"])


def grower(params: Mapping[str, Any], growth: float | None) -> Category | None:
    if growth is None or growth < 0:
        return None
    if growth >= float(params["fast_growth"]):
        return "fast_grower"
    return "stalwart" if growth >= float(params["stalwart_growth"]) else "slow_grower"


def category_of(
    params: Mapping[str, Any], eps: list[float], revenue: list[float | None], cheap_assets: bool
) -> Category | None:
    if cyclical(params, revenue):
        return "cyclical"
    if min(eps[-int(params["turnaround_years"]) :]) <= 0:
        return "turnaround"
    return "asset_play" if cheap_assets else grower(params, cagr(eps[0], eps[-1], len(eps) - 1))


def profile_of(
    params: Mapping[str, Any],
    annual: Annual,
    points: list[FundamentalPoint],
    histories: tuple[History, History],
    ends: list[date],
    market_cap: float,
    today: date,
) -> Profile | None:
    symbol = points[0].symbol
    raw, adjusted = histories
    eps_points = [point for point in points if point.metric == "eps_diluted"]
    yearly = per_share_today(eps_points, raw, adjusted)
    if any(end not in yearly for end in ends):
        return None
    eps = [yearly[end] for end in ends]
    latest = ends[-1]
    equity = amount(annual, symbol, "equity", latest)
    if equity is None:
        return None
    debt_equity = total_debt(annual, symbol, latest) / equity if equity > 0 else math.inf
    tangible_book = equity - amount_or_zero(annual, symbol, "goodwill", latest)
    revenue = [amount(annual, symbol, "revenue", end) for end in ends]
    dividends = [abs(amount_or_zero(annual, symbol, "dividends_paid", end)) for end in ends]
    ttm, prior_ttm = trailing_pair(eps_points, raw, adjusted, today)
    price = float(raw.close[-1])
    year_end_pes = [
        close / value for end, value in zip(ends, eps, strict=True) if value > 0 and (close := close_by(adjusted, end))
    ]
    return Profile(
        category=category_of(params, eps, revenue, tangible_book >= market_cap),
        fiscal_end=latest,
        eps=eps,
        growth=cagr(eps[0], eps[-1], len(eps) - 1),
        ttm=ttm,
        prior_ttm=prior_ttm,
        price=price,
        pe=price / ttm if ttm is not None and ttm > 0 else None,
        median_pe=float(np.median(year_end_pes)) if year_end_pes else None,
        dividend_yield=dividends[-1] / market_cap,
        dividends=dividends,
        debt_equity=debt_equity,
        net_cash=-net_debt(annual, symbol, latest),
        tangible_book=tangible_book,
        market_cap=market_cap,
    )


def chunk_profiles(ctx: Context, symbols: list[str]) -> dict[str, Profile]:
    today = ctx.session.date
    years = int(ctx.params["record_years"])
    points = sorted(
        (point for point in ctx.fundamentals(symbols, METRICS) if point.form in DOMESTIC_FORMS and in_dollars(point)),
        key=lambda point: point.filed,
    )
    annual = domestic_annual(points)
    grouped = by_symbol(points)
    records = {
        symbol: sorted(ends)
        for symbol in symbols
        if (ends := consecutive_fiscal_ends(annual.get((symbol, "eps_diluted"), {}), today, years)) is not None
    }
    if not records:
        return {}
    raw, adjusted = histories_since(ctx, sorted(records), min(ends[0] for ends in records.values()))
    counts = {symbol: count for symbol in records if (count := latest_shares(grouped[symbol], today)) is not None}
    caps = market_caps(ctx, counts, raw, adjusted)
    return {
        symbol: profile
        for symbol, cap in caps.items()
        if cap > 0
        and (
            profile := profile_of(
                ctx.params, annual, grouped[symbol], (raw[symbol], adjusted[symbol]), records[symbol], cap, today
            )
        )
        is not None
    }


def profiles_of(ctx: Context, symbols: list[str]) -> dict[str, Profile]:
    found: dict[str, Profile] = {}
    for offset in range(0, len(symbols), CHUNK):
        found.update(chunk_profiles(ctx, symbols[offset : offset + CHUNK]))
    return found


def described(profile: Profile) -> str:
    growth = "n/a" if profile.growth is None else f"{profile.growth:.1%}"
    pe = "n/a" if profile.pe is None else f"{profile.pe:.1f}"
    return f"5y EPS CAGR {growth}, P/E {pe}, FY {profile.fiscal_end}"


def pegy_text(profile: Profile, pegy: float) -> str:
    growth = profile.growth or 0.0
    return (
        f"PEGY {pegy:.2f} = (EPS growth {growth * PERCENT:.1f} + yield {profile.dividend_yield * PERCENT:.1f}) "
        f"/ P/E {profile.pe:.1f}"
    )


Entry = tuple[float, str]


def grower_entry(params: Mapping[str, Any], profile: Profile) -> Entry | None:
    pegy = profile.pegy
    if pegy is None or pegy < float(params["min_pegy"]) or (profile.growth or 0) > float(params["max_growth"]):
        return None
    if profile.category == "slow_grower" and min(profile.dividends) <= 0:
        return None
    return pegy, pegy_text(profile, pegy)


def peak_yield(profile: Profile) -> float | None:
    return profile.peak_eps / profile.price if profile.peak_eps > 0 else None


def cyclical_entry(params: Mapping[str, Any], profile: Profile) -> Entry | None:
    ttm, prior, peak = profile.ttm, profile.prior_ttm, profile.peak_eps
    score = peak_yield(profile)
    if ttm is None or prior is None or score is None or ttm <= prior or ttm > float(params["cyclical_trough"]) * peak:
        return None
    return (
        score,
        f"trailing EPS {ttm:.2f} up from {prior:.2f} a year ago, {ttm / peak:.0%} of the record's peak {peak:.2f}",
    )


def fiscal_eps(params: Mapping[str, Any], profile: Profile) -> str:
    return ", ".join(f"{value:.2f}" for value in profile.eps[-int(params["turnaround_years"]) :])


def turnaround_entry(params: Mapping[str, Any], profile: Profile) -> Entry | None:
    score = peak_yield(profile)
    if profile.ttm is None or profile.ttm <= 0 or score is None:
        return None
    return score, f"a loss in FY EPS {fiscal_eps(params, profile)}, trailing EPS back to {profile.ttm:.2f}"


def asset_entry(params: Mapping[str, Any], profile: Profile) -> Entry | None:
    discount = profile.market_cap / profile.tangible_book
    if discount > float(params["asset_entry"]):
        return None
    return 1 / discount, f"market cap {discount:.0%} of tangible book ${profile.tangible_book / 1e6:,.0f}M"


ENTRIES: dict[Category, Callable[[Mapping[str, Any], Profile], Entry | None]] = {
    "fast_grower": grower_entry,
    "stalwart": grower_entry,
    "slow_grower": grower_entry,
    "cyclical": cyclical_entry,
    "turnaround": turnaround_entry,
    "asset_play": asset_entry,
}


def entry(params: Mapping[str, Any], profile: Profile) -> Entry | None:
    category = profile.category
    if category is None or profile.debt_equity > float(params["max_debt_equity"]):
        return None
    return ENTRIES[category](params, profile)


def price_exit(params: Mapping[str, Any], category: Category, profile: Profile | None, gain: float) -> str | None:
    if gain >= float(params["exit_gain"]):
        return f"{category} up {gain:.0%} on cost: take the 30-50% and repeat"
    pe, normal = (None, None) if profile is None else (profile.pe, profile.median_pe)
    if pe is None or normal is None or gain < float(params["stretched_gain"]) or pe <= normal:
        return None
    return f"{category} up {gain:.0%} on cost with P/E {pe:.1f} above its record's {normal:.1f}"


Exit = tuple[Trigger, str]


def fast_exit(params: Mapping[str, Any], profile: Profile) -> Exit | None:
    growth = profile.yearly_growth
    if growth is None or growth >= float(params["stalwart_growth"]):
        return None
    return "thesis", f"fast growth ended: trailing EPS {growth:+.1%} on a year ago, {described(profile)}"


def stalwart_exit(_: Mapping[str, Any], profile: Profile) -> Exit | None:
    growth = profile.yearly_growth
    if growth is None or growth >= 0:
        return None
    return "thesis", f"earnings shrinking: trailing EPS {growth:+.1%} on a year ago, {described(profile)}"


def slow_exit(params: Mapping[str, Any], profile: Profile) -> Exit | None:
    before, now = profile.dividends[-2], profile.dividends[-1]
    if now >= before * (1 - float(params["dividend_cut"])):
        return None
    return "thesis", f"dividend cut: ${now / 1e6:,.1f}M paid in FY {profile.fiscal_end} after ${before / 1e6:,.1f}M"


def cyclical_exit(_: Mapping[str, Any], profile: Profile) -> Exit | None:
    ttm, pe, normal = profile.ttm, profile.pe, profile.median_pe
    if ttm is None or pe is None or normal is None or ttm < profile.peak_eps or pe > normal:
        return None
    return (
        "valuation",
        f"cycle top: trailing EPS {ttm:.2f} at or above the record's peak {profile.peak_eps:.2f} "
        f"with P/E {pe:.1f} at or below its record's {normal:.1f}",
    )


def turnaround_exit(params: Mapping[str, Any], profile: Profile) -> Exit | None:
    if profile.ttm is not None and profile.ttm <= 0:
        return "thesis", f"the comeback failed: trailing EPS {profile.ttm:.2f}"
    if min(profile.eps[-int(params["turnaround_years"]) :]) <= 0:
        return None
    return "valuation", f"turned: FY EPS {fiscal_eps(params, profile)}, all profits, and everybody knows it"


def asset_exit(_: Mapping[str, Any], profile: Profile) -> Exit | None:
    if profile.market_cap < profile.tangible_book:
        return None
    return (
        "valuation",
        f"assets priced in: market cap ${profile.market_cap / 1e6:,.0f}M at least tangible book "
        f"${profile.tangible_book / 1e6:,.0f}M",
    )


EXITS: dict[Category, Callable[[Mapping[str, Any], Profile], Exit | None]] = {
    "fast_grower": fast_exit,
    "stalwart": stalwart_exit,
    "slow_grower": slow_exit,
    "cyclical": cyclical_exit,
    "turnaround": turnaround_exit,
    "asset_play": asset_exit,
}


def exit_of(
    params: Mapping[str, Any], category: Category | None, profile: Profile | None, symbol: str, gain: float
) -> Sell | None:
    if category is None:
        return None
    if category in PRICE_EXITS and (reason := price_exit(params, category, profile, gain)) is not None:
        return Sell(symbol, "price", reason)
    if profile is None:
        return None
    if profile.debt_equity > float(params["break_debt_equity"]):
        return Sell(symbol, "thesis", f"debt-to-equity {profile.debt_equity:.2f} in FY {profile.fiscal_end}")
    pegy = profile.pegy
    if category in {"fast_grower", "stalwart"} and pegy is not None and pegy <= float(params["sell_pegy"]):
        return Sell(symbol, "valuation", f"{pegy_text(profile, pegy)}: P/E twice the growth rate")
    found = EXITS[category](params, profile)
    return None if found is None else Sell(symbol, *found)


def opportunistic_buyers(ctx: Context, symbols: list[str]) -> dict[str, Cluster]:
    if not symbols:
        return {}
    since = ctx.session.date - timedelta(days=int(ctx.params["insider_days"]))
    years = int(ctx.params["routine_years"])
    trades = ctx.insider_trades(symbols, since)
    bought = {trade.symbol for trade in trades if trade.code == "P" and trade.acquired}
    if not bought:
        return {}
    history = ctx.insider_trades(sorted(bought), date(since.year - years, 1, 1))
    found = clusters([trade for trade in trades if trade.symbol in bought], history, since, years, ctx.session.open)
    return {cluster.symbol: cluster for cluster in found}


def picks_of(ctx: Context, profiles: dict[str, Profile], skip: set[str]) -> list[Pick]:
    universe = set(ctx.universe)
    passed: dict[str, tuple[Category, float, str]] = {}
    for symbol, profile in sorted(profiles.items()):
        if symbol not in universe or symbol in skip or profile.category is None:
            continue
        found = entry(ctx.params, profile)
        if found is not None:
            passed[symbol] = (profile.category, *found)
    since = ctx.session.date - timedelta(days=round(float(ctx.params["restatement_years"]) * DAYS_PER_YEAR))
    unreliable = restated(ctx, passed, since)
    eligible = sorted(set(passed) - unreliable)
    insiders = opportunistic_buyers(ctx, eligible)
    picks: list[Pick] = []
    for symbol in eligible:
        category, score, why = passed[symbol]
        pluses: list[str] = []
        if symbol in insiders:
            buyers = insiders[symbol]
            pluses.append(
                f"opportunistic insider buying ${buyers.value:,.0f} by {', '.join(sorted(buyers.insiders.values()))}"
            )
        if profiles[symbol].net_cash > 0:
            pluses.append(f"net cash ${profiles[symbol].net_cash / 1e6:,.0f}M")
        detail = f"{category}: {why}; {described(profiles[symbol])}"
        picks.append(Pick(symbol, category, len(pluses), score, "; ".join([detail, *pluses])))
    return picks


def interleaved(picks: list[Pick]) -> list[Pick]:
    ranked = sorted(picks, key=lambda pick: (-pick.pluses, -pick.score, pick.symbol))
    queues = [[pick for pick in ranked if pick.category == category] for category in CATEGORIES]
    return [pick for row in zip_longest(*queues) for pick in row if pick is not None]


def gain_of(ctx: Context, symbol: str) -> float:
    cost = float(ctx.position(symbol).avg_cost)
    return ctx.bars[symbol].close / cost - 1 if cost > 0 and symbol in ctx.bars else 0.0


class Lynch(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        cash_symbol = str(ctx.params["cash_symbol"])
        held = held_stocks(ctx, cash_symbol)
        categories: dict[str, Category] = {
            symbol: category
            for symbol, category in renamed(ctx, ctx.state.get("categories", {})).items()
            if symbol in held
        }
        filings = renamed(ctx, ctx.state.get("filings", {}))
        seen = {symbol: filings[symbol] for symbol in held if symbol in filings}
        fresh = new_filings(ctx, held, seen, FILING_METRIC)
        broken = restated(ctx, held, date.fromisoformat(ctx.state.get("checked", today.isoformat())))
        ctx.state["checked"] = today.isoformat()
        month = f"{today:%Y-%m}"
        scanning = ctx.state.get("scanned") != month
        ctx.state["scanned"] = month
        wanted = sorted(set(operating_companies(ctx, sorted(ctx.universe))) | set(held)) if scanning else fresh
        profiles = profiles_of(ctx, wanted) if wanted else {}
        for symbol in held:
            found = profiles.get(symbol)
            if symbol not in categories and found is not None and found.category is not None:
                categories[symbol] = found.category
        sells = [
            Sell(symbol, "thesis", "8-K Item 4.02, past financial statements no longer to be relied on")
            for symbol in sorted(broken)
        ]
        sells += [
            sell
            for symbol in held
            if symbol not in broken
            and (
                sell := exit_of(ctx.params, categories.get(symbol), profiles.get(symbol), symbol, gain_of(ctx, symbol))
            )
        ]
        sold = {sell.symbol for sell in sells}
        weights = {symbol: weight for symbol, weight in stock_weights(ctx, cash_symbol).items() if symbol not in sold}
        picks = interleaved(picks_of(ctx, profiles, set(held) | sold)) if scanning else []
        buys = self.adds(ctx, profiles, categories, weights) + self.opens(ctx, picks, len(weights))
        if sells or buys or float(ctx.portfolio.cash) > IDLE_CASH * float(ctx.portfolio.equity):
            trade(ctx, sells, buys, cash_symbol)
        bought = {pick.symbol: pick.category for pick in picks if any(buy.symbol == pick.symbol for buy in buys)}
        new_filings(ctx, [symbol for symbol in bought if symbol not in seen], seen, FILING_METRIC)
        ctx.state["filings"] = {symbol: filed for symbol, filed in seen.items() if symbol not in sold}
        ctx.state["categories"] = {
            symbol: category for symbol, category in (categories | bought).items() if symbol not in sold
        }

    def adds(
        self, ctx: Context, profiles: dict[str, Profile], categories: dict[str, Category], weights: dict[str, float]
    ) -> list[Buy]:
        ladder = Ladder(*(float(ctx.params[name]) for name in Ladder._fields))
        found: list[Buy] = []
        for symbol, weight in sorted(weights.items()):
            profile = profiles.get(symbol)
            target = ladder_weight(ctx, ladder, symbol, weight)
            if target is None or profile is None or profile.category != categories.get(symbol):
                continue
            if entry(ctx.params, profile) is None:
                continue
            found.append(
                Buy(
                    symbol,
                    target,
                    f"add on a drop: {-gain_of(ctx, symbol):.0%} below cost and still a {profile.category} "
                    f"to buy, {weight:.1%} to {target:.1%} of equity",
                )
            )
        return found

    def opens(self, ctx: Context, picks: list[Pick], holding: int) -> list[Buy]:
        slots = int(ctx.params["max_names"]) - holding
        start = float(ctx.params["start"])
        return [Buy(pick.symbol, start, pick.reason) for pick in picks[: max(slots, 0)]]


if __name__ == "__main__":
    run(Lynch)
