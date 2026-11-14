from datetime import date, datetime, timedelta
from zoneinfo import ZoneInfo

from buizz_bots.sessions import next_session

from buizz import Bot, Context, run

NEW_YORK = ZoneInfo("America/New_York")
WINDOW = timedelta(hours=24)
LAST_ENTRY_BEFORE_CLOSE = timedelta(minutes=15)


def statement(ctx: Context, day: date) -> datetime | None:
    return next(
        (
            meeting.statement_at
            for meeting in ctx.fomc(day, day)
            if meeting.scheduled and meeting.statement_at is not None
        ),
        None,
    )


def eastern(moment: datetime) -> str:
    return f"{moment.astimezone(NEW_YORK):%Y-%m-%d %H:%M} ET"


class FomcDrift(Bot):
    def on_step(self, ctx: Context) -> None:
        asset: str = ctx.params["asset"]
        today = ctx.session.date
        if ctx.clock >= ctx.session.close:
            return
        following = next_session(ctx)
        tomorrow = None if following is None else statement(ctx, following)
        if ctx.position(asset).qty != 0:
            released = statement(ctx, today)
            if released is not None and ctx.clock >= released:
                ctx.order_target_weight(asset, 0, reason=f"FOMC statement out at {eastern(released)}, drift over")
            elif released is None and tomorrow is None:
                ctx.order_target_weight(asset, 0, reason=f"no scheduled FOMC statement on {today} or the next session")
            return
        if tomorrow is None:
            return
        opens = tomorrow - WINDOW
        last_call = ctx.session.close - LAST_ENTRY_BEFORE_CLOSE
        if ctx.clock >= opens:
            ctx.order_target_weight(
                asset, 1.0, reason=f"24 hours before the scheduled FOMC statement at {eastern(tomorrow)}"
            )
        elif ctx.clock == last_call:
            ctx.order_target_weight(
                asset,
                1.0,
                at="close",
                reason=(
                    f"the session closes before 24 hours ahead of the FOMC statement at {eastern(tomorrow)}: "
                    "hold from its close"
                ),
            )


if __name__ == "__main__":
    run(FomcDrift)
