from buizz_bots.monthly import MAX_SESSIONS_PER_MONTH, anchor_session, trailing_return
from buizz_bots.sessions import last_session_of_month

from buizz import Bot, Context, run


class DualMomentum(Bot):
    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        today = ctx.session.date
        us: str = ctx.params["us"]
        intl: str = ctx.params["intl"]
        cash: str = ctx.params["cash"]
        bonds: str = ctx.params["bonds"]
        months = int(ctx.params["lookback_months"])
        if (since := anchor_session(ctx, months)) is None:
            ctx.log(f"the calendar has no session {months} months back, no {months}-month signal yet", level="warn")
            return
        history = ctx.history([us, intl, cash], "1d", MAX_SESSIONS_PER_MONTH * (months + 1), adjust="all")
        found = {symbol: trailing_return(history[symbol], since, today) for symbol in (us, intl, cash)}
        returns = {symbol: value for symbol, value in found.items() if value is not None}
        if missing := sorted(found.keys() - returns.keys()):
            ctx.log(f"no {months}-month total return since the {since} close for {missing}, no signal", level="warn")
            return
        trend = f"{months}m total return {us} {returns[us]:+.1%} vs T-bills ({cash}) {returns[cash]:+.1%}"
        if returns[us] > returns[cash]:
            pick = intl if returns[intl] > returns[us] else us
            reason = (
                f"{trend}, equities beat T-bills; {pick} leads ({us} {returns[us]:+.1%}, {intl} {returns[intl]:+.1%})"
            )
        else:
            pick = bonds
            reason = f"{trend}, equities at or below T-bills: to bonds"
        others = [symbol for symbol in (us, intl, bonds) if symbol != pick]
        ctx.rebalance(
            {**dict.fromkeys(others, 0.0), pick: 1.0},
            reason={**{symbol: f"{pick} is the pick, not {symbol}: {reason}" for symbol in others}, pick: reason},
            exit_reason="not one of the three GEM assets",
        )


if __name__ == "__main__":
    run(DualMomentum)
