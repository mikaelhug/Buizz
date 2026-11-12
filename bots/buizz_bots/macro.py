from buizz import Context, MacroPoint, MacroSeries
from buizz_bots.monthly import first_of_month

RELEASE_LAG_MONTHS = 4


def latest_monthly(ctx: Context, series: MacroSeries, count: int) -> list[MacroPoint]:
    today = ctx.session.date
    return ctx.macro([series], first_of_month(today, count + RELEASE_LAG_MONTHS), today)[-count:]
