from collections.abc import Mapping
from datetime import date

import numpy as np

from buizz import Context, FundamentalPoint, History
from buizz_bots.fundamentals import adjusted_shares, traded_today

DOLLAR_VOLUME_SESSIONS = 20
MIN_HISTORY_DAYS = 40
LISTED_MISSING = 20
MIN_DAILY_TURNOVER = 1e-4
MAX_DAILY_TURNOVER = 1.0

Identity = tuple[date, date, str]


def market_caps(
    ctx: Context, counts: Mapping[str, FundamentalPoint], raw: Mapping[str, History], split: Mapping[str, History]
) -> dict[str, float]:
    lines: dict[Identity, tuple[float, str, float]] = {}
    for symbol, count in counts.items():
        history = raw.get(symbol)
        if history is None or not traded_today(ctx, history):
            continue
        shares = adjusted_shares(count, history, split[symbol])
        if shares is None:
            continue
        recent = slice(-DOLLAR_VOLUME_SESSIONS, None)
        turnover = float(np.mean(split[symbol].volume[recent])) / shares
        if not MIN_DAILY_TURNOVER <= turnover <= MAX_DAILY_TURNOVER:
            ctx.log(
                f"{symbol} share count {float(count.value):,.0f} on {count.period_end} (filed {count.filed}) "
                f"not used: {DOLLAR_VOLUME_SESSIONS}-session average volume is {turnover:.1e} of it",
                level="warn",
            )
            continue
        line = (
            float(np.mean(history.close[recent] * history.volume[recent])),
            symbol,
            shares * float(history.close[-1]),
        )
        identity = (count.period_end, count.filed, str(count.value))
        if identity not in lines or line[:2] > lines[identity][:2]:
            lines[identity] = line
    return {symbol: cap for _, symbol, cap in lines.values()}


def capped_weights(scores: Mapping[str, float], limits: Mapping[str, float]) -> dict[str, float]:
    weights: dict[str, float] = {}
    free = {symbol: score for symbol, score in scores.items() if score > 0}
    budget = 1.0
    while free:
        total = sum(free.values())
        trial = {symbol: budget * score / total for symbol, score in free.items()}
        over = [symbol for symbol, weight in trial.items() if weight > limits[symbol]]
        if not over:
            return weights | trial
        for symbol in over:
            weights[symbol] = limits[symbol]
            budget -= limits[symbol]
            del free[symbol]
    return weights


def parked(weights: Mapping[str, float], cash: str) -> dict[str, float]:
    rest = 1 - sum(weights.values())
    return {**weights, cash: rest} if rest > 0 else dict(weights)


def missing_caps(members: list[str], caps: Mapping[str, float]) -> str:
    missing = [symbol for symbol in members if symbol not in caps]
    return f"{len(missing)} of {len(members)} members without a market cap: {', '.join(missing[:LISTED_MISSING])}"
