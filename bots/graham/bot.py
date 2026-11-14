from collections import defaultdict
from datetime import date, timedelta
from itertools import pairwise
from typing import Any, NamedTuple

import numpy as np
from buizz_bots.fundamentals import DOMESTIC_FORMS, in_dollars, shares_after, with_fourth_quarters
from buizz_bots.monthly import session_days
from buizz_bots.persona import Buy, Sell, stock_share, stock_weights, temperature, trade
from buizz_bots.sessions import last_session_of_month

from buizz import Bot, Context, FundamentalPoint, History, Metric, run, split_ratio

NET_NET_METRICS: list[Metric] = [
    "current_assets",
    "total_liabilities",
    "total_assets",
    "equity",
    "total_equity",
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
DIVIDEND_BARS = 756
DIVIDEND_STEP = 1e-6
PAYMENTS_COMPARED = 4
MIN_PAYMENTS = 2
OMISSION_GAPS = 1.5
BLOCKING = {"timer", "thesis"}

type Facts = dict[Metric, list[FundamentalPoint]]


class Pick(NamedTuple):
    symbol: str
    rank: float
    reason: str


class Dividend(NamedTuple):
    day: date
    amount: float


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
    total_equity = instants(facts, "total_equity").get(day)
    if assets is not None and total_equity is not None:
        return assets - total_equity
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
    if method == "graham":
        found = net_nets(ctx)
        listed = {pick.symbol for pick in found}
        return found + [pick for pick in rule_1976(ctx) if pick.symbol not in listed]
    raise ValueError(f"method must be graham, net_net or rule_1976, not {method!r}")


def dividends(split: History, total: History) -> list[Dividend]:
    if len(split.close) < MIN_PAYMENTS or not np.array_equal(split.start, total.start):
        return []
    ratio = total.close / split.close
    steps = ratio[1:] / ratio[:-1]
    days = session_days(split)
    return [
        Dividend(days[index + 1].item(), float(split.close[index] * (1 - 1 / steps[index])))
        for index in np.flatnonzero(steps > 1 + DIVIDEND_STEP)
    ]


def dividend_break(paid: list[Dividend], entered: date, today: date, cut: float) -> str | None:
    if len(paid) < MIN_PAYMENTS:
        return None
    recent = paid[-PAYMENTS_COMPARED - 1 :]
    *before, latest = recent
    usual = float(np.median([payment.amount for payment in before]))
    if latest.day >= entered and latest.amount < usual * (1 - cut):
        return f"dividend cut to {latest.amount:.4f} on {latest.day} from a usual {usual:.4f}"
    gaps = [(second.day - first.day).days for first, second in pairwise(recent)]
    due = latest.day + timedelta(days=round(OMISSION_GAPS * float(np.median(gaps))))
    if entered <= due < today:
        return f"dividend omitted: none since {latest.day}, next was due by {due}"
    return None


def temperature_share(ctx: Context) -> tuple[float, str]:
    low, high = float(ctx.params["stock_low"]), float(ctx.params["stock_high"])
    heat = temperature(ctx)
    if heat is None:
        return (low + high) / 2, "no market temperature, the 50-50 default"
    return stock_share(heat, low, high), heat.why


class Graham(Bot):
    def on_step(self, ctx: Context) -> None:
        exit_rule = ctx.params["exit"]
        if exit_rule == "graham":
            self.graham(ctx)
        elif exit_rule == "oppenheimer":
            self.oppenheimer_cohort(ctx)
        else:
            raise ValueError(f"exit must be graham or oppenheimer, not {exit_rule!r}")

    def graham(self, ctx: Context) -> None:
        bonds = ctx.params["bonds"]
        stored: dict[str, str] = ctx.state.get("blocked", {})
        blocked = {ctx.renames.get(symbol, symbol): exited for symbol, exited in stored.items()}
        ctx.state["blocked"] = blocked
        entries = {
            symbol: position.opened_at.date()
            for symbol, position in ctx.portfolio.positions.items()
            if position.qty > 0 and position.opened_at is not None and symbol != bonds
        }
        sells = self.graham_exits(ctx, entries)
        for sell in sells:
            if sell.trigger in BLOCKING:
                blocked[sell.symbol] = ctx.session.date.isoformat()
        month = ctx.session.date.strftime("%Y-%m")
        if ctx.state.get("month") == month:
            if sells:
                trade(ctx, sells, [], bonds)
            return
        ctx.state["month"] = month
        share, why = temperature_share(ctx)
        kept = set(entries) - {sell.symbol for sell in sells}
        sells += self.split_trims(ctx, kept, share, why)
        trade(ctx, sells, self.graham_buys(ctx, set(entries), share, why), bonds, share=share)

    def graham_exits(self, ctx: Context, entries: dict[str, date]) -> list[Sell]:
        today = ctx.session.date
        gain = float(ctx.params["exit_gain"])
        years = int(ctx.params["exit_calendar_years"])
        cut = float(ctx.params["dividend_cut"])
        year_end = today.month == DECEMBER and bool(entries) and last_session_of_month(ctx)
        selling = {order.symbol for order in ctx.portfolio.working if order.side == "sell"}
        watched = sorted(set(entries) - selling)
        split = ctx.history(watched, "1d", DIVIDEND_BARS, "split") if watched else {}
        total = ctx.history(watched, "1d", DIVIDEND_BARS, "all") if watched else {}
        sells: list[Sell] = []
        for symbol in watched:
            entered = entries[symbol]
            cost = float(ctx.position(symbol).avg_cost)
            bar = ctx.bars.get(symbol)
            deadline = entered.year + years
            paid = dividends(split[symbol], total[symbol]) if symbol in split and symbol in total else []
            broken = dividend_break(paid, entered, today, cut)
            if bar is not None and bar.close >= cost * (1 + gain):
                sells.append(
                    Sell(symbol, "price", f"+{gain:.0%} target, close {bar.close:.2f} against cost {cost:.2f}")
                )
            elif today.year > deadline or (today.year == deadline and year_end):
                sells.append(Sell(symbol, "timer", f"{years}-year limit: bought {entered}, end of {deadline} reached"))
            elif broken is not None:
                sells.append(Sell(symbol, "thesis", broken))
        return sells

    def split_trims(self, ctx: Context, kept: set[str], share: float, why: str) -> list[Sell]:
        weights = {
            symbol: weight for symbol, weight in stock_weights(ctx, ctx.params["bonds"]).items() if symbol in kept
        }
        held = sum(weights.values())
        if held <= share + float(ctx.params["split_band"]):
            return []
        scale = share / held
        detail = f"stock share {held:.0%} more than {ctx.params['split_band']:.0%} over its {share:.0%} target, {why}"
        return [Sell(symbol, "valuation", detail, weight * scale) for symbol, weight in sorted(weights.items())]

    def graham_buys(self, ctx: Context, held: set[str], share: float, why: str) -> list[Buy]:
        today = ctx.session.date
        block = int(ctx.params["reentry_block_days"])
        blocked: dict[str, str] = ctx.state["blocked"]
        for symbol, exited in list(blocked.items()):
            if (today - date.fromisoformat(exited)).days >= block:
                del blocked[symbol]
        names = int(ctx.params["min_names"])
        taken = held | set(blocked) | {order.symbol for order in ctx.portfolio.working}
        picks = [pick for pick in screen(ctx) if pick.symbol not in taken]
        ctx.log(
            f"new qualifiers: {len(picks)}, stock share target {share:.0%} ({why}) in slots of 1/{names} of it, "
            f"{len(blocked)} blocked for {block} days after an exit"
        )
        return [Buy(pick.symbol, share / names, f"{pick.reason}; stock share target {share:.0%}") for pick in picks]

    def oppenheimer_cohort(self, ctx: Context) -> None:
        today = ctx.session.date
        month_end = last_session_of_month(ctx)
        pending: dict[str, Any] | None = ctx.state.get("cohort")
        if pending is not None:
            reasons: dict[str, str] = pending["picks"]
            pending["picks"] = {ctx.renames.get(symbol, symbol): reason for symbol, reason in reasons.items()}
        late = pending is not None and today.year > pending["formed"]
        if (today.month == DECEMBER and month_end) or late:
            ctx.state.pop("cohort", None)
            formed = today.year if pending is None else int(pending["formed"])
            picks: dict[str, str] = {} if pending is None else pending["picks"]
            ctx.rebalance(
                dict.fromkeys(picks, 1 / len(picks)) if picks else {},
                reason={
                    symbol: f"{reason} on the November {formed} close, {formed + 1} cohort"
                    for symbol, reason in picks.items()
                },
                exit_reason="12-month cohort ended",
            )
        elif today.month == NOVEMBER and month_end:
            picks = {pick.symbol: pick.reason for pick in screen(ctx)}
            ctx.state["cohort"] = {"formed": today.year, "picks": picks}
            ctx.log(f"qualifiers on the November close, bought at the turn of the year: {len(picks)}")


if __name__ == "__main__":
    run(Graham)
