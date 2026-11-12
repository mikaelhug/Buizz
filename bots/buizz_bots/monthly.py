from collections.abc import Callable
from datetime import UTC, date, datetime, timedelta
from typing import Any

import numpy as np
import numpy.typing as npt

from buizz import Context, History

MAX_SESSIONS_PER_MONTH = 23
MOMENTUM_MONTHS = 12


def session_days(history: History) -> npt.NDArray[np.datetime64]:
    return history.start.astype("datetime64[ns]").astype("datetime64[D]")


def last_session(history: History) -> date:
    return datetime.fromtimestamp(int(history.start[-1]) / 1e9, UTC).date()


def period_end_closes(
    history: History,
    today: date,
    key: Callable[[npt.NDArray[np.datetime64]], npt.NDArray[Any]],
    include_today: bool,
) -> npt.NDArray[np.float64]:
    days = session_days(history)
    if days.size == 0 or days[-1] != np.datetime64(today):
        return np.empty(0)
    periods = key(days)
    return history.close[np.append(periods[1:] != periods[:-1], include_today)]


def month_end_closes(history: History, today: date) -> npt.NDArray[np.float64]:
    return period_end_closes(history, today, lambda days: days.astype("datetime64[M]"), True)


def first_of_month(day: date, months_back: int) -> date:
    index = day.year * 12 + day.month - 1 - months_back
    return date(index // 12, index % 12 + 1, 1)


def anchor_session(ctx: Context, months: int) -> date | None:
    today = ctx.session.date
    sessions = ctx.calendar(first_of_month(today, months), first_of_month(today, months - 1) - timedelta(days=1))
    return sessions[-1].date if sessions else None


def close_on(history: History, day: date) -> float | None:
    found = np.flatnonzero(session_days(history) == np.datetime64(day))
    return float(history.close[found[0]]) if found.size else None


def trailing_return(history: History, since: date, today: date) -> float | None:
    start, end = close_on(history, since), close_on(history, today)
    return None if start is None or end is None else end / start - 1


def momentum(history: History, today: date) -> float | None:
    closes = month_end_closes(history, today)
    if closes.size < MOMENTUM_MONTHS + 1 or closes[-MOMENTUM_MONTHS - 1] <= 0:
        return None
    return float(closes[-2] / closes[-MOMENTUM_MONTHS - 1] - 1)


def momentum_sessions(ctx: Context) -> int:
    today = ctx.session.date
    return len(ctx.calendar(first_of_month(today, MOMENTUM_MONTHS), today))
