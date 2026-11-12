from collections.abc import Mapping

from buizz import Context

DRIFT_BAND = 0.10


def held_weight(ctx: Context, symbol: str) -> float | None:
    qty = ctx.position(symbol).qty
    if qty == 0:
        return 0.0
    bar = ctx.bars.get(symbol)
    return None if bar is None else qty * bar.close / float(ctx.portfolio.equity)


def drifted(ctx: Context, targets: Mapping[str, float], band: float) -> bool:
    held = {symbol for symbol, position in ctx.portfolio.positions.items() if position.qty != 0}
    if not held <= targets.keys():
        return True
    return any(
        (weight := held_weight(ctx, symbol)) is not None and abs(weight - target) > band
        for symbol, target in targets.items()
    )


def hold_only(ctx: Context, symbol: str, why: str, weight: float = 1.0) -> None:
    if drifted(ctx, {symbol: weight}, DRIFT_BAND):
        ctx.rebalance({symbol: weight}, reason=why, exit_reason=why)


def switch(ctx: Context, risk_on: bool, asset: str, safe: str, why: str) -> None:
    if risk_on:
        hold_only(ctx, asset, why, float(ctx.params["risk_on_weight"]))
    else:
        hold_only(ctx, safe, why)
