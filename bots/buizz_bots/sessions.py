from datetime import date, timedelta

from buizz import Context


def next_session(ctx: Context) -> date | None:
    today = ctx.session.date
    later = ctx.calendar(today, today + timedelta(days=10))[1:]
    return later[0].date if later else None


def last_session_of_month(ctx: Context) -> bool:
    following = next_session(ctx)
    return following is None or following.month != ctx.session.date.month


def week_ends(ctx: Context) -> bool:
    following = next_session(ctx)
    return following is None or following.isocalendar()[:2] != ctx.session.date.isocalendar()[:2]
