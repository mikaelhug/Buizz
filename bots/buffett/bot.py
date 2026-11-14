import bisect
import math
from collections import defaultdict
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from datetime import date, timedelta
from typing import Any, NamedTuple

import numpy as np
from buizz_bots.fundamentals import (
    DOMESTIC_FORMS,
    Annual,
    amount,
    amount_or_zero,
    by_symbol,
    consecutive_fiscal_ends,
    domestic_annual,
    histories_since,
    latest_filing,
    total_debt,
    traded_today,
    year_end_shares,
)
from buizz_bots.macro import latest_monthly
from buizz_bots.persona import (
    DAYS_PER_YEAR,
    Buy,
    Ladder,
    Sell,
    candidates,
    held_stocks,
    ladder_weight,
    latest_macro,
    restated,
    stock_weights,
    trade,
)
from buizz_bots.ranks import percentiles
from buizz_bots.sessions import week_ends
from buizz_bots.tickers import renamed
from buizz_bots.value import FINANCIALS
from systematic import AnnualCache, rebalance

from buizz import Bot, Context, InsiderTrade, MacroPoint, Metric, run, split_ratio

VARIANTS = ("owner", "systematic")
METRICS: list[Metric] = [
    "operating_income",
    "net_income",
    "depreciation_amortization",
    "capex",
    "equity",
    "pretax_income",
    "dividends_paid",
    "share_repurchases",
    "shares_outstanding",
    "short_term_debt",
    "short_term_borrowings",
    "current_long_term_debt",
    "long_term_debt",
    "long_term_debt_noncurrent",
]
CHUNK = 200
CPI_BASE_YEAR = 2014
RATE_LOOKBACK_DAYS = 14
RESTATEMENT_YEARS = 5
INSIDER_DAYS = 365
DAYS_PER_MONTH = DAYS_PER_YEAR / 12
PARK_ABOVE = 0.02
RATIO_DIGITS = 4
FILING_METRIC: Metric = "operating_income"
CHECKED = "checked"
WATCHED = "watched"
ORIGINS = "origins"
STEADINESS = "steadiness"
STEADINESS_REACH = 64


class Rules(NamedTuple):
    record_years: int
    min_roe: float
    max_debt_cover: float
    max_capex_share: float
    min_pretax: float
    min_yield: float
    max_insider_sale: float


class Year(NamedTuple):
    end: date
    operating_income: float
    net_income: float
    cash_earnings: float
    capex: float
    equity: float
    debt: float
    retained: float

    @property
    def owner_earnings(self) -> float:
        return self.cash_earnings - self.capex


class Record(NamedTuple):
    fiscal_end: str
    owner_earnings: float
    shares: float
    shares_filed: str
    roe: float
    debt_cover: float
    capex_share: float
    retained_return: float | None
    roc_cv: float


class Assessment(NamedTuple):
    roc_cv: float | None
    outcome: Record | str


class Review(NamedTuple):
    records: dict[str, Record]
    failures: dict[str, str]
    restated: set[str]


class Prices(NamedTuple):
    yields: dict[str, float]
    hurdle: float
    hurdle_why: str
    rate: float | None


class Steadiness(NamedTuple):
    below: int
    middle: list[float]
    above: int

    @property
    def records(self) -> int:
        return self.below + len(self.middle) + self.above

    @property
    def median(self) -> float:
        if not self.records:
            return 0.0
        low, high = ((self.records - 1) // 2, self.records // 2)
        last = len(self.middle) - 1
        return float(np.mean([self.middle[min(max(rank - self.below, 0), last)] for rank in (low, high)]))


class Watch(NamedTuple):
    day: str | None
    filed: str | None
    before: float | None
    after: float | None


class Memory:
    def __init__(self) -> None:
        self.spreads: dict[str, float | None] = {}
        self.members: dict[date, set[str]] = {}

    def members_on(self, ctx: Context, day: date) -> set[str]:
        if day not in self.members:
            self.members[day] = set(ctx.universe_at(day, int(ctx.params["owner_universe"]), asset_class="stock"))
        return self.members[day]


@dataclass
class Book:
    held: list[str]
    remaining: list[str]
    weights: dict[str, float]
    ladder: Ladder
    free: float


def rules_of(params: Mapping[str, Any]) -> Rules:
    return Rules(
        int(params["record_years"]),
        float(params["min_roe"]),
        float(params["max_debt_cover"]),
        float(params["max_capex_share"]),
        float(params["min_pretax_2014"]),
        float(params["min_yield"]),
        float(params["max_insider_sale"]),
    )


def money(value: float) -> str:
    return f"${value / 1e6:,.0f}M"


def treasury_10y(ctx: Context) -> float | None:
    found = latest_macro(ctx, "dgs10", RATE_LOOKBACK_DAYS)
    return None if found is None else found / 100


def price_level(ctx: Context) -> float:
    months: dict[date, MacroPoint] = {}
    base_year = ctx.macro(["cpiaucns"], date(CPI_BASE_YEAR, 1, 1), date(CPI_BASE_YEAR, 12, 31))
    for point in sorted(base_year, key=lambda point: point.published):
        months[point.date] = point
    latest = latest_monthly(ctx, "cpiaucns", 1)
    if not latest or not months:
        return 1.0
    return float(latest[-1].value) / float(np.mean([float(point.value) for point in months.values()]))


def year_of(annual: Annual, symbol: str, end: date) -> Year | None:
    found = [
        amount(annual, symbol, metric, end)
        for metric in ("operating_income", "net_income", "depreciation_amortization", "capex", "equity")
    ]
    values = [value for value in found if value is not None]
    if len(values) < len(found):
        return None
    ebit, income, depreciation, capex, equity = values
    paid_out = abs(amount_or_zero(annual, symbol, "dividends_paid", end)) + abs(
        amount_or_zero(annual, symbol, "share_repurchases", end)
    )
    debt = total_debt(annual, symbol, end)
    return Year(end, ebit, income, income + depreciation, abs(capex), equity, debt, income - paid_out)


def return_on_retained(record: list[Year]) -> float | None:
    retained = sum(year.retained for year in record[1:])
    if retained <= 0:
        return None
    return (record[-1].owner_earnings - record[0].owner_earnings) / retained


def mean_roe(record: list[Year]) -> float:
    return float(np.mean([year.net_income / year.equity for year in record]))


def capex_share(record: list[Year]) -> float:
    return sum(year.capex for year in record) / sum(year.cash_earnings for year in record)


def record_of(annual: Annual, symbol: str, today: date, years: int) -> list[Year] | str:
    ends = consecutive_fiscal_ends(annual.get((symbol, "operating_income"), {}), today, years)
    if ends is None:
        return f"no {years} consecutive fiscal years of 10-K operating income"
    found = {end: year_of(annual, symbol, end) for end in reversed(ends)}
    record = [year for year in found.values() if year is not None]
    if len(record) < len(found):
        missing = min(end for end, year in found.items() if year is None)
        return f"FY ending {missing} lacks net income, D&A, capex or equity"
    weak = next((year for year in record if year.operating_income <= 0 or year.owner_earnings <= 0), None)
    if weak is not None:
        return (
            f"FY ending {weak.end}: operating income {money(weak.operating_income)}, "
            f"owner earnings {money(weak.owner_earnings)}"
        )
    thin = next((year for year in record if year.equity <= 0), None)
    if thin is not None:
        return f"shareholders' equity {money(thin.equity)} in FY ending {thin.end}"
    return record


def weakness(record: list[Year], pretax: float | None, rules: Rules, pretax_floor: float) -> str | None:
    latest = record[-1]
    roe = mean_roe(record)
    if roe < rules.min_roe:
        return f"{len(record)}y mean return on equity {roe:.1%}, below {rules.min_roe:.0%}"
    debt_cover = latest.debt / latest.owner_earnings
    if debt_cover > rules.max_debt_cover:
        return f"debt {money(latest.debt)} is {debt_cover:.1f}x owner earnings, above {rules.max_debt_cover:g}x"
    reinvested = capex_share(record)
    if reinvested > rules.max_capex_share:
        return f"capex took {reinvested:.0%} of net income plus D&A, above {rules.max_capex_share:.0%}"
    if pretax is None or pretax < pretax_floor:
        return f"pre-tax income {'not reported' if pretax is None else money(pretax)}, below {money(pretax_floor)}"
    retained_return = return_on_retained(record)
    if retained_return is not None and retained_return < rules.min_yield:
        return (
            f"a retained dollar added {retained_return:.1%} to owner earnings since FY {record[0].end.year}, "
            f"below {rules.min_yield:.0%}"
        )
    return None


def assessment_of(annual: Annual, symbol: str, today: date, rules: Rules, pretax_floor: float) -> Assessment:
    record = record_of(annual, symbol, today, rules.record_years)
    if isinstance(record, str):
        return Assessment(None, record)
    returns = np.array([year.operating_income / (year.equity + year.debt) for year in record])
    roc_cv = round(float(returns.std() / returns.mean()), RATIO_DIGITS)
    latest = record[-1]
    failure = weakness(record, amount(annual, symbol, "pretax_income", latest.end), rules, pretax_floor)
    if failure is not None:
        return Assessment(roc_cv, failure)
    shares = year_end_shares(annual, symbol, latest.end)
    if shares is None or shares.value <= 0:
        return Assessment(roc_cv, f"no share count after FY ending {latest.end}")
    retained_return = return_on_retained(record)
    return Assessment(
        roc_cv,
        Record(
            latest.end.isoformat(),
            latest.owner_earnings,
            float(shares.value),
            shares.filed.isoformat(),
            round(mean_roe(record), RATIO_DIGITS),
            round(latest.debt / latest.owner_earnings, RATIO_DIGITS),
            round(capex_share(record), RATIO_DIGITS),
            None if retained_return is None else round(retained_return, RATIO_DIGITS),
            roc_cv,
        ),
    )


def assess(ctx: Context, symbols: list[str], rules: Rules) -> dict[str, Assessment]:
    today = ctx.session.date
    profiles = ctx.instruments(symbols)
    floor = rules.min_pretax * price_level(ctx)
    found: dict[str, Assessment] = {}
    operating: list[str] = []
    for symbol in symbols:
        sic = profiles[symbol].sic if symbol in profiles else None
        if sic is not None and sic in FINANCIALS:
            found[symbol] = Assessment(None, f"a financial (SIC {sic}), which owner earnings do not describe")
        else:
            operating.append(symbol)
    for offset in range(0, len(operating), CHUNK):
        chunk = operating[offset : offset + CHUNK]
        annual = domestic_annual(ctx.fundamentals(chunk, METRICS))
        found |= {symbol: assessment_of(annual, symbol, today, rules, floor) for symbol in chunk}
    return found


def insider_net_sale(trades: Iterable[InsiderTrade]) -> float:
    bought = sold = 0.0
    stakes: dict[tuple[frozenset[int], str, bool], tuple[date, float]] = {}
    for line in trades:
        if line.shares is None or not any(owner.officer or owner.director for owner in line.owners):
            continue
        if line.code == "P" and line.acquired:
            bought += float(line.shares)
        elif line.code == "S" and not line.acquired and line.plan_10b5_1 is not True:
            sold += float(line.shares)
        if line.shares_after is not None:
            key = (frozenset(owner.cik for owner in line.owners), line.security, line.direct)
            if key not in stakes or line.date >= stakes[key][0]:
                stakes[key] = (line.date, float(line.shares_after))
    net = sold - bought
    return net / (net + sum(stake for _, stake in stakes.values())) if net > 0 else 0.0


def judge(
    ctx: Context, assessed: Mapping[str, Assessment], steadiness: Steadiness, rules: Rules
) -> dict[str, Record | str]:
    today = ctx.session.date
    outcomes: dict[str, Record | str] = {}
    for symbol, seen in assessed.items():
        outcome = seen.outcome
        if isinstance(outcome, Record) and outcome.roc_cv > steadiness.median:
            outcome = (
                f"return on capital varies {outcome.roc_cv:.2f} (std/mean), outside the steadier half "
                f"of {steadiness.records} records (median {steadiness.median:.2f})"
            )
        outcomes[symbol] = outcome
    passed = sorted(symbol for symbol, outcome in outcomes.items() if isinstance(outcome, Record))
    if not passed:
        return outcomes
    for symbol in restated(ctx, passed, today - timedelta(days=round(RESTATEMENT_YEARS * DAYS_PER_YEAR))):
        outcomes[symbol] = f"8-K Item 4.02 restatement within {RESTATEMENT_YEARS} years"
    aligned = [symbol for symbol in passed if isinstance(outcomes[symbol], Record)]
    trades: dict[str, list[InsiderTrade]] = defaultdict(list)
    for line in ctx.insider_trades(aligned, today - timedelta(days=INSIDER_DAYS)) if aligned else []:
        trades[line.symbol].append(line)
    for symbol in aligned:
        sale = insider_net_sale(trades[symbol])
        if sale > rules.max_insider_sale:
            outcomes[symbol] = (
                f"officers and directors sold {sale:.0%} of their stake, net, in {INSIDER_DAYS} days "
                f"outside 10b5-1 plans, above {rules.max_insider_sale:.0%}"
            )
    return outcomes


def owner_yields(ctx: Context, records: Mapping[str, Record]) -> dict[str, float]:
    if not records:
        return {}
    since = min(date.fromisoformat(record.shares_filed) for record in records.values())
    raw, split = histories_since(ctx, sorted(records), since)
    yields: dict[str, float] = {}
    for symbol, record in records.items():
        history = raw.get(symbol)
        if history is None or not traded_today(ctx, history):
            continue
        ratio = split_ratio(history, split[symbol], date.fromisoformat(record.shares_filed))
        if ratio is None:
            continue
        market_cap = record.shares * ratio * float(history.close[-1])
        if market_cap > 0:
            yields[symbol] = record.owner_earnings / market_cap
    return yields


def qualification(record: Record, years: int) -> str:
    retained = "" if record.retained_return is None else f", {record.retained_return:.0%} on retained earnings"
    return (
        f"{years}y record: ROE {record.roe:.0%}, owner earnings {money(record.owner_earnings)} "
        f"(FY ending {record.fiscal_end}), debt {record.debt_cover:.1f}x owner earnings, "
        f"capex {record.capex_share:.0%} of cash earnings, ROC std/mean {record.roc_cv:.2f}{retained}"
    )


def months_held(ctx: Context, symbol: str) -> str:
    opened = ctx.position(symbol).opened_at
    if opened is None:
        return ""
    return f", held {(ctx.session.date - opened.date()).days / DAYS_PER_MONTH:.0f} months"


def latest_filings(ctx: Context, symbols: set[str]) -> dict[str, date]:
    if not symbols:
        return {}
    grouped = by_symbol(ctx.fundamentals(sorted(symbols), [FILING_METRIC]))
    return {
        symbol: filed for symbol in symbols if (filed := latest_filing(grouped[symbol], DOMESTIC_FORMS)) is not None
    }


def steadiness_of(spreads: Iterable[float | None]) -> Steadiness:
    ordered = sorted(spread for spread in spreads if spread is not None)
    centre = len(ordered) // 2
    low, high = max(centre - STEADINESS_REACH, 0), min(centre + STEADINESS_REACH + 1, len(ordered))
    return Steadiness(low, ordered[low:high], len(ordered) - high)


def rescored(steadiness: Steadiness, changes: Iterable[Watch]) -> Steadiness:
    below, middle, above = steadiness.below, list(steadiness.middle), steadiness.above
    floor = middle[0] if below else -math.inf
    ceiling = middle[-1] if above else math.inf
    for change in changes:
        if change.before is None:
            pass
        elif change.before < floor:
            below -= 1
        elif change.before > ceiling:
            above -= 1
        elif change.before in middle:
            middle.remove(change.before)
        elif change.before == floor:
            below -= 1
        else:
            above -= 1
        if change.after is None:
            pass
        elif change.after < floor:
            below += 1
        elif change.after > ceiling:
            above += 1
        else:
            bisect.insort(middle, change.after)
    return Steadiness(below, middle, above)


@dataclass
class Watchlist:
    checked: date | None
    watches: dict[str, Watch]
    origins: dict[str, str]
    filings: dict[str, date]

    @classmethod
    def load(cls, ctx: Context, held: list[str], records: Mapping[str, Record]) -> "Watchlist":
        watches = {symbol: Watch(*fields) for symbol, fields in renamed(ctx, ctx.state.get(WATCHED, {})).items()}
        for symbol in held:
            if symbol not in watches:
                ranked = records[symbol].roc_cv if symbol in records else None
                watches[symbol] = Watch(None, None, ranked, ranked)
        origins: dict[str, str] = dict(ctx.state.get(ORIGINS, {}))
        for old, new in ctx.renames.items():
            origins[new] = origins.pop(old, old)
        checked = date.fromisoformat(ctx.state[CHECKED]) if CHECKED in ctx.state else None
        return cls(checked, watches, origins, {})

    def fresh(
        self, ctx: Context, held: list[str], records: Mapping[str, Record], members: set[str] | None, memory: Memory
    ) -> list[str]:
        watched = set(held) if members is None else members.union(held)
        known = set(held)
        if members is not None:
            before = set[str]() if self.checked is None else memory.members_on(ctx, self.checked)
            known = {symbol for symbol in watched if self.origins.get(symbol, symbol) in before}
        cutoffs = ({} if self.checked is None else dict.fromkeys(known, self.checked)) | {
            symbol: date.fromisoformat(watch.day) for symbol, watch in self.watches.items() if watch.day is not None
        }
        seen = {symbol: self.watches[symbol].filed for symbol in held if self.watches[symbol].filed is not None}
        self.filings = latest_filings(ctx, watched)
        return sorted(
            {
                symbol
                for symbol, filed in self.filings.items()
                if (filed.isoformat() != seen[symbol] if symbol in seen else filed >= cutoffs.get(symbol, filed))
            }
            | {symbol for symbol in held if symbol not in records}
        )

    def rank(
        self,
        ctx: Context,
        held: list[str],
        members: set[str] | None,
        assessed: Mapping[str, Assessment],
        memory: Memory,
    ) -> Steadiness:
        memory.spreads |= {symbol: seen.roc_cv for symbol, seen in assessed.items()}
        if members is not None:
            memory.spreads = {symbol: memory.spreads[symbol] for symbol in members.union(assessed)}
            ctx.state[STEADINESS] = list(steadiness_of(memory.spreads.values()))
            self.checked = ctx.session.date
            memory.members = {self.checked: members}
            self.watches = {
                symbol: self.watches[symbol]._replace(
                    before=memory.spreads.get(symbol), after=memory.spreads.get(symbol)
                )
                for symbol in held
            }
            self.origins = {}
        else:
            self.watches |= {
                symbol: self.watches[symbol]._replace(after=seen.roc_cv) for symbol, seen in assessed.items()
            }
        return rescored(Steadiness(*ctx.state[STEADINESS]), self.watches.values())

    def save(self, ctx: Context, held: list[str]) -> None:
        stepped = ctx.session.date.isoformat()
        if self.checked is not None:
            ctx.state[CHECKED] = self.checked.isoformat()
        ctx.state[WATCHED] = {
            symbol: list(
                watch._replace(
                    day=stepped, filed=filed.isoformat() if (filed := self.filings.get(symbol)) else watch.filed
                )
                if symbol in held
                else watch
            )
            for symbol, watch in self.watches.items()
        }
        ctx.state[ORIGINS] = self.origins


def requalified(ctx: Context, held: list[str], rules: Rules, memory: Memory) -> Review:
    today = ctx.session.date
    records = {symbol: Record(*fields) for symbol, fields in renamed(ctx, ctx.state.get("records", {})).items()}
    watchlist = Watchlist.load(ctx, held, records)
    memory.spreads = renamed(ctx, memory.spreads)
    members = memory.members_on(ctx, today) if watchlist.checked is None or week_ends(ctx) else None
    if members is not None:
        records = {symbol: record for symbol, record in records.items() if symbol in members or symbol in held}
    fresh = watchlist.fresh(ctx, held, records, members, memory)
    unscored = set[str]() if members is None else members.difference(memory.spreads)
    scored = sorted(unscored.union(fresh))
    assessed = assess(ctx, scored, rules) if scored else {}
    steadiness = watchlist.rank(ctx, held, members, assessed, memory)
    outcomes = judge(ctx, {symbol: assessed[symbol] for symbol in fresh}, steadiness, rules) if fresh else {}
    for symbol, outcome in outcomes.items():
        if isinstance(outcome, Record):
            records[symbol] = outcome
        else:
            records.pop(symbol, None)
    if outcomes:
        passed = sorted(symbol for symbol, outcome in outcomes.items() if isinstance(outcome, Record))
        ctx.log(f"{len(outcomes)} names requalified on new filings, {len(passed)} pass: {', '.join(passed[:20])}")
    since = today - timedelta(days=round(RESTATEMENT_YEARS * DAYS_PER_YEAR))
    flagged = restated(ctx, sorted(records), since)
    for symbol in flagged:
        records.pop(symbol, None)
    watchlist.save(ctx, held)
    ctx.state["records"] = {symbol: list(record) for symbol, record in records.items()}
    failures = {symbol: outcome for symbol, outcome in outcomes.items() if isinstance(outcome, str)}
    return Review(records, failures, flagged)


def prices_of(ctx: Context, records: Mapping[str, Record], min_yield: float) -> Prices:
    yields = owner_yields(ctx, records)
    rate = treasury_10y(ctx)
    if rate is None:
        return Prices(yields, min_yield, f"the hurdle {min_yield:.1%}", None)
    hurdle = max(min_yield, rate + float(ctx.params["yield_spread"]))
    return Prices(yields, hurdle, f"the hurdle {hurdle:.1%} (10-year Treasury {rate:.1%})", rate)


def thesis_sells(ctx: Context, held: list[str], review: Review, prices: Prices) -> list[Sell]:
    absurd = float(ctx.params["absurd_ratio"])
    sells: list[Sell] = []
    for symbol in held:
        found = prices.yields.get(symbol)
        if symbol in review.failures:
            sells.append(Sell(symbol, "thesis", f"{review.failures[symbol]}{months_held(ctx, symbol)}"))
        elif symbol in review.restated:
            why = f"8-K Item 4.02 restatement: earlier statements unreliable{months_held(ctx, symbol)}"
            sells.append(Sell(symbol, "thesis", why))
        elif prices.rate is not None and found is not None and found < prices.rate * absurd:
            why = (
                f"owner-earnings yield {found:.1%}, below {absurd:g}x the 10-year Treasury's {prices.rate:.1%}"
                f"{months_held(ctx, symbol)}"
            )
            sells.append(Sell(symbol, "valuation", why))
    return sells


def adds(ctx: Context, book: Book, records: Mapping[str, Record], prices: Prices) -> list[Buy]:
    buys: list[Buy] = []
    for symbol in book.remaining:
        found = prices.yields.get(symbol)
        weight = book.weights.get(symbol)
        if symbol not in records or weight is None or found is None or found < prices.hurdle:
            continue
        target = ladder_weight(ctx, book.ladder, symbol, weight)
        if target is None or target - weight > book.free:
            continue
        book.free -= target - weight
        cost = float(ctx.position(symbol).avg_cost)
        why = (
            f"add on weakness: close {ctx.bars[symbol].close / cost - 1:.0%} from the average cost {cost:.2f}, "
            f"still qualifies, owner-earnings yield {found:.1%} at or above {prices.hurdle_why}"
        )
        buys.append(Buy(symbol, target, why))
    return buys


def swap_out(ctx: Context, book: Book, yields: Mapping[str, float], symbol: str, busy: set[str]) -> Sell | None:
    ratio = float(ctx.params["swap_ratio"])
    weakest = min(
        (held for held in book.remaining if held in yields and held in book.weights and held not in busy),
        key=lambda held: (yields[held], held),
        default=None,
    )
    if weakest is None or yields[weakest] * ratio > yields[symbol]:
        return None
    if book.ladder.start > book.free + book.weights[weakest]:
        return None
    why = (
        f"{symbol} yields {yields[symbol]:.1%} in owner earnings, at least {ratio:g}x this holding's "
        f"{yields[weakest]:.1%}{months_held(ctx, weakest)}"
    )
    return Sell(weakest, "swap", why)


def new_names(
    ctx: Context, book: Book, records: Mapping[str, Record], prices: Prices, busy: set[str]
) -> tuple[list[Sell], list[Buy]]:
    quality = percentiles({symbol: record.roe for symbol, record in records.items()})

    def gate(symbol: str) -> float | None:
        found = prices.yields.get(symbol)
        return None if found is None or found < prices.hurdle else found * quality[symbol]

    owned = {(records[held].fiscal_end, records[held].owner_earnings) for held in book.remaining if held in records}
    slots = int(ctx.params["max_names"]) - len(book.remaining)
    ranked = candidates([symbol for symbol in records if symbol not in book.held], gate)
    buys: list[Buy] = []
    for rank, (symbol, _) in enumerate(ranked, 1):
        record = records[symbol]
        if (record.fiscal_end, record.owner_earnings) in owned:
            continue
        why = (
            f"{qualification(record, int(ctx.params['record_years']))}; owner-earnings yield "
            f"{prices.yields[symbol]:.1%} at or above {prices.hurdle_why}, rank {rank} of {len(ranked)} "
            "by yield x ROE percentile"
        )
        if slots > 0 and book.ladder.start <= book.free:
            buys.append(Buy(symbol, book.ladder.start, why))
            owned.add((record.fiscal_end, record.owner_earnings))
            slots -= 1
            book.free -= book.ladder.start
            continue
        swapped = swap_out(ctx, book, prices.yields, symbol, busy)
        if swapped is None:
            return [], buys
        return [swapped], [*buys, Buy(symbol, book.ladder.start, f"{why}, in place of {swapped.symbol}")]
    return [], buys


def owner(ctx: Context, memory: Memory) -> None:
    params = ctx.params
    cash_symbol = str(params["cash_symbol"])
    rules = rules_of(params)
    held = held_stocks(ctx, cash_symbol)
    review = requalified(ctx, held, rules, memory)
    prices = prices_of(ctx, review.records, rules.min_yield)
    sells = thesis_sells(ctx, held, review, prices)
    sold = {sell.symbol for sell in sells}
    remaining = [symbol for symbol in held if symbol not in sold]
    weights = stock_weights(ctx, cash_symbol)
    ladder = Ladder(
        float(params["start_weight"]), float(params["add_weight"]), float(params["add_on_drop"]), float(params["cap"])
    )
    book = Book(held, remaining, weights, ladder, 1 - sum(weights.get(symbol, 0.0) for symbol in remaining))
    buys = adds(ctx, book, review.records, prices)
    swaps, entries = new_names(ctx, book, review.records, prices, {buy.symbol for buy in buys})
    if sells or swaps or buys or entries or float(ctx.portfolio.cash) > PARK_ABOVE * float(ctx.portfolio.equity):
        trade(ctx, [*sells, *swaps], [*buys, *entries], cash_symbol)


class Buffett(Bot):
    def __init__(self) -> None:
        self.cache: AnnualCache = {}
        self.memory = Memory()

    def on_start(self, ctx: Context) -> None:
        variant = ctx.params["variant"]
        if variant not in VARIANTS:
            raise ValueError(f"variant must be one of {VARIANTS}, got {variant!r}")

    def on_step(self, ctx: Context) -> None:
        if ctx.params["variant"] == "systematic":
            rebalance(ctx, self.cache)
        else:
            owner(ctx, self.memory)


if __name__ == "__main__":
    run(Buffett)
