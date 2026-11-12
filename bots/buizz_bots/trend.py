import numpy as np

from buizz import Context
from buizz_bots.monthly import MAX_SESSIONS_PER_MONTH, month_end_closes, session_days

type Trend = tuple[float, float]


def daily_trend(ctx: Context, symbol: str, sessions: int) -> Trend | None:
    history = ctx.history([symbol], "1d", sessions, adjust="all")[symbol]
    if history.close.size < sessions or session_days(history)[-1] != np.datetime64(ctx.session.date):
        return None
    return float(history.close[-1]), float(history.close.mean())


def month_end_trend(ctx: Context, symbol: str, months: int) -> Trend | None:
    history = ctx.history([symbol], "1d", MAX_SESSIONS_PER_MONTH * months, adjust="all")[symbol]
    closes = month_end_closes(history, ctx.session.date)[-months:]
    return None if closes.size < months else (float(closes[-1]), float(closes.mean()))
