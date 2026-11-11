from datetime import date

import numpy as np
import numpy.typing as npt
from buizz_bots.sessions import last_session_of_month

from buizz import Bot, Context, History, run

MAX_SESSIONS_PER_MONTH = 23


def month_end_closes(history: History, today: date) -> npt.NDArray[np.float64]:
    days = history.start.astype("datetime64[ns]").astype("datetime64[D]")
    if days.size == 0 or days[-1] != np.datetime64(today):
        return np.empty(0)
    months = days.astype("datetime64[M]")
    return history.close[np.append(months[1:] != months[:-1], True)]


class Gtaa5(Bot):
    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        today = ctx.session.date
        assets: list[str] = ctx.params["assets"]
        cash: str = ctx.params["cash"]
        months = int(ctx.params["sma_months"])
        sleeve = 1 / len(assets)
        history = ctx.history(assets, "1d", MAX_SESSIONS_PER_MONTH * months, adjust="all")
        targets = dict.fromkeys([*assets, cash], 0.0)
        reasons: dict[str, str] = {}
        for asset in assets:
            closes = month_end_closes(history[asset], today)[-months:]
            if closes.size < months:
                reasons[asset] = f"{asset} has {closes.size} of {months} month-end closes, to cash"
            elif closes[-1] > (sma := float(closes.mean())):
                targets[asset] = sleeve
                reasons[asset] = f"{asset} {closes[-1]:.2f} above its {months}m SMA {sma:.2f}"
            else:
                reasons[asset] = f"{asset} {closes[-1]:.2f} at or below its {months}m SMA {sma:.2f}, to cash"
        timed_out = [asset for asset in assets if targets[asset] == 0]
        targets[cash] = sleeve * len(timed_out)
        reasons[cash] = f"cash for {len(timed_out)} of {len(assets)} sleeves: {', '.join(timed_out) or 'none'}"
        ctx.rebalance(
            targets,
            reason=reasons,
            exit_reason="not one of the five sleeves or cash",
        )


if __name__ == "__main__":
    run(Gtaa5)
