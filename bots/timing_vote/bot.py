from buizz_bots.signals import SmaCache, breadth, credit, curve, growth, labour, trend, valuation, vix_term, vote_share
from buizz_bots.switch import drifted

from buizz import Bot, Context, run

MAPPINGS = ("linear", "majority")
MAJORITY = 0.6
MINORITY = 0.4


def exposure(share: float, l_max: float, mapping: str) -> float:
    if mapping == "linear":
        return l_max * share
    if share >= MAJORITY:
        return l_max
    return 1.0 if share >= MINORITY else 0.0


class TimingVote(Bot):
    def __init__(self) -> None:
        self.cache = SmaCache()

    def on_start(self, ctx: Context) -> None:
        if ctx.params["mapping"] not in MAPPINGS:
            raise ValueError(f"mapping must be one of {', '.join(MAPPINGS)}")

    def on_step(self, ctx: Context) -> None:
        asset: str = ctx.params["asset"]
        cash: str = ctx.params["cash"]
        l_max = float(ctx.params["l_max"])
        band = float(ctx.params["band"])
        found = vote_share(
            {
                "trend": trend(ctx, asset),
                "breadth": breadth(ctx, self.cache),
                "credit": credit(ctx),
                "vix": vix_term(ctx),
                "labour": labour(ctx),
                "growth": growth(ctx),
                "curve": curve(ctx),
                "valuation": valuation(ctx),
            }
        )
        if found is None:
            ctx.log("no condition has its data yet, no decision", level="warn")
            return
        share, tally = found
        weight = exposure(share, l_max, ctx.params["mapping"])
        targets = {asset: weight, cash: max(0.0, 1 - weight)}
        summary = f"{asset} {weight:.2f}: {tally}"
        ctx.log(summary)
        if not drifted(ctx, targets, band):
            return
        ctx.rebalance(
            targets,
            reason={asset: summary, cash: f"the {targets[cash]:.2f} of equity not in {asset}"},
            exit_reason=f"neither {asset} nor {cash}",
        )


if __name__ == "__main__":
    run(TimingVote)
