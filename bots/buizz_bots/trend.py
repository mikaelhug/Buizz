from datetime import date

import numpy as np

from buizz import Context, History
from buizz_bots.monthly import MAX_SESSIONS_PER_MONTH, month_end_closes, session_days

type Trend = tuple[float, float]


def sma_trend(history: History, today: date, sessions: int) -> Trend | None:
    closes = history.close[-sessions:]
    if closes.size < sessions or session_days(history)[-1] != np.datetime64(today):
        return None
    return float(closes[-1]), float(closes.mean())


def month_end_sma_trend(history: History, today: date, months: int) -> Trend | None:
    closes = month_end_closes(history, today)[-months:]
    return None if closes.size < months else (float(closes[-1]), float(closes.mean()))


def daily_trend(ctx: Context, symbol: str, sessions: int) -> Trend | None:
    history = ctx.history([symbol], "1d", sessions, adjust="all")[symbol]
    return sma_trend(history, ctx.session.date, sessions)


def month_end_trend(ctx: Context, symbol: str, months: int) -> Trend | None:
    history = ctx.history([symbol], "1d", MAX_SESSIONS_PER_MONTH * months, adjust="all")[symbol]
    return month_end_sma_trend(history, ctx.session.date, months)
