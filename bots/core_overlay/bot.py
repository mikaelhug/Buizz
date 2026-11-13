from buizz_bots.drift import Book, rebalance_within_band
from buizz_bots.ex_junk import ExJunk
from buizz_bots.quality_momentum import QualityMomentum
from buizz_bots.signals import OVERLAYS, SmaCache, overlay_share
from buizz_bots.switch import held_weight

from buizz import Bot, Context, run

CORES = {"ex_junk": ExJunk, "quality_momentum": QualityMomentum}


def off_target(ctx: Context, symbol: str, target: float, band: float) -> bool:
    held = held_weight(ctx, symbol)
    return held is None or abs(held - target) > band


class CoreOverlay(Bot):
    def __init__(self) -> None:
        self.cache = SmaCache()
        self.core: ExJunk | QualityMomentum | None = None

    def on_start(self, ctx: Context) -> None:
        if ctx.params["core"] not in CORES:
            raise ValueError(f"core must be one of {', '.join(CORES)}")
        if ctx.params["overlay"] not in OVERLAYS:
            raise ValueError(f"overlay must be one of {', '.join(OVERLAYS)}")

    def on_step(self, ctx: Context) -> None:
        core = self.core = self.core or CORES[ctx.params["core"]]()
        hedge: str = ctx.params["cash"]
        bills: str = ctx.params["bills"]
        found = overlay_share(ctx, ctx.params["overlay"], self.cache, ctx.params["market"])
        if found is None:
            ctx.log(f"the {ctx.params['overlay']} overlay has no data yet, no decision", level="warn")
            return
        share, why = found
        exposure = float(ctx.params["l_max"]) * share
        overlay = f"exposure {exposure:.2f}: {why}"
        hedge_reason = f"{hedge} {exposure - 1:+.2f} of equity on top of the core for {overlay}"
        bills_weight = max(0.0, 1 - exposure)
        bills_reason = f"{bills} {bills_weight:.2f}, the exposure below 1 in T-bills, for {overlay}"
        ctx.log(overlay)
        if core.due(ctx) and (book := core.book(ctx)) is not None:
            self.rebalance(ctx, book, exposure, hedge_reason, bills_reason)
            return
        if "parked" not in ctx.state:
            return
        hedge_weight = float(ctx.state["parked"]) + exposure - 1
        band = float(ctx.params["band"])
        if off_target(ctx, hedge, hedge_weight, band) or off_target(ctx, bills, bills_weight, band):
            ctx.order_target_weight(hedge, hedge_weight, reason=hedge_reason)
            ctx.order_target_weight(bills, bills_weight, reason=bills_reason)

    def rebalance(self, ctx: Context, book: Book, exposure: float, hedge_reason: str, bills_reason: str) -> None:
        hedge: str = ctx.params["cash"]
        bills: str = ctx.params["bills"]
        parked = book.targets.get(hedge, 0.0)
        ctx.state["parked"] = parked
        targets = {**book.targets, hedge: parked + exposure - 1, bills: max(0.0, 1 - exposure)}
        reasons = {**book.reasons, hedge: hedge_reason, bills: bills_reason}
        rebalance_within_band(
            ctx, targets, float(ctx.params["drift_band"]), reason=reasons, exit_reason=book.exit_reason
        )


if __name__ == "__main__":
    run(CoreOverlay)
