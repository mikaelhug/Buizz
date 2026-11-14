from datetime import date, timedelta
from typing import NamedTuple
from zoneinfo import ZoneInfo

import numpy as np
from buizz_bots.fundamentals import DOMESTIC_FORMS, QUARTER_TOLERANCE_DAYS, by_symbol, in_dollars, with_fourth_quarters
from buizz_bots.insiders import sessions_since
from buizz_bots.quality_momentum import standardized_surprise

from buizz import Bot, Context, EarningsRelease, FundamentalPoint, History, run, split_ratio

NEW_YORK = ZoneInfo("America/New_York")
BREAKPOINT_DAYS = 91
MIN_BREAKPOINT_EVENTS = 100
MAX_QUARTER_AGE_DAYS = 100
EPS_HISTORY_DAYS = 3 * 365 + 2 * MAX_QUARTER_AGE_DAYS
ENTRY_CALENDAR_DAYS_PER_SESSION = 2
SIDES = {1: "top", -1: "bottom"}


class Event(NamedTuple):
    symbol: str
    accession: str
    announced: date
    filed: date


class Surprise(NamedTuple):
    quarter: date
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
    quarters = {point.period_end: (value, point.filed) for point, value in adjusted if point.period_type == "Q"}
    years = {point.period_end: (point, value) for point, value in adjusted if point.period_type == "FY"}
    filled = with_fourth_quarters(
        {end: value for end, (value, _) in quarters.items()},
        [(point.period_start or end, end, value) for end, (point, value) in years.items()],
    )
    return {end: quarters[end] if end in quarters else (value, years[end][0].filed) for end, value in filled.items()}


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
    return None if sue is None else Surprise(quarter, sue)


class PeadSue(Bot):
    def __init__(self) -> None:
        self.sues: list[tuple[date, float]] = []
        self.done: set[str] = set()
        self.started = False

    def breakpoints(self, today: date, decile: float) -> tuple[float, float] | None:
        start = today - timedelta(days=BREAKPOINT_DAYS)
        self.sues = [(day, value) for day, value in self.sues if day >= start]
        values = [value for day, value in self.sues if day < today]
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
        held: dict[str, list[str | int | float]] = ctx.state.setdefault("held", {})
        lookback = entry_lag * ENTRY_CALENDAR_DAYS_PER_SESSION if self.started else BREAKPOINT_DAYS
        self.started = True
        since = today - timedelta(days=lookback)
        events = [
            event
            for event in announcements(ctx.earnings(sorted(ctx.universe)), since)
            if event.accession not in self.done
        ]
        age = sessions_since(ctx, {event.accession: event.announced for event in events})
        measured = self.surprises(ctx, events) if events else {}
        cutoffs = self.breakpoints(today, float(ctx.params["decile"]))
        for event in events:
            found = measured.get(event.accession)
            if found is None:
                if age[event.accession] >= entry_lag:
                    self.done.add(event.accession)
                continue
            self.done.add(event.accession)
            self.sues.append((event.announced, found.sue))
            if cutoffs is None or age[event.accession] > entry_lag:
                continue
            side = 1 if found.sue >= cutoffs[1] else -1 if found.sue <= cutoffs[0] else 0
            if side:
                held[event.symbol] = [side, event.announced.isoformat(), round(found.sue, 4), found.quarter.isoformat()]
            elif event.symbol in held:
                del held[event.symbol]
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
