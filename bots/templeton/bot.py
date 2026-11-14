from collections.abc import Mapping
from datetime import date, timedelta
from typing import Any, NamedTuple

import numpy as np
from buizz_bots.caps import market_caps
from buizz_bots.fundamentals import (
    DOMESTIC_FORMS,
    Annual,
    amount,
    by_symbol,
    consecutive_fiscal_ends,
    domestic_annual,
    histories_since,
    in_dollars,
    latest_shares,
    traded_today,
)
from buizz_bots.persona import (
    DAYS_PER_YEAR,
    YEAR_SESSIONS,
    Buy,
    Sell,
    candidates,
    held_stocks,
    new_filings,
    restated,
    stock_weights,
    temperature,
    trade,
)
from buizz_bots.tickers import renamed
from buizz_bots.value import EBIT_METRICS, NET_DEBT_METRICS, ebit, net_debt, operating_companies

from buizz import Bot, Context, Metric, run

METRICS: list[Metric] = [*EBIT_METRICS, "equity", "shares_outstanding", *NET_DEBT_METRICS]
FILING_METRIC: Metric = "equity"
CHUNK = 200
IDLE_CASH = 0.01
WHOLE_SLOT = 1e-9


class Bargain(NamedTuple):
    fall: float
    normal_ebit: float
    years: int
    fiscal_end: date
    net_debt: float
    equity: float
    market_cap: float

    @property
    def enterprise_value(self) -> float:
        return self.market_cap + self.net_debt

    @property
    def ev_ebit(self) -> float | None:
        if self.normal_ebit <= 0 or self.enterprise_value <= 0:
            return None
        return self.enterprise_value / self.normal_ebit

    @property
    def earnings_yield(self) -> float | None:
        multiple = self.ev_ebit
        return None if multiple is None else 1 / multiple

    @property
    def described(self) -> str:
        multiple = "n/a" if self.ev_ebit is None else f"{self.ev_ebit:.1f}"
        return (
            f"{self.fall:.0%} below its 3-year high, EV/normal EBIT {multiple} "
            f"(EV ${self.enterprise_value / 1e6:,.0f}M over the {self.years}-year mean EBIT "
            f"${self.normal_ebit / 1e6:,.0f}M to FY {self.fiscal_end}), "
            f"net debt ${self.net_debt / 1e6:,.0f}M"
        )


class Swap(NamedTuple):
    sell: Sell
    buy: Buy


def falls_of(ctx: Context, symbols: list[str]) -> dict[str, float]:
    if not symbols:
        return {}
    sessions = int(ctx.params["high_years"]) * YEAR_SESSIONS
    return {
        symbol: 1 - float(history.close[-1]) / float(np.max(history.close))
        for symbol, history in ctx.history(symbols, "1d", sessions, "split").items()
        if len(history.close) >= YEAR_SESSIONS and traded_today(ctx, history)
    }


def normal_record(annual: Annual, symbol: str, today: date, years: int) -> tuple[float, date] | None:
    reported = annual.get((symbol, "operating_income"), {}).keys() | annual.get((symbol, "pretax_income"), {}).keys()
    ends = consecutive_fiscal_ends(reported, today, years)
    if ends is None:
        return None
    found = [value for end in ends if (value := ebit(annual, symbol, end)) is not None]
    if len(found) < years:
        return None
    return sum(found) / years, ends[0]


def bargains_of(ctx: Context, falls: Mapping[str, float]) -> dict[str, Bargain]:
    symbols = sorted(falls)
    if not symbols:
        return {}
    today = ctx.session.date
    years = int(ctx.params["normal_years"])
    points = sorted(
        (point for point in ctx.fundamentals(symbols, METRICS) if point.form in DOMESTIC_FORMS and in_dollars(point)),
        key=lambda point: point.filed,
    )
    annual = domestic_annual(points)
    grouped = by_symbol(points)
    records = {symbol: record for symbol in symbols if (record := normal_record(annual, symbol, today, years))}
    counts = {symbol: count for symbol in records if (count := latest_shares(grouped[symbol], today)) is not None}
    if not counts:
        return {}
    raw, adjusted = histories_since(ctx, sorted(counts), min(count.filed for count in counts.values()))
    found: dict[str, Bargain] = {}
    for symbol, cap in market_caps(ctx, counts, raw, adjusted).items():
        normal, end = records[symbol]
        equity = amount(annual, symbol, "equity", end)
        if equity is not None:
            found[symbol] = Bargain(falls[symbol], normal, years, end, net_debt(annual, symbol, end), equity, cap)
    return found


def scan(ctx: Context, held: list[str]) -> dict[str, Bargain]:
    deepest = float(ctx.params["min_fall"])
    universe = operating_companies(ctx, sorted(set(ctx.universe) - set(held)))
    found: dict[str, Bargain] = {}
    for offset in range(0, len(universe), CHUNK):
        falls = falls_of(ctx, universe[offset : offset + CHUNK])
        found.update(bargains_of(ctx, {symbol: fall for symbol, fall in falls.items() if fall >= deepest}))
    return found | bargains_of(ctx, falls_of(ctx, held))


def insolvent(params: Mapping[str, Any], bargain: Bargain) -> str | None:
    if bargain.normal_ebit <= 0:
        return (
            f"the {bargain.years}-year mean EBIT is ${bargain.normal_ebit / 1e6:,.0f}M, no earning power to normalise"
        )
    if bargain.equity <= 0:
        return f"equity ${bargain.equity / 1e6:,.0f}M in FY {bargain.fiscal_end}"
    leverage = bargain.net_debt / bargain.normal_ebit
    if leverage > float(params["max_debt_ebit"]):
        return f"net debt {leverage:.1f} times normal EBIT in FY {bargain.fiscal_end}"
    return None


def sell_of(params: Mapping[str, Any], symbol: str, bargain: Bargain) -> Sell | None:
    broken = insolvent(params, bargain)
    if broken is not None:
        return Sell(symbol, "thesis", f"no longer solvent: {broken}")
    multiple = bargain.ev_ebit
    if multiple is not None and multiple >= float(params["full_ev_ebit"]):
        return Sell(symbol, "valuation", f"no longer a bargain: {bargain.described}")
    return None


def timer_of(ctx: Context, symbol: str) -> Sell | None:
    opened = ctx.position(symbol).opened_at
    if opened is None:
        return None
    held = (ctx.session.date - opened.date()).days
    if held < float(ctx.params["hold_years"]) * DAYS_PER_YEAR:
        return None
    return Sell(
        symbol, "timer", f"held {held / DAYS_PER_YEAR:.1f} years since {opened.date()}, his usual holding period"
    )


def bargains_to_buy(ctx: Context, bargains: dict[str, Bargain], skip: set[str]) -> list[tuple[str, float]]:
    universe = set(ctx.universe)
    deepest = float(ctx.params["min_fall"])
    floor = float(ctx.params["min_market_cap"])
    cheapest = float(ctx.params["max_ev_ebit"])

    def gate(symbol: str) -> float | None:
        bargain = bargains[symbol]
        if symbol not in universe or symbol in skip or bargain.fall < deepest or bargain.market_cap < floor:
            return None
        multiple = bargain.ev_ebit
        if multiple is None or multiple > cheapest or insolvent(ctx.params, bargain) is not None:
            return None
        return 1 / multiple

    passed = candidates(bargains, gate)
    since = ctx.session.date - timedelta(days=round(float(ctx.params["restatement_years"]) * DAYS_PER_YEAR))
    unreliable = restated(ctx, [symbol for symbol, _ in passed], since)
    return [(symbol, score) for symbol, score in passed if symbol not in unreliable]


def swaps_of(
    ctx: Context, bargains: dict[str, Bargain], offers: list[tuple[str, float]], kept: list[str], weight: float
) -> list[Swap]:
    edge = 1 + float(ctx.params["swap_edge"])
    worst = sorted(
        (earnings_yield, symbol)
        for symbol in kept
        if symbol in bargains and (earnings_yield := bargains[symbol].earnings_yield) is not None
    )
    found: list[Swap] = []
    for (offered, score), (held_yield, held) in zip(offers, worst, strict=False):
        if score < edge * held_yield:
            break
        found.append(
            Swap(
                Sell(
                    held,
                    "swap",
                    f"{offered} is a bargain {score / held_yield - 1:.0%} better: EBIT/EV {score:.1%} "
                    f"against {held_yield:.1%}",
                ),
                Buy(
                    offered,
                    weight,
                    f"swap for {held}, EBIT/EV {score:.1%} against {held_yield:.1%}: {bargains[offered].described}",
                ),
            )
        )
    return found


def cold_of(ctx: Context) -> tuple[bool, str]:
    heat = temperature(ctx)
    if heat is None:
        return False, "market temperature unknown"
    return heat.heat <= float(ctx.params["cold"]), heat.why


class Templeton(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        cash_symbol = str(ctx.params["cash_symbol"])
        held = held_stocks(ctx, cash_symbol)
        filings = renamed(ctx, ctx.state.get("filings", {}))
        seen = {symbol: filings[symbol] for symbol in held if symbol in filings}
        fresh = new_filings(ctx, held, seen, FILING_METRIC)
        broken = restated(ctx, held, date.fromisoformat(ctx.state.get("checked", today.isoformat())))
        ctx.state["checked"] = today.isoformat()
        cold, why, scanning = self.weather(ctx)
        bargains = scan(ctx, held) if scanning else bargains_of(ctx, falls_of(ctx, fresh))
        sells = [
            Sell(symbol, "thesis", "8-K Item 4.02, past financial statements no longer to be relied on")
            for symbol in sorted(broken)
        ]
        for symbol in held:
            if symbol in broken:
                continue
            found = sell_of(ctx.params, symbol, bargains[symbol]) if symbol in bargains else None
            if (found := found or timer_of(ctx, symbol)) is not None:
                sells.append(found)
        sold = {sell.symbol for sell in sells}
        weights = {symbol: weight for symbol, weight in stock_weights(ctx, cash_symbol).items() if symbol not in sold}
        buys: list[Buy] = []
        if scanning:
            weight = 1 / int(ctx.params["max_names"])
            offers = bargains_to_buy(ctx, bargains, set(held) | sold)
            opening = 0
            if cold:
                room = int((1 - sum(weights.values())) / weight + WHOLE_SLOT)
                opening = max(min(int(ctx.params["max_names"]) - len(weights), room), 0)
                buys = [
                    Buy(symbol, weight, f"maximum pessimism: {bargains[symbol].described}; {why}")
                    for symbol, _ in offers[:opening]
                ]
            swaps = swaps_of(ctx, bargains, offers[opening:], sorted(weights), weight)
            sells += [swap.sell for swap in swaps]
            buys += [swap.buy for swap in swaps]
            sold |= {swap.sell.symbol for swap in swaps}
        if sells or buys or float(ctx.portfolio.cash) > IDLE_CASH * float(ctx.portfolio.equity):
            trade(ctx, sells, buys, cash_symbol)
        new_filings(ctx, [buy.symbol for buy in buys if buy.symbol not in seen], seen, FILING_METRIC)
        ctx.state["filings"] = {symbol: filed for symbol, filed in seen.items() if symbol not in sold}

    def weather(self, ctx: Context) -> tuple[bool, str, bool]:
        today = ctx.session.date
        year, week, _ = today.isocalendar()
        if ctx.state.get("week") == f"{year}-W{week:02d}":
            return False, "", False
        ctx.state["week"] = f"{year}-W{week:02d}"
        cold, why = cold_of(ctx)
        month = f"{today:%Y-%m}"
        scanning = cold or ctx.state.get("month") != month
        ctx.state["month"] = month
        return cold, why, scanning


if __name__ == "__main__":
    run(Templeton)
