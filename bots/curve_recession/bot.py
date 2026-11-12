from buizz_bots.sessions import last_session_of_month
from buizz_bots.signals import curve_inversions, labour, monthly_trend, sahm
from buizz_bots.switch import switch

from buizz import Bot, Context, run


class CurveRecession(Bot):
    def on_step(self, ctx: Context) -> None:
        if not last_session_of_month(ctx):
            return
        asset: str = ctx.params["asset"]
        bonds: str = ctx.params["bonds"]
        armed_days = int(ctx.params["armed_days"])
        sahm_threshold = float(ctx.params["sahm_threshold"])
        unrate_months = int(ctx.params["unrate_months"])
        sma_months = int(ctx.params["sma_months"])
        if (found := curve_inversions(ctx, armed_days)) is None:
            ctx.log(f"no 10y and 3m yields of the same day in the last {armed_days} days, no decision", level="warn")
            return
        paired, inverted = found
        if not inverted:
            why = f"10y-3m not negative on any of {paired} days in the last {armed_days} days: unarmed, {asset}"
            ctx.log(why)
            switch(ctx, True, asset, bonds, why)
            return
        gap = sahm(ctx)
        jobs = labour(ctx, unrate_months)
        if gap is None or jobs is None:
            ctx.log("too few unemployment rates visible for the Sahm rule and its average, no decision", level="warn")
            return
        if (price := monthly_trend(ctx, asset, sma_months)) is None:
            ctx.log(f"{asset} has fewer than {sma_months} month-end closes, no decision", level="warn")
            return
        signal = (
            f"armed: 10y-3m last negative on {inverted[-1]}; Sahm gap {gap:.2f} (trigger {sahm_threshold:.2f}), "
            f"{jobs.why}; {price.why}"
        )
        triggered = gap >= sahm_threshold or not jobs.bullish
        if not triggered:
            why = f"{signal}: no labour trigger, {asset}"
        elif price.bullish:
            why = f"{signal}: labour trigger but {asset} above its SMA, {asset}"
        else:
            why = f"{signal}: labour trigger and {asset} at or below its SMA, {bonds}"
        ctx.log(why)
        switch(ctx, not triggered or price.bullish, asset, bonds, why)


if __name__ == "__main__":
    run(CurveRecession)
