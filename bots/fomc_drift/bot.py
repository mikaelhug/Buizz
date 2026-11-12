from datetime import timedelta
from zoneinfo import ZoneInfo

from buizz_bots.sessions import next_session

from buizz import Bot, Context, run

NEW_YORK = ZoneInfo("America/New_York")
ENTRY_BEFORE_CLOSE = timedelta(minutes=15)


class FomcDrift(Bot):
    def on_step(self, ctx: Context) -> None:
        asset: str = ctx.params["asset"]
        today = ctx.session.date
        if ctx.clock >= ctx.session.close:
            return
        if ctx.position(asset).qty != 0:
            statement = next(
                (meeting.statement_at for meeting in ctx.fomc(today, today) if meeting.scheduled),
                None,
            )
            if statement is None:
                ctx.order_target_weight(asset, 0, reason=f"no scheduled FOMC statement on {today}, out")
            elif ctx.clock >= statement:
                ctx.order_target_weight(
                    asset, 0, reason=f"FOMC statement out at {statement.astimezone(NEW_YORK):%H:%M} ET, drift over"
                )
            return
        if ctx.clock != ctx.session.close - ENTRY_BEFORE_CLOSE or (following := next_session(ctx)) is None:
            return
        for meeting in ctx.fomc(following, following):
            if meeting.scheduled and meeting.statement_at is not None:
                ctx.order_target_weight(
                    asset,
                    1.0,
                    at="close",
                    reason=(
                        f"scheduled FOMC statement on {following} at "
                        f"{meeting.statement_at.astimezone(NEW_YORK):%H:%M} ET: hold from today's close"
                    ),
                )
                return


if __name__ == "__main__":
    run(FomcDrift)
