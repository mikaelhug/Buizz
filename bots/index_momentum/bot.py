from buizz_bots.monthly import MAX_SESSIONS_PER_MONTH, month_end_closes
from buizz_bots.ranks import highest_first
from buizz_bots.sessions import last_session_of_month
from buizz_bots.signals import Signal, SmaCache, breadth, credit, trend, vix_term
from buizz_bots.switch import drifted

from buizz import Bot, Context, run

OVERLAYS = ("none", "votes", "breadth")


def trailing_returns(ctx: Context, symbols: list[str], months: int) -> dict[str, float]:
    history = ctx.history(symbols, "1d", MAX_SESSIONS_PER_MONTH * (months + 1), adjust="all")
    returns: dict[str, float] = {}
    for symbol in symbols:
        closes = month_end_closes(history[symbol], ctx.session.date)
        if closes.size > months and closes[-1 - months] > 0:
            returns[symbol] = float(closes[-1] / closes[-1 - months] - 1)
    return returns


def select(ctx: Context, assets: list[str], cash: str, months: int, top: int) -> dict[str, str] | None:
    returns = trailing_returns(ctx, [*assets, cash], months)
    if cash not in returns or not (ranked := highest_first({a: returns[a] for a in assets if a in returns})):
        return None
    hurdle = returns[cash]
    picks: dict[str, str] = {}
    for rank, asset in enumerate(ranked[:top], start=1):
        if returns[asset] > hurdle:
            picks[asset] = (
                f"rank {rank} of {len(ranked)} by {months}m total return {returns[asset]:+.1%}, "
                f"above T-bills ({cash}) {hurdle:+.1%}"
            )
    return picks


def votes(ctx: Context, market: str) -> tuple[float, str] | None:
    found: dict[str, Signal | None] = {"trend": trend(ctx, market), "vix": vix_term(ctx), "credit": credit(ctx)}
    available = {name: vote for name, vote in found.items() if vote is not None}
    if not available:
        return None
    share = sum(vote.bullish for vote in available.values()) / len(available)
    tally = "; ".join(f"{name} {'+' if vote.bullish else '-'} ({vote.why})" for name, vote in available.items())
    return share, f"{share:.0%} of {len(available)} votes bullish: {tally}"


def breadth_or_trend(ctx: Context, cache: SmaCache, market: str) -> tuple[float, str] | None:
    if (wide := breadth(ctx, cache)) is None or (own := trend(ctx, market)) is None:
        return None
    risk_on = wide.bullish or own.bullish
    return float(risk_on), f"{'risk-on' if risk_on else 'risk-off'}: {wide.why}; {own.why}"


class IndexMomentum(Bot):
    def __init__(self) -> None:
        self.cache = SmaCache()

    def on_start(self, ctx: Context) -> None:
        if ctx.params["overlay"] not in OVERLAYS:
            raise ValueError(f"overlay must be one of {', '.join(OVERLAYS)}")

    def on_step(self, ctx: Context) -> None:
        assets: list[str] = ctx.params["assets"]
        cash: str = ctx.params["cash"]
        months = int(ctx.params["months"])
        top = int(ctx.params["top"])
        l_max = float(ctx.params["l_max"])
        band = float(ctx.params["band"])
        if "picks" not in ctx.state or last_session_of_month(ctx):
            if (picks := select(ctx, assets, cash, months, top)) is None:
                ctx.log(f"no {months}-month total returns for {cash} and the assets yet, no decision", level="warn")
                return
            ctx.state["picks"] = picks
        picks = dict[str, str](ctx.state["picks"])
        scale, overlay = l_max, f"fixed exposure {l_max:.2f}"
        market: str = ctx.params["market"]
        if ctx.params["overlay"] != "none":
            found = (
                votes(ctx, market) if ctx.params["overlay"] == "votes" else breadth_or_trend(ctx, self.cache, market)
            )
            if found is None:
                ctx.log(f"the {ctx.params['overlay']} overlay has no data yet, no decision", level="warn")
                return
            share, overlay = found
            scale = l_max * share
        leg = scale / top
        targets = dict.fromkeys(assets, 0.0) | dict.fromkeys(picks, leg)
        targets[cash] = max(0.0, 1 - leg * len(picks))
        summary = f"{len(picks)} of {top} slots held at {leg:.2f} each; {overlay}"
        ctx.log(summary)
        if not drifted(ctx, {symbol: weight for symbol, weight in targets.items() if weight > 0}, band):
            return
        ctx.rebalance(
            targets,
            reason={
                **{symbol: f"not a pick this month; {summary}" for symbol in assets},
                **{symbol: f"{why}; {summary}" for symbol, why in picks.items()},
                cash: f"the {targets[cash]:.2f} of equity not in the picks; {summary}",
            },
            exit_reason="not one of the assets or cash",
        )


if __name__ == "__main__":
    run(IndexMomentum)
