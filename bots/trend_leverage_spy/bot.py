from datetime import date

from buizz_bots.monthly import MAX_SESSIONS_PER_MONTH, last_session
from buizz_bots.preclose import ProxyLog, session_so_far, with_session_so_far
from buizz_bots.sessions import last_session_of_month
from buizz_bots.signals import TREND_MONTHS, TREND_SESSIONS, Signal
from buizz_bots.switch import drifted
from buizz_bots.trend import month_end_sma_trend, sma_trend
from buizz_bots.volatility import ewma_sessions, ex_ante_volatility

from buizz import Bot, Context, History, run

FILTERS = ("sma200", "10-month")


def regime(history: History, day: date, rule: str, buffer: float) -> Signal | None:
    if rule == "sma200":
        found, length = sma_trend(history, day, TREND_SESSIONS), f"{TREND_SESSIONS}-day"
    else:
        found, length = month_end_sma_trend(history, day, TREND_MONTHS), f"{TREND_MONTHS}-month"
    if found is None:
        return None
    close, sma = found
    return Signal(close > sma * (1 + buffer), f"{close:.2f} vs {1 + buffer:.2f} x its {length} SMA {sma:.2f}")


class TrendLeverageSpy(Bot):
    def on_start(self, ctx: Context) -> None:
        if ctx.params["filter"] not in FILTERS:
            raise ValueError(f"filter must be one of {', '.join(FILTERS)}")

    def on_step(self, ctx: Context) -> None:
        asset: str = ctx.params["asset"]
        cash: str = ctx.params["cash"]
        rule: str = ctx.params["filter"]
        l_max = float(ctx.params["l_max"])
        target_vol = float(ctx.params["target_vol"])
        buffer = float(ctx.params["buffer"])
        band = float(ctx.params["band"])
        centre_of_mass = 1 / (2 ** (1 / float(ctx.params["vol_half_life_days"])) - 1)
        sessions = ewma_sessions(centre_of_mass)
        bars = max(sessions, MAX_SESSIONS_PER_MONTH * TREND_MONTHS, TREND_SESSIONS)
        history = ctx.history([asset], "1d", bars, adjust="all")[asset]
        bar = session_so_far(ctx, asset)
        if bar is None or history.start.size == 0:
            ctx.log(f"no {asset} trade today or no history yet, no decision", level="warn")
            return
        proxy = ProxyLog(ctx)
        settled = regime(history, last_session(history), rule, buffer)
        if settled is not None:
            proxy.check(asset, history, {"risk_on": settled.bullish})
        today = with_session_so_far(history, bar)
        held = any(position.qty != 0 for position in ctx.portfolio.positions.values())
        if rule == "sma200" or last_session_of_month(ctx) or not held:
            if (signal := regime(today, ctx.session.date, rule, buffer)) is None:
                ctx.log(f"{asset} has too short a history for its {rule} filter, no decision", level="warn")
                proxy.save()
                return
            proxy.record(asset, "risk_on", signal.bullish)
            risk_on, why = signal.bullish, f"{asset} {signal.why}"
        else:
            risk_on = ctx.position(asset).qty > 0
            why = f"between month ends, keeping the month-end {'risk-on' if risk_on else 'risk-off'} decision"
        proxy.save()
        if not risk_on:
            weight = 0.0
        elif target_vol > 0:
            closes = today.close[-(sessions + 1) :]
            if closes.size <= sessions:
                ctx.log(f"{asset} has {closes.size} of {sessions + 1} closes for its volatility, no decision")
                return
            vol = ex_ante_volatility(closes, centre_of_mass)
            weight = min(l_max, target_vol / vol)
            why = f"{why}; EWMA vol {vol:.1%}, {target_vol:.0%}/{vol:.1%} capped at {l_max:.2f}"
        else:
            weight = l_max
        targets = {asset: weight, cash: max(0.0, 1 - weight)}
        summary = f"{why}: {asset} {weight:.2f}"
        ctx.log(summary)
        if not drifted(ctx, targets, band):
            return
        ctx.rebalance(
            targets,
            at="close",
            reason={asset: summary, cash: f"the {targets[cash]:.2f} of equity not in {asset}"},
            exit_reason=f"neither {asset} nor {cash}",
        )


if __name__ == "__main__":
    run(TrendLeverageSpy)
