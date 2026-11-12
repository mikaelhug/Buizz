from buizz_bots.sessions import last_session_of_month
from buizz_bots.signals import monthly_trend, valuation

from buizz import Bot, Context, run


class ForwardPeTilt(Bot):
    def on_step(self, ctx: Context) -> None:
        equity: str = ctx.params["equity"]
        bonds: str = ctx.params["bonds"]
        invested = any(ctx.position(symbol).qty != 0 for symbol in (equity, bonds))
        if invested and not last_session_of_month(ctx):
            return
        months = int(ctx.params["sma_months"])
        years = float(ctx.params["min_history_years"])
        value = valuation(ctx, float(ctx.params["expensive_quantile"]), years)
        price = monthly_trend(ctx, equity, months)
        tilted = value is not None and not value.bullish and price is not None and not price.bullish
        bond_weight = float(ctx.params["bond_weight"]) if tilted else 0.0
        state = "expensive and in a downtrend, tilt" if tilted else "hold"
        valued = value.why if value else f"fewer than {years:g} years of forward P/E and 10-year yield"
        trended = price.why if price else f"{equity} has fewer than {months} month-end closes"
        ctx.rebalance(
            {equity: 1 - bond_weight, bonds: bond_weight},
            reason=f"{state}: {valued}; {trended}",
            exit_reason="not part of the valuation and trend tilt",
        )


if __name__ == "__main__":
    run(ForwardPeTilt)
