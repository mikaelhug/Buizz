from collections import defaultdict
from datetime import date, timedelta
from itertools import pairwise
from typing import Any, NamedTuple

from buizz_bots.fundamentals import DOMESTIC_FORMS, in_dollars, shares_after, with_fourth_quarters
from buizz_bots.sessions import last_session_of_month

from buizz import Bot, Context, FundamentalPoint, History, Metric, run, split_ratio

NET_NET_METRICS: list[Metric] = [
    "current_assets",
    "total_liabilities",
    "total_assets",
    "equity",
    "minority_interest",
    "preferred_stock",
    "shares_outstanding",
]
RULE_1976_METRICS: list[Metric] = ["total_assets", "equity", "eps_diluted"]
MAX_QUARTER_AGE_DAYS = 200
HISTORY_BARS = 520
AAA_LOOKBACK_DAYS = 100
QUARTER_GAP_DAYS = range(80, 101)
QUARTERS_PER_YEAR = 4
DECEMBER = 12
NOVEMBER = 11

type Facts = dict[Metric, list[FundamentalPoint]]


class Pick(NamedTuple):
    symbol: str
    rank: float
    reason: str


def domestic_facts(points: list[FundamentalPoint]) -> dict[str, Facts]:
    facts: defaultdict[str, Facts] = defaultdict(dict)
    for point in sorted(points, key=lambda point: point.filed):
        if point.form in DOMESTIC_FORMS and in_dollars(point):
            facts[point.symbol].setdefault(point.metric, []).append(point)
    return facts


def instants(facts: Facts, metric: Metric) -> dict[date, float]:
    return {point.period_end: float(point.value) for point in facts.get(metric, [])}


def latest_balance_sheet(facts: Facts, metric: Metric, today: date) -> date | None:
    days = instants(facts, metric)
    if not days or (today - max(days)).days > MAX_QUARTER_AGE_DAYS:
        return None
    return max(days)


def total_liabilities(facts: Facts, day: date) -> float | None:
    reported = instants(facts, "total_liabilities").get(day)
    if reported is not None:
        return reported
    assets = instants(facts, "total_assets").get(day)
    equity = instants(facts, "equity").get(day)
    if assets is None or equity is None:
        return None
    return assets - equity - instants(facts, "minority_interest").get(day, 0.0)


def preferred_stock(facts: Facts, day: date) -> float:
    reported = {end: value for end, value in instants(facts, "preferred_stock").items() if end <= day}
    return reported[max(reported)] if reported else 0.0


def net_current_asset_value(facts: Facts, today: date) -> tuple[date, float] | None:
    day = latest_balance_sheet(facts, "current_assets", today)
    if day is None:
        return None
    liabilities = total_liabilities(facts, day)
    if liabilities is None:
        return None
    return day, instants(facts, "current_assets")[day] - liabilities - preferred_stock(facts, day)


def trailing_eps(points: list[FundamentalPoint], raw: History, adjusted: History, today: date) -> float | None:
    reported: dict[date, float] = {}
    years: list[tuple[date, date, float]] = []
    for point in points:
        ratio = split_ratio(raw, adjusted, point.filed)
        if ratio is None or point.period_start is None:
            continue
        per_share_today = float(point.value) / ratio
        if point.period_type == "Q":
            reported[point.period_end] = per_share_today
        else:
            years.append((point.period_start, point.period_end, per_share_today))
    quarters = with_fourth_quarters(reported, years)
    ends = sorted(quarters, reverse=True)[:QUARTERS_PER_YEAR]
    if len(ends) < QUARTERS_PER_YEAR or (today - ends[0]).days > MAX_QUARTER_AGE_DAYS:
        return None
    if any((later - earlier).days not in QUARTER_GAP_DAYS for later, earlier in pairwise(ends)):
        return None
    return sum(quarters[end] for end in ends)


def split_histories(ctx: Context, symbols: list[str]) -> tuple[dict[str, History], dict[str, History]]:
    return ctx.history(symbols, "1d", HISTORY_BARS, "raw"), ctx.history(symbols, "1d", HISTORY_BARS, "split")


def screened_symbols(ctx: Context) -> list[str]:
    return [symbol for symbol in ctx.universe if symbol in ctx.bars]


def net_nets(ctx: Context) -> list[Pick]:
    today = ctx.session.date
    limit = float(ctx.params["net_net_ratio"])
    candidates: dict[str, tuple[date, float, FundamentalPoint]] = {}
    for symbol, facts in domestic_facts(ctx.fundamentals(screened_symbols(ctx), NET_NET_METRICS)).items():
        ncav = net_current_asset_value(facts, today)
        if ncav is None or ncav[1] <= 0:
            continue
        counts = {
            point.period_end: point
            for point in facts.get("shares_outstanding", [])
            if point.period_start is None and point.value > 0
        }
        shares = shares_after(counts, ncav[0])
        if shares is not None:
            candidates[symbol] = (*ncav, shares)
    if not candidates:
        return []
    raw, adjusted = split_histories(ctx, list(candidates))
    picks: list[Pick] = []
    for symbol, (day, ncav, shares) in candidates.items():
        ratio = split_ratio(raw[symbol], adjusted[symbol], shares.filed)
        if ratio is None:
            continue
        price_to_ncav = float(shares.value) * ratio * ctx.bars[symbol].close / ncav
        if price_to_ncav <= limit:
            reason = f"price {price_to_ncav:.0%} of NCAV ${ncav / 1e6:,.1f}M at {day}, shares on {shares.period_end}"
            picks.append(Pick(symbol, price_to_ncav, reason))
    return sorted(picks, key=lambda pick: pick.rank)


def latest_aaa(ctx: Context) -> float | None:
    today = ctx.session.date
    points = ctx.macro(["aaa"], today - timedelta(days=AAA_LOOKBACK_DAYS), today)
    return float(max(points, key=lambda point: point.date).value) if points else None


def rule_1976(ctx: Context) -> list[Pick]:
    today = ctx.session.date
    aaa = latest_aaa(ctx)
    if aaa is None:
        ctx.log(f"no Moody's AAA yield in the {AAA_LOOKBACK_DAYS} days to {today}, no 1976 screen", "warn")
        return []
    max_pe = min(float(ctx.params["max_pe_1976"]), max(float(ctx.params["always_acceptable_pe_1976"]), 100 / (2 * aaa)))
    min_equity = float(ctx.params["min_equity_to_assets_1976"])
    candidates: dict[str, tuple[float, list[FundamentalPoint]]] = {}
    for symbol, facts in domestic_facts(ctx.fundamentals(screened_symbols(ctx), RULE_1976_METRICS)).items():
        day = latest_balance_sheet(facts, "total_assets", today)
        if day is None or "eps_diluted" not in facts:
            continue
        assets = instants(facts, "total_assets")[day]
        equity = instants(facts, "equity").get(day)
        if equity is not None and assets > 0 and equity / assets >= min_equity:
            candidates[symbol] = (equity / assets, facts["eps_diluted"])
    if not candidates:
        return []
    raw, adjusted = split_histories(ctx, list(candidates))
    picks: list[Pick] = []
    for symbol, (equity_share, eps_points) in candidates.items():
        eps = trailing_eps(eps_points, raw[symbol], adjusted[symbol], today)
        if eps is None or eps <= 0:
            continue
        pe = ctx.bars[symbol].close / eps
        if pe <= max_pe:
            reason = f"P/E {pe:.1f} at most {max_pe:.1f} (AAA {aaa:.2f}%), equity {equity_share:.0%} of assets"
            picks.append(Pick(symbol, pe, reason))
    return sorted(picks, key=lambda pick: pick.rank)


def screen(ctx: Context) -> list[Pick]:
    method = ctx.params["method"]
    if method == "net_net":
        return net_nets(ctx)
    if method == "rule_1976":
        return rule_1976(ctx)
    raise ValueError(f"method must be net_net or rule_1976, not {method!r}")


class Graham(Bot):
    def on_step(self, ctx: Context) -> None:
        exit_rule = ctx.params["exit"]
        if exit_rule == "graham":
            entries = {
                symbol: position.opened_at.date()
                for symbol, position in ctx.portfolio.positions.items()
                if position.qty > 0 and position.opened_at is not None
            }
            self.graham_exits(ctx, entries)
            self.graham_buys(ctx, set(entries))
        elif exit_rule == "oppenheimer":
            self.oppenheimer_cohort(ctx)
        else:
            raise ValueError(f"exit must be graham or oppenheimer, not {exit_rule!r}")

    def graham_exits(self, ctx: Context, entries: dict[str, date]) -> None:
        today = ctx.session.date
        gain = float(ctx.params["exit_gain"])
        years = int(ctx.params["exit_calendar_years"])
        year_end = today.month == DECEMBER and bool(entries) and last_session_of_month(ctx)
        selling = {order.symbol for order in ctx.portfolio.working if order.side == "sell"}
        time_exits: dict[str, str] = ctx.state.setdefault("time_exits", {})
        for symbol, entered in sorted(entries.items()):
            if symbol in selling:
                continue
            cost = float(ctx.position(symbol).avg_cost)
            bar = ctx.bars.get(symbol)
            deadline = entered.year + years
            if bar is not None and bar.close >= cost * (1 + gain):
                ctx.order_target_weight(
                    symbol, 0, reason=f"price: +{gain:.0%} target, close {bar.close:.2f} against cost {cost:.2f}"
                )
            elif today.year > deadline or (today.year == deadline and year_end):
                ctx.order_target_weight(
                    symbol, 0, reason=f"{years}-year limit: bought {entered}, end of {deadline} reached"
                )
                time_exits[symbol] = today.isoformat()

    def graham_buys(self, ctx: Context, held: set[str]) -> None:
        today = ctx.session.date
        month = today.strftime("%Y-%m")
        if ctx.state.get("month") == month:
            return
        ctx.state["month"] = month
        block = int(ctx.params["reentry_block_days"])
        time_exits: dict[str, str] = ctx.state.setdefault("time_exits", {})
        for symbol, exited in list(time_exits.items()):
            if (today - date.fromisoformat(exited)).days >= block:
                del time_exits[symbol]
        names = int(ctx.params["min_names"])
        slots = int(float(ctx.portfolio.cash) * names / float(ctx.portfolio.equity))
        taken = held | set(time_exits) | {order.symbol for order in ctx.portfolio.working}
        picks = [pick for pick in screen(ctx) if pick.symbol not in taken]
        ctx.log(
            f"new qualifiers: {len(picks)}, cash for {max(slots, 0)} positions of 1/{names}, "
            f"{len(time_exits)} blocked for {block} days after a time exit"
        )
        for pick in picks[: max(slots, 0)]:
            ctx.order_target_weight(pick.symbol, 1 / names, reason=pick.reason)

    def oppenheimer_cohort(self, ctx: Context) -> None:
        today = ctx.session.date
        month_end = last_session_of_month(ctx)
        pending: dict[str, Any] | None = ctx.state.get("cohort")
        if pending is not None:
            ranks: dict[str, float] = pending["picks"]
            pending["picks"] = {ctx.renames.get(symbol, symbol): rank for symbol, rank in ranks.items()}
        late = pending is not None and today.year > pending["formed"]
        if (today.month == DECEMBER and month_end) or late:
            ctx.state.pop("cohort", None)
            formed = today.year if pending is None else int(pending["formed"])
            picks: dict[str, float] = {} if pending is None else pending["picks"]
            label = "price {:.0%} of NCAV" if ctx.params["method"] == "net_net" else "P/E {:.1f}"
            ctx.rebalance(
                dict.fromkeys(picks, 1 / len(picks)) if picks else {},
                reason={
                    symbol: f"{label.format(rank)} on the November {formed} close, {formed + 1} cohort"
                    for symbol, rank in picks.items()
                },
                exit_reason="12-month cohort ended",
            )
        elif today.month == NOVEMBER and month_end:
            picks = {pick.symbol: pick.rank for pick in screen(ctx)}
            ctx.state["cohort"] = {"formed": today.year, "picks": picks}
            ctx.log(f"qualifiers on the November close, bought at the turn of the year: {len(picks)}")


if __name__ == "__main__":
    run(Graham)
