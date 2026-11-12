from datetime import date, timedelta

import numpy as np
import numpy.typing as npt

from buizz import Bot, Context, History, run

SESSIONS_PER_WEEK = 5
WEEKS_PER_YEAR = 52
DAYS_FROM_EPOCH_TO_MONDAY = 3


def completed_week_closes(history: History, today: date, week_ends_today: bool) -> npt.NDArray[np.float64]:
    days = history.start.astype("datetime64[ns]").astype("datetime64[D]")
    if days.size == 0 or days[-1] != np.datetime64(today):
        return np.empty(0)
    weeks = (days.astype(np.int64) + DAYS_FROM_EPOCH_TO_MONDAY) // 7
    return history.close[np.append(weeks[1:] != weeks[:-1], week_ends_today)]


class LowVolatility(Bot):
    def on_step(self, ctx: Context) -> None:
        today = ctx.session.date
        following = ctx.calendar(today, today + timedelta(days=10))[1].date
        if following.month == today.month:
            return
        week_ends_today = following.isocalendar().week != today.isocalendar().week
        weeks: int = ctx.params["vol_weeks"]
        decile = float(ctx.params["decile"])
        members = sorted(ctx.universe)
        history = ctx.history(members, "1d", SESSIONS_PER_WEEK * (weeks + 2), adjust="all")
        volatility: dict[str, float] = {}
        short: dict[str, int] = {}
        for symbol in members:
            returns = np.diff(np.log(completed_week_closes(history[symbol], today, week_ends_today)))[-weeks:]
            if returns.size < weeks:
                short[symbol] = returns.size
            else:
                volatility[symbol] = float(np.std(returns, ddof=1) * np.sqrt(WEEKS_PER_YEAR))
        if not volatility:
            return
        ranked = sorted(volatility, key=lambda symbol: (volatility[symbol], symbol))
        count = max(1, round(decile * len(ranked)))
        rank = {symbol: place for place, symbol in enumerate(ranked, start=1)}
        window = f"{weeks} weekly returns to {today}"
        targets = dict.fromkeys(ranked[:count], 1 / count)
        reasons = {
            symbol: f"vol {volatility[symbol]:.1%} ranks {rank[symbol]} of {len(ranked)}, "
            f"in the lowest {count} by {window}"
            for symbol in targets
        }
        for symbol in ctx.portfolio.positions:
            if symbol in targets:
                continue
            targets[symbol] = 0.0
            if symbol in volatility:
                reasons[symbol] = (
                    f"vol {volatility[symbol]:.1%} ranks {rank[symbol]} of {len(ranked)}, "
                    f"outside the lowest {count} by {window}"
                )
            elif symbol in short:
                reasons[symbol] = f"has {short[symbol]} of {window}"
            else:
                reasons[symbol] = f"not an S&P 500 member with bars on {today}"
        ctx.rebalance(targets, reason=reasons, exit_reason="not in the lowest-volatility decile")


if __name__ == "__main__":
    run(LowVolatility)
