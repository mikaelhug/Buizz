from buizz import Context


def hold_only(ctx: Context, symbol: str, why: str) -> None:
    held = {name for name, position in ctx.portfolio.positions.items() if position.qty != 0}
    if held != {symbol}:
        ctx.rebalance({symbol: 1.0}, reason=why, exit_reason=why)
