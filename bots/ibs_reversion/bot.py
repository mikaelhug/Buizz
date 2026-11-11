from typing import NamedTuple

from buizz_bots.preclose import ProxyLog, session_so_far

from buizz import Bot, Context, run

SIDES = {"long": 1, "flat": 0, "short": -1}


class Rules(NamedTuple):
    long_ibs: float
    short_ibs: float

    def side(self, ibs: float | None) -> str:
        if ibs is None:
            return "flat"
        if ibs <= self.long_ibs:
            return "long"
        if ibs >= self.short_ibs:
            return "short"
        return "flat"

    def reason(self, ibs: float | None) -> str:
        if ibs is None:
            return "high equals low, IBS undefined"
        if ibs <= self.long_ibs:
            return f"IBS {ibs:.3f} at or below {self.long_ibs}, close near the day's low"
        if ibs >= self.short_ibs:
            return f"IBS {ibs:.3f} at or above {self.short_ibs}, close near the day's high"
        return f"IBS {ibs:.3f} between {self.long_ibs} and {self.short_ibs}"


def internal_bar_strength(high: float, low: float, close: float) -> float | None:
    return (close - low) / (high - low) if high > low else None


def held_side(qty: int) -> str:
    return "long" if qty > 0 else "short" if qty < 0 else "flat"


class IbsReversion(Bot):
    def on_step(self, ctx: Context) -> None:
        rules = Rules(ctx.params["long_ibs"], ctx.params["short_ibs"])
        weight = 1 / len(ctx.universe)
        proxy = ProxyLog(ctx)
        history = ctx.history(ctx.universe, "1d", 1)
        for symbol in ctx.universe:
            bar = session_so_far(ctx, symbol)
            if bar is None:
                continue
            official = history[symbol]
            if len(official.close) > 0:
                settled = internal_bar_strength(
                    float(official.high[-1]), float(official.low[-1]), float(official.close[-1])
                )
                proxy.check(symbol, official, {"target": rules.side(settled)})
            ibs = internal_bar_strength(bar.high, bar.low, bar.close)
            side = rules.side(ibs)
            proxy.record(symbol, "target", side)
            if side != held_side(ctx.position(symbol).qty):
                ctx.order_target_weight(symbol, SIDES[side] * weight, at="close", reason=rules.reason(ibs))
        proxy.save()


if __name__ == "__main__":
    run(IbsReversion)
