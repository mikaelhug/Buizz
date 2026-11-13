from buizz_bots.insiders import aged_out, buy_reason, fresh_clusters, remember
from buizz_bots.switch import held_weight

from buizz import Bot, Context, run

CORE_BAND = 0.05


class InsiderOverlay(Bot):
    def on_start(self, ctx: Context) -> None:
        if ctx.params["core"] == ctx.params["hedge"]:
            raise ValueError("core and hedge must be different symbols")

    def on_step(self, ctx: Context) -> None:
        core: str = ctx.params["core"]
        hedge: str = ctx.params["hedge"]
        max_names = int(ctx.params["max_names"])
        hold = int(ctx.params["hold_sessions"])
        skip = int(ctx.params["skip_largest"])
        weight = float(ctx.params["overlay"]) / max_names
        held = {
            symbol
            for symbol, position in ctx.portfolio.positions.items()
            if position.qty != 0 and symbol not in (core, hedge)
        }
        signals, held_for, exits = aged_out(ctx, held)
        staying = held - set(exits)
        largest = set(ctx.universe_at(ctx.session.date, skip, asset_class="stock") if skip else [])
        candidates = [symbol for symbol in ctx.universe if symbol not in largest]
        entries = fresh_clusters(ctx, candidates, held, max_names - len(staying))
        remember(ctx, signals, entries)

        core_weight = held_weight(ctx, core)
        core_off = core_weight is None or abs(core_weight - 1) > CORE_BAND
        if not entries and not exits and not core_off:
            return
        equity = float(ctx.portfolio.equity)
        long_book = weight * len(entries) + sum(
            ctx.position(symbol).qty * ctx.bars[symbol].close / equity if symbol in ctx.bars else weight
            for symbol in staying
        )
        picks = len(staying) + len(entries)
        for symbol in exits:
            ctx.order_target_weight(symbol, 0, reason=f"held {held_for.get(symbol, hold)} sessions since its cluster")
        if core_off:
            ctx.order_target_weight(core, 1.0, reason=f"core {core} at 100% of equity under the insider overlay")
        ctx.order_target_weight(
            hedge, -long_book, reason=f"hedge {hedge} short {long_book:.1%} of equity against {picks} insider picks"
        )
        for found in entries:
            ctx.order_target_weight(found.symbol, weight, reason=buy_reason(ctx, found))


if __name__ == "__main__":
    run(InsiderOverlay)
