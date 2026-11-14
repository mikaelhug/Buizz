from collections.abc import Mapping

from buizz import Context


def renamed[T](ctx: Context, keyed: Mapping[str, T]) -> dict[str, T]:
    return {ctx.renames.get(symbol, symbol): value for symbol, value in keyed.items()}
