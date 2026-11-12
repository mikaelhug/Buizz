from collections.abc import Mapping

from buizz import Context


def rebalance_within_band(
    ctx: Context, targets: Mapping[str, float], band: float, *, reason: Mapping[str, str], exit_reason: str
) -> None:
    equity = float(ctx.portfolio.equity)
    held = {
        symbol: position.qty * ctx.bars[symbol].close / equity if symbol in ctx.bars else targets[symbol]
        for symbol, position in ctx.portfolio.positions.items()
        if symbol in targets and position.qty > 0
    }
    staying = sum(
        weight if abs(weight - targets[symbol]) <= band * targets[symbol] else targets[symbol]
        for symbol, weight in held.items()
    )
    entering = sum(weight for symbol, weight in targets.items() if symbol not in held)
    room = max(sum(targets.values()) - staying, 0.0)
    scale = 1.0 if entering <= room else room / entering
    ctx.rebalance(
        {symbol: weight if symbol in held else weight * scale for symbol, weight in targets.items()},
        reason=reason,
        exit_reason=exit_reason,
        band=band,
    )
