from buizz_bots.drift import rebalance_within_band
from buizz_bots.quality_momentum import QualityMomentum

from buizz import Bot, Context, run


class Sp500QualityMomentum(Bot):
    def __init__(self) -> None:
        self.core = QualityMomentum()

    def on_step(self, ctx: Context) -> None:
        if not self.core.due(ctx) or (book := self.core.book(ctx)) is None:
            return
        rebalance_within_band(
            ctx, book.targets, float(ctx.params["drift_band"]), reason=book.reasons, exit_reason=book.exit_reason
        )


if __name__ == "__main__":
    run(Sp500QualityMomentum)
