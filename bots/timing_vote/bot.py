from buizz_bots.signals import Signal, SmaCache, breadth, credit, curve, labour, trend, valuation, vix_term
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
        votes: dict[str, Signal | None] = {
            "trend": trend(ctx, asset),
            "breadth": breadth(ctx, self.cache),
            "credit": credit(ctx),
            "vix": vix_term(ctx),
            "labour": labour(ctx),
            "curve": curve(ctx),
            "valuation": valuation(ctx),
        }
        available = {name: vote for name, vote in votes.items() if vote is not None}
        missing = ", ".join(name for name, vote in votes.items() if vote is None) or "none"
        if not available:
            ctx.log("no condition has its data yet, no decision", level="warn")
            return
        bullish = sum(vote.bullish for vote in available.values())
        share = bullish / len(available)
        weight = exposure(share, l_max, ctx.params["mapping"])
        targets = {asset: weight, cash: max(0.0, 1 - weight)}
        tally = ", ".join(f"{name} {'+' if vote.bullish else '-'}" for name, vote in available.items())
        summary = f"{bullish} of {len(available)} votes bullish ({tally}; missing {missing}), {asset} {weight:.2f}"
        ctx.log(f"{summary}: " + "; ".join(vote.why for vote in available.values()))
        if not drifted(ctx, targets, band):
            return
        ctx.rebalance(
            targets,
            reason={asset: summary, cash: f"the {targets[cash]:.2f} of equity not in {asset}"},
            exit_reason=f"neither {asset} nor {cash}",
        )


if __name__ == "__main__":
    run(TimingVote)
