from bisect import bisect_right
from datetime import date, timedelta
from typing import NamedTuple
from zoneinfo import ZoneInfo

import numpy as np
from buizz_bots.fundamentals import DOMESTIC_FORMS, QUARTER_TOLERANCE_DAYS, by_symbol, in_dollars, quarterly
from buizz_bots.insiders import sessions_since
from buizz_bots.quality_momentum import standardized_surprise
from buizz_bots.tickers import renamed

from buizz import Bot, Context, EarningsRelease, FundamentalPoint, History, run, split_ratio

NEW_YORK = ZoneInfo("America/New_York")
BREAKPOINT_DAYS = 91
MIN_BREAKPOINT_EVENTS = 100
MAX_QUARTER_AGE_DAYS = 100
EPS_HISTORY_DAYS = 3 * 365 + 2 * MAX_QUARTER_AGE_DAYS
ENTRY_CALENDAR_DAYS_PER_SESSION = 2
SIDES = {1: "top", -1: "bottom"}
STEPPED = "stepped"


class Event(NamedTuple):
    symbol: str
    accession: str
    announced: date
    filed: date


class Surprise(NamedTuple):
    quarter: date
    filed: date
    sue: float


class Measured(NamedTuple):
    announced: date
    public: date
    sue: float


def announcements(releases: list[EarningsRelease], since: date) -> list[Event]:
    days = [(release, release.accepted_at.astimezone(NEW_YORK).date()) for release in releases]
    return [Event(release.symbol, release.accession, day, release.filed) for release, day in days if day >= since]


def split_eps(points: list[FundamentalPoint], raw: History, split: History) -> list[tuple[FundamentalPoint, float]]:
    adjusted: list[tuple[FundamentalPoint, float]] = []
    for point in points:
        if point.period_start is None or point.form not in DOMESTIC_FORMS or not in_dollars(point):
            continue
        ratio = split_ratio(raw, split, point.filed)
        if ratio is not None and ratio > 0:
            adjusted.append((point, float(point.value) / ratio))
    return adjusted


def quarterly_with_filed(adjusted: list[tuple[FundamentalPoint, float]]) -> dict[date, tuple[float, date]]:
    filed = {point.period_end: point.filed for point, _ in adjusted if point.period_type == "FY"} | {
        point.period_end: point.filed for point, _ in adjusted if point.period_type == "Q"
    }
    return {end: (value, filed[end]) for end, value in quarterly(adjusted).items()}


def surprise(event: Event, eps: dict[date, tuple[float, date]], today: date) -> Surprise | None:
    reported = [
        end
        for end, (_, filed) in eps.items()
        if end < event.announced <= end + timedelta(days=MAX_QUARTER_AGE_DAYS) and filed >= event.filed
    ]
    if not reported:
        return None
    quarter = max(reported)
    known = {end: value for end, (value, _) in eps.items() if end <= quarter + timedelta(days=QUARTER_TOLERANCE_DAYS)}
    sue = standardized_surprise(known, today)
    return None if sue is None else Surprise(quarter, eps[quarter][1], sue)


def public_lag(sessions: list[date], announced: date, filed: date) -> tuple[date, int] | None:
    public = bisect_right(sessions, filed)
    if public == len(sessions):
        return None
    return sessions[public], public + 1 - bisect_right(sessions, announced)


def decide(
    held: dict[str, list[str | int | float]], undecided: list[tuple[Event, Surprise]], cutoffs: tuple[float, float]
) -> None:
    low, high = cutoffs
    for event, found in undecided:
        side = 1 if found.sue >= high else -1 if found.sue <= low else 0
        if side:
            held[event.symbol] = [side, event.announced.isoformat(), round(found.sue, 4), found.quarter.isoformat()]
        elif event.symbol in held:
            del held[event.symbol]


class PeadSue(Bot):
    def __init__(self) -> None:
        self.sues: list[Measured] = []
        self.done: set[str] = set()
        self.started = False

    def breakpoints(self, today: date, decile: float) -> tuple[float, float] | None:
        start = today - timedelta(days=BREAKPOINT_DAYS)
        self.sues = [found for found in self.sues if found.announced >= start]
        values = [found.sue for found in self.sues if found.public < today]
        if len(values) < MIN_BREAKPOINT_EVENTS:
            return None
        low, high = np.quantile(values, [decile, 1 - decile])
        return float(low), float(high)

    def surprises(self, ctx: Context, events: list[Event]) -> dict[str, Surprise]:
        today = ctx.session.date
        symbols = sorted({event.symbol for event in events})
        points = by_symbol(ctx.fundamentals(symbols, ["eps_diluted"]))
        bars = len(ctx.calendar(today - timedelta(days=EPS_HISTORY_DAYS), today)) + 1
        raw = ctx.history(symbols, "1d", bars, "raw")
        split = ctx.history(symbols, "1d", bars, "split")
        found: dict[str, Surprise] = {}
        for event in events:
            eps = quarterly_with_filed(split_eps(points[event.symbol], raw[event.symbol], split[event.symbol]))
            if (measured := surprise(event, eps, today)) is not None:
                found[event.accession] = measured
        return found

    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        hold = int(ctx.params["hold_sessions"])
        entry_lag = int(ctx.params["entry_lag_sessions"])
        held: dict[str, list[str | int | float]] = renamed(ctx, ctx.state.get("held", {}))
        ctx.state["held"] = held
        lookback = entry_lag * ENTRY_CALENDAR_DAYS_PER_SESSION if self.started else BREAKPOINT_DAYS
        self.started = True
        recent = announcements(ctx.earnings(sorted(ctx.universe)), today - timedelta(days=lookback))
        self.done &= {event.accession for event in recent}
        events = [event for event in recent if event.accession not in self.done]
        age = sessions_since(ctx, {event.accession: event.announced for event in events})
        measured = self.surprises(ctx, events) if events else {}
        first = min((event.announced for event in events), default=today)
        sessions = [session.date for session in ctx.calendar(first, today)]
        stepped = ctx.state.get(STEPPED)
        ctx.state[STEPPED] = today.isoformat()
        undecided: list[tuple[Event, Surprise]] = []
        for event in events:
            found = measured.get(event.accession)
            if found is None:
                if age[event.accession] >= entry_lag:
                    self.done.add(event.accession)
                continue
            self.done.add(event.accession)
            published = public_lag(sessions, event.announced, found.filed)
            if published is None:
                continue
            public, lag = published
            if lag <= entry_lag:
                self.sues.append(Measured(event.announced, public, found.sue))
            if age[event.accession] <= entry_lag and (stepped is None or public > date.fromisoformat(stepped)):
                undecided.append((event, found))
        cutoffs = self.breakpoints(today, float(ctx.params["decile"]))
        if cutoffs is not None:
            decide(held, undecided, cutoffs)
        held_for = sessions_since(ctx, {symbol: date.fromisoformat(str(entry[1])) for symbol, entry in held.items()})
        for symbol in [symbol for symbol, sessions in held_for.items() if sessions >= hold]:
            del held[symbol]
        self.trade(ctx, held, cutoffs)

    def trade(
        self, ctx: Context, held: dict[str, list[str | int | float]], cutoffs: tuple[float, float] | None
    ) -> None:
        cap = float(ctx.params["max_weight"])
        legs = {side: [symbol for symbol, entry in held.items() if entry[0] == side] for side in SIDES}
        targets: dict[str, float] = {}
        reasons: dict[str, str] = {}
        for side, symbols in legs.items():
            for symbol in symbols:
                _, announced, sue, quarter = held[symbol]
                targets[symbol] = side * min(cap, 1 / len(symbols))
                cutoff = "" if cutoffs is None else f" (cutoffs {cutoffs[0]:.2f} / {cutoffs[1]:.2f})"
                reasons[symbol] = (
                    f"SUE {float(sue):.2f}, {SIDES[side]} decile{cutoff}: quarter to {quarter} announced {announced}"
                )
        ctx.rebalance(
            targets,
            reason=reasons,
            exit_reason=f"held {ctx.params['hold_sessions']} sessions since its announcement, or no longer extreme",
            band=float(ctx.params["drift_band"]),
        )


if __name__ == "__main__":
    run(PeadSue)
