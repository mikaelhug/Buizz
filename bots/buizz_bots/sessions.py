from datetime import timedelta

from buizz import Context


def last_session_of_month(ctx: Context) -> bool:
    today = ctx.session.date
    return ctx.calendar(today, today + timedelta(days=10))[1].date.month != today.month
