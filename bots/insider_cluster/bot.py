from buizz_bots.insiders import aged_out, buy_reason, fresh_clusters, remember

from buizz import Bot, Context, run


class InsiderCluster(Bot):
    def on_step(self, ctx: Context) -> None:
        cash: str = ctx.params["cash"]
        max_names = int(ctx.params["max_names"])
        hold = int(ctx.params["hold_sessions"])
        held = {symbol for symbol, position in ctx.portfolio.positions.items() if position.qty != 0 and symbol != cash}
        signals, held_for, exits = aged_out(ctx, held)
        staying = held - set(exits)
        entries = fresh_clusters(ctx, ctx.universe, held, max_names - len(staying))
        remember(ctx, signals, entries)

        if not entries and not exits and ctx.position(cash).qty != 0:
            return
        equity = float(ctx.portfolio.equity)
        kept = sum(
            ctx.position(symbol).qty * ctx.bars[symbol].close / equity if symbol in ctx.bars else 1 / max_names
            for symbol in staying
        )
        room = max(0.0, 1 - kept)
        weight = min(1 / max_names, room / len(entries)) if entries else 0.0
        parked = max(0.0, room - weight * len(entries))
        for symbol in exits:
            ctx.order_target_weight(symbol, 0, reason=f"held {held_for.get(symbol, hold)} sessions since its cluster")
        parked_qty = ctx.position(cash).qty
        if cash not in ctx.bars or int(parked * equity / ctx.bars[cash].close) != parked_qty:
            ctx.order_target_weight(
                cash, parked, reason=f"idle cash: {len(staying) + len(entries)} of {max_names} slots taken"
            )
        for found in entries:
            ctx.order_target_weight(found.symbol, weight, reason=buy_reason(ctx, found))


if __name__ == "__main__":
    run(InsiderCluster)
