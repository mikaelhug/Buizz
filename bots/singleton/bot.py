from datetime import date, timedelta
from decimal import Decimal
from typing import NamedTuple

import numpy as np
import numpy.typing as npt
from buizz_bots.fundamentals import (
    DOMESTIC_FORMS,
    Annual,
    adjusted_shares,
    amount,
    amount_or_zero,
    by_symbol,
    consecutive_fiscal_ends,
    domestic_annual,
    histories_since,
    in_dollars,
    traded_today,
    year_end_shares,
)
from buizz_bots.insiders import net_selling
from buizz_bots.monthly import session_days
from buizz_bots.persona import (
    DAYS_PER_YEAR,
    Buy,
    Ladder,
    Sell,
    held_stocks,
    ladder_weight,
    latest_macro,
    restated,
    stock_weights,
    trade,
)

from buizz import Bot, Context, FundamentalPoint, History, Metric, run

METRICS: list[Metric] = ["shares_outstanding", "share_repurchases", "net_income", "operating_cash_flow", "capex"]
FISCAL_YEARS = 3
YEAR_AGO_DAYS = range(300, 431)
MAX_COUNT_AGE_DAYS = 200
INSIDER_DAYS = 365
RESTATEMENT_DAYS = 5 * 365
MACRO_LOOKBACK_DAYS = 30


class Year(NamedTuple):
    start: date
    end: date
    repurchases: float
    net_income: float
    free_cash_flow: float


class Record(NamedTuple):
    symbol: str
    years: tuple[Year, Year]
    counts: tuple[FundamentalPoint, FundamentalPoint, FundamentalPoint]
    instants: list[FundamentalPoint]


class Bonds(NamedTuple):
    days: npt.NDArray[np.datetime64]
    yields: npt.NDArray[np.float64]


class Measure(NamedTuple):
    shrink: float
    paid_yield: float
    hurdle: float
    latest_yield: float
    latest_bond: float
    earnings_yield: float
    buyback_yield: float
    coverage: float
    issued: float | None
    market_cap: float


def record_of(annual: Annual, symbol: str, instants: list[FundamentalPoint], today: date) -> Record | None:
    ends = consecutive_fiscal_ends(annual.get((symbol, "net_income"), {}), today, FISCAL_YEARS)
    if ends is None:
        return None
    latest, prior, earliest = ends
    counts = [year_end_shares(annual, symbol, end) for end in (earliest, prior, latest)]
    years: list[Year] = []
    for start, end in ((earliest, prior), (prior, latest)):
        income = amount(annual, symbol, "net_income", end)
        operating = amount(annual, symbol, "operating_cash_flow", end)
        if income is None or operating is None:
            return None
        repurchases = amount_or_zero(annual, symbol, "share_repurchases", end)
        free_cash_flow = operating - amount_or_zero(annual, symbol, "capex", end)
        years.append(Year(start, end, repurchases, income, free_cash_flow))
    first, middle, last = counts
    if first is None or middle is None or last is None:
        return None
    return Record(symbol, (years[0], years[1]), (first, middle, last), instants)


def bought_back(record: Record, coverage: float) -> bool:
    repurchased = sum(year.repurchases for year in record.years)
    return (
        all(year.repurchases > 0 and year.net_income > 0 for year in record.years)
        and sum(year.free_cash_flow for year in record.years) >= coverage * repurchased
    )


def bond_yields(ctx: Context, since: date) -> Bonds:
    points = ctx.macro(["dgs10"], since, ctx.session.date)
    days = np.array([point.date for point in points], dtype="datetime64[D]")
    return Bonds(days, np.array([float(point.value) / 100 for point in points]))


def mean_between(
    days: npt.NDArray[np.datetime64], values: npt.NDArray[np.float64], start: date, end: date
) -> float | None:
    inside = (days > np.datetime64(start)) & (days <= np.datetime64(end))
    return float(values[inside].mean()) if inside.any() else None


def year_ago_growth(record: Record, raw: History, adjusted: History, today: date) -> float | None:
    counts: dict[date, FundamentalPoint] = {}
    for point in sorted(record.instants, key=lambda point: point.filed):
        counts[point.period_end] = point
    if not counts:
        return None
    latest = max(counts)
    if (today - latest).days > MAX_COUNT_AGE_DAYS:
        return None
    earlier = [end for end in counts if (latest - end).days in YEAR_AGO_DAYS]
    if not earlier:
        return None
    before = min(earlier, key=lambda end: abs((latest - end).days - DAYS_PER_YEAR))
    now, then = adjusted_shares(counts[latest], raw, adjusted), adjusted_shares(counts[before], raw, adjusted)
    return None if now is None or then is None or then <= 0 else now / then - 1


def measure(
    record: Record, raw: History, adjusted: History, bonds: Bonds, max_pe: float, today: date
) -> Measure | None:
    shares = [count for point in record.counts if (count := adjusted_shares(point, raw, adjusted)) is not None]
    if len(shares) != len(record.counts) or min(shares) <= 0:
        return None
    first, middle, last = shares
    days = session_days(adjusted)
    yields: list[float] = []
    hurdles: list[float] = []
    bonds_paid: list[float] = []
    for year, (opening, closing) in zip(record.years, ((first, middle), (middle, last)), strict=True):
        price = mean_between(days, adjusted.close, year.start, year.end)
        bond = mean_between(bonds.days, bonds.yields, year.start, year.end)
        if price is None or bond is None:
            return None
        yields.append(year.net_income / (price * (opening + closing) / 2))
        hurdles.append(max(bond, 1 / max_pe))
        bonds_paid.append(bond)
    repurchased = sum(year.repurchases for year in record.years)
    weights = [year.repurchases / repurchased for year in record.years] if repurchased > 0 else [0.5, 0.5]
    span = (record.years[1].end - record.years[0].start).days / DAYS_PER_YEAR
    market_cap = float(adjusted.close[-1]) * last
    latest = record.years[1]
    return Measure(
        shrink=(last / first) ** (1 / span) - 1,
        paid_yield=float(np.dot(weights, yields)),
        hurdle=float(np.dot(weights, hurdles)),
        latest_yield=yields[1],
        latest_bond=bonds_paid[1],
        earnings_yield=latest.net_income / market_cap,
        buyback_yield=latest.repurchases / market_cap,
        coverage=sum(year.free_cash_flow for year in record.years) / repurchased if repurchased > 0 else 0.0,
        issued=year_ago_growth(record, raw, adjusted, today),
        market_cap=market_cap,
    )


class Singleton(Bot):
    def on_step(self, ctx: Context) -> None:
        month = f"{ctx.session.date:%Y-%m}"
        if ctx.state.get("month") == month:
            return
        ctx.state["month"] = month
        bills = ctx.params["bills"]
        held = held_stocks(ctx, bills)
        measures = self.measures(ctx, sorted(set(ctx.universe) | set(held)), set(held))
        cheap = self.cheap(ctx, measures)
        watched = sorted(set(held) | set(cheap))
        today = ctx.session.date
        broken = restated(ctx, watched, today - timedelta(days=RESTATEMENT_DAYS))
        net_sold = net_selling(ctx.insider_trades(watched, today - timedelta(days=INSIDER_DAYS))) if watched else {}
        sells = self.exits(ctx, held, measures, broken, net_sold)
        qualified = {
            symbol: measured
            for symbol, measured in cheap.items()
            if symbol not in broken and net_sold.get(symbol, Decimal(0)) <= 0
        }
        share, why = self.stock_share(ctx)
        buys = self.buys(ctx, held, sells, qualified, share, why)
        ctx.log(f"qualifiers: {len(qualified)} of {len(measures)} measured, {len(held)} held, stock share {share:.0%}")
        trade(ctx, sells, buys, bills, share=share)

    def measures(self, ctx: Context, symbols: list[str], held: set[str]) -> dict[str, Measure]:
        today = ctx.session.date
        coverage = float(ctx.params["fcf_coverage"])
        points = [point for point in ctx.fundamentals(symbols, METRICS) if point.form in DOMESTIC_FORMS]
        annual = domestic_annual(points)
        instants = by_symbol(
            point
            for point in points
            if point.metric == "shares_outstanding"
            and point.period_start is None
            and in_dollars(point)
            and point.value > 0
            and (today - point.period_end).days <= MAX_COUNT_AGE_DAYS + YEAR_AGO_DAYS.stop
        )
        records = [
            found
            for symbol in symbols
            if (found := record_of(annual, symbol, instants.get(symbol, []), today)) is not None
            and (symbol in held or bought_back(found, coverage))
        ]
        if not records:
            return {}
        since = min(min(found.years[0].start, *(point.filed for point in found.instants)) for found in records)
        raw, adjusted = histories_since(ctx, [found.symbol for found in records], since)
        bonds = bond_yields(ctx, since)
        max_pe = float(ctx.params["max_pe"])
        return {
            found.symbol: measured
            for found in records
            if found.symbol in raw
            and traded_today(ctx, raw[found.symbol])
            and (measured := measure(found, raw[found.symbol], adjusted[found.symbol], bonds, max_pe, today))
            is not None
        }

    def exits(
        self,
        ctx: Context,
        held: list[str],
        measures: dict[str, Measure],
        broken: set[str],
        net_sold: dict[str, Decimal],
    ) -> list[Sell]:
        issuing = float(ctx.params["exit_issuance"])
        selling = float(ctx.params["exit_insider_selling"])
        sells: list[Sell] = []
        for symbol in held:
            measured = measures.get(symbol)
            sold = float(net_sold.get(symbol, 0))
            if symbol in broken:
                sells.append(Sell(symbol, "thesis", "8-K Item 4.02 non-reliance: the accounts it was bought on"))
            elif measured is None:
                continue
            elif measured.issued is not None and measured.issued >= issuing:
                sells.append(Sell(symbol, "thesis", f"issuing: shares {measured.issued:+.1%} in a year"))
            elif measured.latest_yield < measured.latest_bond and measured.buyback_yield > 0:
                sells.append(
                    Sell(
                        symbol,
                        "thesis",
                        f"buying back expensive: earnings yield {measured.latest_yield:.1%} at the year's prices "
                        f"below the 10-year Treasury's {measured.latest_bond:.1%}",
                    )
                )
            elif sold >= selling * measured.market_cap:
                sells.append(
                    Sell(
                        symbol,
                        "thesis",
                        f"insiders net sellers of ${sold:,.0f} in 12 months, "
                        f"{sold / measured.market_cap:.1%} of the company",
                    )
                )
        return sells

    def cheap(self, ctx: Context, measures: dict[str, Measure]) -> dict[str, Measure]:
        shrink = float(ctx.params["min_shrink"])
        bond = latest_macro(ctx, "dgs10", MACRO_LOOKBACK_DAYS)
        hurdle_now = max((bond or 0) / 100, 1 / float(ctx.params["max_pe"]))
        return {
            symbol: measured
            for symbol, measured in measures.items()
            if measured.buyback_yield > 0
            and measured.shrink <= -shrink
            and measured.paid_yield >= measured.hurdle
            and measured.earnings_yield >= hurdle_now
            and (measured.issued is None or measured.issued <= 0)
        }

    def stock_share(self, ctx: Context) -> tuple[float, str]:
        pe = latest_macro(ctx, "sp500_forward_pe", MACRO_LOOKBACK_DAYS)
        bond = latest_macro(ctx, "dgs10", MACRO_LOOKBACK_DAYS)
        if pe is None or bond is None or pe <= 0:
            return 1.0, "fully in stocks without a forward P/E or 10-year yield"
        if 100 / pe >= bond:
            return 1.0, f"S&P 500 forward earnings yield {100 / pe:.1f}% at least the 10-year's {bond:.1f}%"
        defensive = float(ctx.params["defensive_share"])
        return defensive, f"S&P 500 forward earnings yield {100 / pe:.1f}% below the 10-year's {bond:.1f}%"

    def buys(
        self,
        ctx: Context,
        held: list[str],
        sells: list[Sell],
        qualified: dict[str, Measure],
        share: float,
        why: str,
    ) -> list[Buy]:
        ladder = Ladder(
            float(ctx.params["start"]),
            float(ctx.params["add_step"]),
            float(ctx.params["add_on_drop"]),
            float(ctx.params["cap"]),
        )
        weights = stock_weights(ctx, ctx.params["bills"])
        sold = {sell.symbol for sell in sells}
        kept = [symbol for symbol in held if symbol not in sold]
        adds: list[Buy] = []
        for symbol in kept:
            if symbol not in qualified:
                continue
            weight = ladder_weight(ctx, ladder, symbol, weights.get(symbol, 0))
            if weight is not None:
                cost = float(ctx.position(symbol).avg_cost)
                adds.append(
                    Buy(
                        symbol,
                        weight,
                        f"add: close {ctx.bars[symbol].close / cost - 1:+.0%} on cost, {describe(qualified[symbol])}",
                    )
                )
        room = max(int(ctx.params["max_names"]) - len(kept), 0)
        fresh = sorted(
            (symbol for symbol in qualified if symbol not in kept and symbol not in sold),
            key=lambda symbol: (-qualified[symbol].buyback_yield * qualified[symbol].earnings_yield, symbol),
        )[:room]
        opens = [Buy(symbol, ladder.start, f"{describe(qualified[symbol])}; {why}") for symbol in fresh]
        return adds + opens


def describe(measured: Measure) -> str:
    return (
        f"shares {measured.shrink:+.1%}/yr, buybacks at an earnings yield of {measured.paid_yield:.1%} "
        f"against a {measured.hurdle:.1%} hurdle, buyback yield {measured.buyback_yield:.1%}, "
        f"earnings yield now {measured.earnings_yield:.1%}, free cash flow {measured.coverage:.0%} of buybacks"
    )


if __name__ == "__main__":
    run(Singleton)
