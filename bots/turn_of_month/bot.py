from datetime import date, timedelta

from buizz import Bot, Context, run

STOCKS = "SPY"
BILLS = "BIL"
CALENDAR_LOOKAHEAD = timedelta(days=45)


def turn_of_month_day(sessions: list[date], day: date, first_day: int, last_day: int) -> int | None:
    month = [session for session in sessions if (session.year, session.month) == (day.year, day.month)]
    from_start = month.index(day) + 1
    from_end = from_start - len(month) - 1
    if from_start <= last_day:
        return from_start
    if sessions[-1] <= month[-1]:
        raise RuntimeError(f"the exchange calendar ends before the month after {day} begins")
    if from_end >= first_day:
        return from_end
    return None


class TurnOfMonth(Bot):
    def on_step(self, ctx: Context) -> None:
        first_day: int = ctx.params["first_day"]
        last_day: int = ctx.params["last_day"]
        today = ctx.session.date
        sessions = [session.date for session in ctx.calendar(today.replace(day=1), today + CALENDAR_LOOKAHEAD)]
        earning = sessions[sessions.index(today) + 2]
        day = turn_of_month_day(sessions, earning, first_day, last_day)
        if day is None:
            hold, drop = BILLS, STOCKS
            reason = f"{earning} is outside the turn of the month, trading days {first_day:+d} to {last_day:+d}"
        else:
            hold, drop = STOCKS, BILLS
            reason = f"{earning} is trading day {day:+d} of the turn of the month"
        if ctx.position(drop).qty != 0:
            ctx.order_target_weight(drop, 0, at="close", reason=reason)
        if ctx.position(hold).qty == 0:
            ctx.order_target_weight(hold, ctx.params["weight"], at="close", reason=reason)


if __name__ == "__main__":
    run(TurnOfMonth)
