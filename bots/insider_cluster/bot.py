from bisect import bisect_right
from collections import defaultdict
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import date, datetime, timedelta
from decimal import Decimal
from itertools import groupby

from buizz import Bot, Context, InsiderTrade, run

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


def routine(months: set[tuple[int, int]], day: date, years: int) -> bool:
    return all((day.year - back, day.month) in months for back in range(1, years + 1))


def cluster(
    symbol: str,
    found: list[Filing],
    months: dict[int, set[tuple[int, int]]],
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
                cik: name for cik, name in filing.insiders.items() if not routine(months.get(cik, set()), day, years)
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
    history: list[InsiderTrade],
    since: date,
    years: int,
    session_open: datetime,
) -> list[Cluster]:
    months: dict[str, dict[int, set[tuple[int, int]]]] = {
        symbol: traded_months(rows) for symbol, rows in groupby(history, key=lambda trade: trade.symbol)
    }
    found: list[Cluster] = []
    for symbol, rows in groupby(trades, key=lambda trade: trade.symbol):
        formed = cluster(symbol, standing(filings(rows)), months.get(symbol, {}), since, years, session_open)
        if formed is not None:
            found.append(formed)
    return found


def sessions_since(ctx: Context, signals: dict[str, date]) -> dict[str, int]:
    if not signals:
        return {}
    today = ctx.session.date
    dates = [session.date for session in ctx.calendar(min(signals.values()), today)]
    return {symbol: bisect_right(dates, today) - bisect_right(dates, signal) for symbol, signal in signals.items()}


class InsiderCluster(Bot):
    def on_step(self, ctx: Context) -> None:
        cash: str = ctx.params["cash"]
        max_names = int(ctx.params["max_names"])
        hold = int(ctx.params["hold_sessions"])
        window = int(ctx.params["window_days"])
        min_insiders = int(ctx.params["min_insiders"])
        min_value = Decimal(str(ctx.params["min_value"]))
        years = int(ctx.params["routine_years"])
        today = ctx.session.date
        held = {symbol for symbol, position in ctx.portfolio.positions.items() if position.qty != 0 and symbol != cash}
        signals = {
            symbol: date.fromisoformat(day) for symbol, day in ctx.state.get("signals", {}).items() if symbol in held
        }
        held_for = sessions_since(ctx, {symbol: signals[symbol] for symbol in held if symbol in signals})
        exits = sorted(symbol for symbol in held if held_for.get(symbol, hold) >= hold)
        staying = held - set(exits)

        since = today - timedelta(days=window)

        def signals_today(found: Cluster) -> bool:
            return found.fresh and len(found.insiders) >= min_insiders and found.value >= min_value

        trades = ctx.insider_trades(ctx.universe, since)
        loose = {
            found.symbol
            for found in clusters(trades, [], since, years, ctx.session.open)
            if found.symbol not in held and signals_today(found)
        }
        history = ctx.insider_trades(sorted(loose), date(since.year - years, since.month, 1)) if loose else []
        entries = sorted(
            (
                found
                for found in clusters(
                    [trade for trade in trades if trade.symbol in loose], history, since, years, ctx.session.open
                )
                if signals_today(found)
            ),
            key=lambda found: (-found.value, found.symbol),
        )[: max_names - len(staying)]
        for found in entries:
            signals[found.symbol] = today
        ctx.state["signals"] = {symbol: day.isoformat() for symbol, day in signals.items()}

        if not entries and not exits and ctx.position(cash).qty != 0:
            return
        equity = float(ctx.portfolio.equity)
        kept = sum(
            ctx.position(symbol).qty * ctx.bars[symbol].close / equity if symbol in ctx.bars else 1 / max_names
            for symbol in staying
        )
        room = max(0.0, 1 - kept)
        weight = min(1 / max_names, room / len(entries)) if entries else 0.0
        parked = max(0.0, room - weight * len(entries))
        for symbol in exits:
            ctx.order_target_weight(symbol, 0, reason=f"held {held_for.get(symbol, hold)} sessions since its cluster")
        parked_qty = ctx.position(cash).qty
        if cash not in ctx.bars or int(parked * equity / ctx.bars[cash].close) != parked_qty:
            ctx.order_target_weight(
                cash, parked, reason=f"idle cash: {len(staying) + len(entries)} of {max_names} slots taken"
            )
        for found in entries:
            buyers = ", ".join(sorted(found.insiders.values()))
            reason = (
                f"{len(found.insiders)} insiders ({buyers}) bought ${found.value:,.0f} "
                f"in the open market between {found.first} and {found.last}"
            )
            ctx.log(f"cluster in {found.symbol}: {reason}")
            ctx.order_target_weight(found.symbol, weight, reason=reason)


if __name__ == "__main__":
    run(InsiderCluster)
