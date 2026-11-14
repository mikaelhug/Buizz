from bisect import bisect_right
from collections import defaultdict
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import date, datetime, timedelta
from decimal import Decimal
from itertools import groupby

from buizz import Context, InsiderTrade

OPEN_MARKET = {"P", "S"}


@dataclass(frozen=True)
class Filing:
    accession: str
    form: str
    available_at: datetime
    owners: frozenset[int]
    insiders: dict[int, str]
    dates: frozenset[date]
    purchases: tuple[tuple[date, Decimal], ...]


@dataclass(frozen=True)
class Cluster:
    symbol: str
    insiders: dict[int, str]
    value: Decimal
    first: date
    last: date
    fresh: bool


def filings(trades: Iterable[InsiderTrade]) -> list[Filing]:
    found: list[Filing] = []
    for accession, rows in groupby(trades, key=lambda trade: trade.accession):
        lines = list(rows)
        head = lines[0]
        found.append(
            Filing(
                accession=accession,
                form=head.form,
                available_at=head.available_at,
                owners=frozenset(owner.cik for owner in head.owners),
                insiders={owner.cik: owner.name for owner in head.owners if owner.officer or owner.director},
                dates=frozenset(line.date for line in lines),
                purchases=tuple(
                    (line.date, line.shares * line.price)
                    for line in lines
                    if line.code == "P" and line.acquired and line.shares is not None and line.price is not None
                ),
            )
        )
    return found


def standing(found: list[Filing]) -> list[Filing]:
    kept: list[Filing] = []
    for filing in found:
        if filing.form == "4/A":
            amended = next(
                (
                    earlier
                    for earlier in reversed(kept)
                    if earlier.owners == filing.owners and earlier.dates & filing.dates
                ),
                None,
            )
            if amended is not None:
                kept.remove(amended)
        kept.append(filing)
    return kept


def traded_months(trades: Iterable[InsiderTrade]) -> dict[int, set[tuple[int, int]]]:
    months: dict[int, set[tuple[int, int]]] = defaultdict(set)
    for trade in trades:
        if trade.code in OPEN_MARKET:
            for owner in trade.owners:
                months[owner.cik].add((trade.date.year, trade.date.month))
    return months


def opportunistic(months: set[tuple[int, int]], year: int, years: int) -> bool:
    prior = range(year - years, year)
    traded_every_year = all(any((past, month) in months for month in range(1, 13)) for past in prior)
    routine = any(all((past, month) in months for past in prior) for month in range(1, 13))
    return traded_every_year and not routine


def cluster(
    symbol: str,
    found: list[Filing],
    months: dict[int, set[tuple[int, int]]] | None,
    since: date,
    years: int,
    session_open: datetime,
) -> Cluster | None:
    insiders: dict[int, str] = {}
    value = Decimal(0)
    days: list[date] = []
    fresh = False
    for filing in found:
        for day, amount in filing.purchases:
            if day < since:
                continue
            buyers = {
                cik: name
                for cik, name in filing.insiders.items()
                if months is None or opportunistic(months.get(cik, set()), day.year, years)
            }
            if buyers:
                insiders |= buyers
                value += amount
                days.append(day)
                fresh |= filing.available_at >= session_open
    if not days:
        return None
    return Cluster(symbol, insiders, value, min(days), max(days), fresh)


def clusters(
    trades: list[InsiderTrade],
    history: list[InsiderTrade] | None,
    since: date,
    years: int,
    session_open: datetime,
) -> list[Cluster]:
    months: dict[str, dict[int, set[tuple[int, int]]]] = {
        symbol: traded_months(rows) for symbol, rows in groupby(history or [], key=lambda trade: trade.symbol)
    }
    found: list[Cluster] = []
    for symbol, rows in groupby(trades, key=lambda trade: trade.symbol):
        own = None if history is None else months.get(symbol, {})
        formed = cluster(symbol, standing(filings(rows)), own, since, years, session_open)
        if formed is not None:
            found.append(formed)
    return found


def sessions_since(ctx: Context, signals: dict[str, date]) -> dict[str, int]:
    if not signals:
        return {}
    today = ctx.session.date
    dates = [session.date for session in ctx.calendar(min(signals.values()), today)]
    return {symbol: bisect_right(dates, today) - bisect_right(dates, signal) for symbol, signal in signals.items()}


def aged_out(ctx: Context, held: set[str]) -> tuple[dict[str, date], dict[str, int], list[str]]:
    hold = int(ctx.params["hold_sessions"])
    signals = {
        symbol: date.fromisoformat(day) for symbol, day in ctx.state.get("signals", {}).items() if symbol in held
    }
    held_for = sessions_since(ctx, {symbol: signals[symbol] for symbol in held if symbol in signals})
    return signals, held_for, sorted(symbol for symbol in held if held_for.get(symbol, hold) >= hold)


def fresh_clusters(ctx: Context, symbols: list[str], held: set[str], room: int) -> list[Cluster]:
    window = int(ctx.params["window_days"])
    min_insiders = int(ctx.params["min_insiders"])
    min_value = Decimal(str(ctx.params["min_value"]))
    years = int(ctx.params["routine_years"])
    since = ctx.session.date - timedelta(days=window)

    def signals_today(found: Cluster) -> bool:
        return found.fresh and len(found.insiders) >= min_insiders and found.value >= min_value

    trades = ctx.insider_trades(symbols, since)
    loose = {
        found.symbol
        for found in clusters(trades, None, since, years, ctx.session.open)
        if found.symbol not in held and signals_today(found)
    }
    history = ctx.insider_trades(sorted(loose), date(since.year - years, 1, 1)) if loose else []
    return sorted(
        (
            found
            for found in clusters(
                [trade for trade in trades if trade.symbol in loose], history, since, years, ctx.session.open
            )
            if signals_today(found)
        ),
        key=lambda found: (-found.value, found.symbol),
    )[:room]


def remember(ctx: Context, signals: dict[str, date], entries: list[Cluster]) -> None:
    for found in entries:
        signals[found.symbol] = ctx.session.date
    ctx.state["signals"] = {symbol: day.isoformat() for symbol, day in signals.items()}


def buy_reason(ctx: Context, found: Cluster) -> str:
    buyers = ", ".join(sorted(found.insiders.values()))
    reason = (
        f"{len(found.insiders)} insiders ({buyers}) bought ${found.value:,.0f} "
        f"in the open market between {found.first} and {found.last}"
    )
    ctx.log(f"cluster in {found.symbol}: {reason}")
    return reason
