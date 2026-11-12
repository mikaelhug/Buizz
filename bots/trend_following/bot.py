import math
from datetime import date, timedelta

import numpy as np
import numpy.typing as npt
from buizz_bots.sessions import last_session_of_month

from buizz import Bot, Context, History, run

VARIANCE_DAYS_PER_YEAR = 261
MAX_SESSIONS_PER_MONTH = 23
TRUNCATED_WEIGHT = 1e-4


def month_start(day: date, months_back: int) -> date:
    index = day.year * 12 + day.month - 1 - months_back
    return date(index // 12, index % 12 + 1, 1)


def close_on(history: History, day: date) -> float | None:
    found = np.flatnonzero(history.start.astype("datetime64[ns]").astype("datetime64[D]") == np.datetime64(day))
    return float(history.close[found[0]]) if found.size else None


def ex_ante_volatility(closes: npt.NDArray[np.float64], centre_of_mass: float) -> float:
    newest_first = (closes[1:] / closes[:-1] - 1)[::-1]
    delta = centre_of_mass / (centre_of_mass + 1)
    weights = delta ** np.arange(newest_first.size)
    weights /= weights.sum()
    mean = float(weights @ newest_first)
    return math.sqrt(VARIANCE_DAYS_PER_YEAR * float(weights @ (newest_first - mean) ** 2))


class TrendFollowing(Bot):
    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        assets: list[str] = ctx.params["assets"]
        cash: str = ctx.params["cash"]
        months = int(ctx.params["lookback_months"])
        asset_vol = float(ctx.params["asset_vol"])
        centre_of_mass = float(ctx.params["vol_com_days"])
        max_gross = float(ctx.params["max_gross"])
        today = ctx.session.date
        first_day = month_start(today, months)
        if not (anchor_month := ctx.calendar(first_day, month_start(today, months - 1) - timedelta(days=1))):
            ctx.log(f"the calendar has no session in {first_day:%Y-%m}, no signal yet", level="warn")
            return
        anchor = anchor_month[-1].date
        vol_sessions = math.ceil(math.log(TRUNCATED_WEIGHT) / math.log(centre_of_mass / (centre_of_mass + 1)))
        history = ctx.history(
            [*assets, cash], "1d", max(vol_sessions, MAX_SESSIONS_PER_MONTH * (months + 1)) + 1, adjust="all"
        )

        def trailing_return(symbol: str) -> float | None:
            start, end = close_on(history[symbol], anchor), close_on(history[symbol], today)
            return None if start is None or end is None else end / start - 1

        if (cash_return := trailing_return(cash)) is None:
            ctx.log(f"no {months}-month return of {cash} since the {anchor} close, no excess returns", level="warn")
            return
        signals: dict[str, tuple[float, float]] = {}
        for asset in assets:
            if (total := trailing_return(asset)) is None:
                continue
            if (vol := ex_ante_volatility(history[asset].close[-(vol_sessions + 1) :], centre_of_mass)) > 0:
                signals[asset] = (total, vol)
        count = len(signals)
        raw = {
            asset: float(np.sign(total - cash_return)) * asset_vol / vol / count
            for asset, (total, vol) in signals.items()
        }
        gross = sum(abs(weight) for weight in raw.values())
        scale = min(1.0, max_gross / gross) if gross > 0 else 1.0
        capped = f", book scaled x{scale:.2f} from {gross:.2f}x to the {max_gross:.1f}x gross cap" if scale < 1 else ""
        missing = f"no {months}-month excess return or volatility since the {anchor} close, flat"
        reasons = dict.fromkeys(assets, missing)
        for asset, (total, vol) in signals.items():
            excess = total - cash_return
            side = "long" if excess > 0 else "short" if excess < 0 else "flat"
            reasons[asset] = (
                f"{months}m excess {excess:+.1%} ({asset} {total:+.1%} vs {cash} {cash_return:+.1%} since {anchor})"
                f" → {side}; ex-ante vol {vol:.1%}, {asset_vol:.0%}/{vol:.1%}/{count} = {raw[asset]:+.2f}{capped}"
            )
        targets = {asset: scale * raw.get(asset, 0.0) for asset in assets}
        ctx.log(
            f"{count} of {len(assets)} assets with a signal, gross {scale * gross:.2f}x,"
            f" net {sum(targets.values()):+.2f}x of equity{capped}"
        )
        ctx.rebalance(targets, reason=reasons, exit_reason="not one of the trend-following assets")


if __name__ == "__main__":
    run(TrendFollowing)
