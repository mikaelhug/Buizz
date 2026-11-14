from datetime import date, timedelta

import numpy as np
from buizz_bots.caps import capped_weights, market_caps, parked
from buizz_bots.drift import rebalance_within_band
from buizz_bots.fundamentals import by_symbol, histories_since, latest_shares
from buizz_bots.monthly import MOMENTUM_MONTHS, first_of_month, session_days
from buizz_bots.ranks import highest_first
from buizz_bots.sessions import REBALANCED, last_session_of_month, next_session

from buizz import Bot, Context, History, run

SEMIANNUAL_MONTHS = (3, 9)
THIRD_FRIDAY_EARLIEST_DAY = 15
FRIDAY = 4
WINSOR = 3.0


def third_friday(year: int, month: int) -> date:
    earliest = date(year, month, THIRD_FRIDAY_EARLIEST_DAY)
    return earliest + timedelta(days=(FRIDAY - earliest.weekday()) % 7)


def rebalance_due(ctx: Context) -> bool:
    if REBALANCED not in ctx.state:
        return True
    if ctx.params["rebalance"] == "monthly":
        return last_session_of_month(ctx)
    today = ctx.session.date
    if today.month not in SEMIANNUAL_MONTHS:
        return False
    friday = third_friday(today.year, today.month)
    following = next_session(ctx)
    return today <= friday and (following is None or following > friday)


def risk_adjusted_momentum(history: History, today: date, skipped: int) -> float | None:
    days = session_days(history)
    if days.size == 0 or days[-1] != np.datetime64(today):
        return None
    months = days.astype("datetime64[M]")
    ends = np.flatnonzero(np.append(months[1:] != months[:-1], True))
    if ends.size < MOMENTUM_MONTHS + skipped:
        return None
    closes = history.close[ends[-MOMENTUM_MONTHS - skipped] : ends[-skipped] + 1]
    if closes.size < MOMENTUM_MONTHS or np.any(closes <= 0):
        return None
    volatility = float(np.std(np.diff(closes) / closes[:-1], ddof=1))
    return None if volatility == 0 else float(closes[-1] / closes[0] - 1) / volatility


def winsorized_z(values: dict[str, float]) -> dict[str, float]:
    data = np.array(list(values.values()))
    spread = float(np.std(data))
    if spread == 0:
        return {}
    z = np.clip((data - data.mean()) / spread, -WINSOR, WINSOR)
    return dict(zip(values, z.tolist(), strict=True))


def momentum_score(z: float) -> float:
    return 1 + z if z > 0 else 1 / (1 - z)


def buffered(ranked: list[str], held: set[str], count: int, buffer: float) -> list[str]:
    chosen = ranked[: round(count * (1 - buffer))]
    keep = [symbol for symbol in ranked[len(chosen) : round(count * (1 + buffer))] if symbol in held]
    chosen += keep[: count - len(chosen)]
    return chosen + [symbol for symbol in ranked if symbol not in chosen][: count - len(chosen)]


def book_weights(ctx: Context, chosen: list[str], caps: dict[str, float], z: dict[str, float]) -> dict[str, float]:
    if ctx.params["weighting"] == "equal":
        return dict.fromkeys(chosen, 1 / len(chosen))
    total = sum(caps.values())
    multiple = float(ctx.params["index_multiple"])
    ceiling = float(ctx.params["max_weight"])
    weights = capped_weights(
        {symbol: caps[symbol] * momentum_score(z[symbol]) for symbol in chosen},
        {symbol: min(ceiling, multiple * caps[symbol] / total) for symbol in chosen},
    )
    return parked(weights, ctx.params["cash"])


class Sp500MomentumCap(Bot):
    def on_step(self, ctx: Context) -> None:
        if not rebalance_due(ctx):
            return
        today = ctx.session.date
        ctx.state[REBALANCED] = today.isoformat()
        members = sorted(ctx.universe)
        own = by_symbol(ctx.fundamentals(members, ["shares_outstanding"]))
        counts = {symbol: count for symbol in members if (count := latest_shares(own[symbol], today)) is not None}
        since = min([*(count.filed for count in counts.values()), first_of_month(today, MOMENTUM_MONTHS + 1)])
        raw, split = histories_since(ctx, sorted(counts), since)
        skipped = 2 if last_session_of_month(ctx) else 3
        caps = market_caps(ctx, counts, raw, split)
        signals = {
            symbol: value
            for symbol in sorted(caps)
            if (value := risk_adjusted_momentum(split[symbol], today, skipped)) is not None
        }
        z = winsorized_z(signals)
        ranked = highest_first(z)
        held = set(ctx.portfolio.positions)
        chosen = buffered(ranked, held, int(ctx.params["holdings"]), float(ctx.params["buffer"]))
        if not chosen:
            ctx.log(f"no S&P 500 member with a market cap and 13 months of prices on {today}", level="warn")
            return
        targets = book_weights(ctx, chosen, caps, z)
        place = {symbol: rank for rank, symbol in enumerate(ranked, 1)}
        reasons = {
            symbol: f"weight {weight:.3%}, risk-adjusted 12-1 momentum {signals[symbol]:.2f}, "
            f"z {z[symbol]:+.2f} ranks {place[symbol]} of {len(ranked)}"
            for symbol, weight in targets.items()
            if symbol in z
        }
        cash = ctx.params["cash"]
        if cash in targets:
            reasons[cash] = f"weight {targets[cash]:.3%} left over by the per-name caps"
        ctx.log(f"{len(caps)} members with a market cap, {len(ranked)} with momentum, {len(chosen)} held")
        rebalance_within_band(
            ctx,
            targets,
            float(ctx.params["drift_band"]),
            reason=reasons,
            exit_reason=f"not among the {len(chosen)} chosen of {len(ranked)} by risk-adjusted momentum and its buffer",
        )


if __name__ == "__main__":
    run(Sp500MomentumCap)
