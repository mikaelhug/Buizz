from buizz_bots.monthly import MAX_SESSIONS_PER_MONTH, month_end_closes
from buizz_bots.sessions import last_session_of_month

from buizz import Bot, Context, run


class SectorRotation(Bot):
    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        today = ctx.session.date
        sectors: list[str] = ctx.params["sectors"]
        top = int(ctx.params["top"])
        lookbacks: list[int] = ctx.params["lookbacks_months"]
        hedge = bool(ctx.params["hedge"])
        market: str = ctx.params["market"]
        sma_months = int(ctx.params["sma_months"])
        cash: str = ctx.params["cash"]
        needed = max(lookbacks) + 1
        months = max(needed, sma_months)
        history = ctx.history([*sectors, market], "1d", MAX_SESSIONS_PER_MONTH * months, adjust="all")
        blend = "/".join(map(str, lookbacks))
        scores: dict[str, float] = {}
        reasons: dict[str, str] = {}
        for sector in sectors:
            closes = month_end_closes(history[sector], today)[-needed:]
            if closes.size < needed:
                reasons[sector] = f"{sector} has {closes.size} of {needed} month-end closes, not ranked"
                continue
            scores[sector] = sum(float(closes[-1] / closes[-1 - k] - 1) for k in lookbacks) / len(lookbacks)
        ranked = sorted(scores, key=lambda sector: (-scores[sector], sector))
        for rank, sector in enumerate(ranked, start=1):
            standing = "in" if rank <= top else "outside"
            reasons[sector] = (
                f"rank {rank} of {len(ranked)}, {standing} the top {top}, "
                f"by {blend}m average total return {scores[sector]:+.2%}"
            )
        held = ranked[:top]
        risk_off = ""
        if hedge:
            closes = month_end_closes(history[market], today)[-sma_months:]
            if closes.size < sma_months:
                risk_off = f"{market} has {closes.size} of {sma_months} month-end closes"
            elif closes[-1] <= (sma := float(closes.mean())):
                risk_off = f"{market} {closes[-1]:.2f} at or below its {sma_months}m SMA {sma:.2f}"
        if risk_off:
            for sector in held:
                reasons[sector] += f", out: {risk_off}"
            held = []
        targets = dict.fromkeys([*sectors, cash], 0.0) | dict.fromkeys(held, 1 / top)
        targets[cash] = 1 - len(held) / top
        reasons[cash] = (
            f"all in T-bills: {risk_off}"
            if risk_off
            else f"T-bills for {top - len(held)} of {top} slots, {len(ranked)} sectors ranked"
        )
        ctx.rebalance(targets, reason=reasons, exit_reason="not one of the sectors or cash")


if __name__ == "__main__":
    run(SectorRotation)
