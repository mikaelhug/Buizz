from collections.abc import Mapping

from buizz import Context

UNCHANGED_SHARE_MARGIN = 0.5


def rebalance_within_band(
    ctx: Context, targets: Mapping[str, float], band: float, *, reason: Mapping[str, str], exit_reason: str
) -> None:
    equity = float(ctx.portfolio.equity)
    held = {
        symbol: (position.qty, ctx.bars[symbol].close)
        for symbol, position in ctx.portfolio.positions.items()
        if targets.get(symbol, 0) > 0 and position.qty > 0 and symbol in ctx.bars
    }
    staying = {
        symbol: qty * close / equity
        for symbol, (qty, close) in held.items()
        if abs(qty * close / equity - targets[symbol]) <= band * targets[symbol]
    }
    excess = sum(weight - targets[symbol] for symbol, weight in staying.items())
    for symbol in sorted(staying, key=lambda symbol: (targets[symbol] - staying[symbol], symbol)):
        if excess <= 0:
            break
        excess -= staying.pop(symbol) - targets[symbol]
    unchanged = {symbol: (held[symbol][0] + UNCHANGED_SHARE_MARGIN) * held[symbol][1] / equity for symbol in staying}
    ctx.rebalance({**targets, **unchanged}, reason=reason, exit_reason=exit_reason)
