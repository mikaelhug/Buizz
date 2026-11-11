from datetime import date, timedelta

import numpy as np

from buizz import Bot, Context, History, run

MAX_SESSIONS_PER_MONTH = 23


def first_of_month(day: date, months_back: int) -> date:
    index = day.year * 12 + day.month - 1 - months_back
    return date(index // 12, index % 12 + 1, 1)


def total_return(history: History, since: date, today: date) -> float | None:
    days = history.start.astype("datetime64[ns]").astype("datetime64[D]")
    anchor = np.flatnonzero(days == np.datetime64(since))
    if anchor.size == 0 or days[-1] != np.datetime64(today):
        return None
    return float(history.close[-1] / history.close[anchor[0]] - 1)


class DualMomentum(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        if ctx.calendar(today, today + timedelta(days=10))[1].date.month == today.month:
            return
        us: str = ctx.params["us"]
        intl: str = ctx.params["intl"]
        cash: str = ctx.params["cash"]
        bonds: str = ctx.params["bonds"]
        months = int(ctx.params["lookback_months"])
        start = first_of_month(today, months)
        if not (start_month := ctx.calendar(start, first_of_month(today, months - 1) - timedelta(days=1))):
            ctx.log(f"the calendar has no session in {start:%Y-%m}, no {months}-month signal yet", level="warn")
            return
        since = start_month[-1].date
        history = ctx.history([us, intl, cash], "1d", MAX_SESSIONS_PER_MONTH * (months + 1), adjust="all")
        found = {symbol: total_return(history[symbol], since, today) for symbol in (us, intl, cash)}
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
        held = {symbol for symbol, position in ctx.portfolio.positions.items() if position.qty != 0}
        if held == {pick} and 0 <= float(ctx.portfolio.cash) < ctx.bars[pick].close:
            ctx.log(f"hold {pick}, fully invested: {reason}")
            return
        others = [symbol for symbol in (us, intl, bonds) if symbol != pick]
        ctx.rebalance(
            {**dict.fromkeys(others, 0.0), pick: 1.0},
            reason={**{symbol: f"{pick} is the pick, not {symbol}: {reason}" for symbol in others}, pick: reason},
            exit_reason="not one of the three GEM assets",
        )


if __name__ == "__main__":
    run(DualMomentum)
