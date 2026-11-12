from collections import Counter
from typing import Any, Literal, TypedDict

import numpy as np
from buizz_bots.sessions import week_ends

from buizz import Bot, Context, CorporateAction, Fill, History, run

TICK = 0.01
HISTORY_BARS = 250
MAX_CUTS = 50


class Unit(TypedDict):
    qty: int
    price: float
    stop: float


class Trade(TypedDict):
    side: int
    n: float
    units: list[Unit]


class Placed(TypedDict):
    symbol: str
    kind: Literal["unit", "exit"]
    side: int
    n: float
    units: list[int]


def wilder_n(history: History, period: int) -> float | None:
    if len(history.close) <= period:
        return None
    previous = history.close[:-1]
    true_range = np.maximum(history.high[1:], previous) - np.minimum(history.low[1:], previous)
    n = float(true_range[:period].mean())
    for value in true_range[period:].tolist():
        n = (n * (period - 1) + value) / period
    return n if n > 0 else None


def side_name(side: int) -> str:
    return "long" if side > 0 else "short"


def notional_account(ctx: Context) -> float:
    step: float = ctx.params["drawdown_step"]
    cut: float = ctx.params["drawdown_cut"]
    equity = float(ctx.portfolio.equity)
    year = ctx.session.date.year
    account: dict[str, Any] | None = ctx.state.get("account")
    if account is None or account["year"] != year:
        account = {"year": year, "base": equity, "cuts": 0}
        ctx.log(f"notional account for {year}: {equity:,.0f}")
    base: float = account["base"]
    cuts: int = account["cuts"]
    if equity >= base:
        cuts = 0
    else:
        while cuts < MAX_CUTS and equity <= base * (1 - step * sum((1 - cut) ** i for i in range(cuts + 1))):
            cuts += 1
    if cuts != account["cuts"]:
        ctx.log(f"equity {equity:,.0f} against the yearly {base:,.0f}: notional account cut {cuts} times")
    account["cuts"] = cuts
    ctx.state["account"] = account
    return base * (1 - cut) ** cuts


def fill_price(fills: list[Fill]) -> tuple[int, float]:
    shares = sum(fill.qty for fill in fills)
    value = sum(fill.price * fill.qty for fill in fills)
    return abs(shares), float(value / shares) if shares else 0.0


def split(trade: Trade, ratio: float) -> None:
    trade["n"] /= ratio
    units = trade["units"]
    total = int(sum(unit["qty"] for unit in units) * ratio)
    for unit in units:
        unit["qty"] = int(unit["qty"] * ratio)
        unit["price"] /= ratio
        unit["stop"] /= ratio
    units[-1]["qty"] += total - sum(unit["qty"] for unit in units)


def close_units(trades: dict[str, Trade], exits: list[tuple[Placed, list[Fill]]]) -> None:
    for order, filled in exits:
        trade = trades.get(order["symbol"])
        if trade is None:
            continue
        shares = abs(sum(fill.qty for fill in filled))
        for index in order["units"]:
            taken = min(shares, trade["units"][index]["qty"])
            trade["units"][index]["qty"] -= taken
            shares -= taken
    for symbol in list(trades):
        trades[symbol]["units"] = [unit for unit in trades[symbol]["units"] if unit["qty"] > 0]
        if not trades[symbol]["units"]:
            del trades[symbol]


def open_unit(trades: dict[str, Trade], order: Placed, filled: list[Fill], stop_n: float, add_step: float) -> None:
    shares, price = fill_price(filled)
    side = order["side"]
    trade = trades.get(order["symbol"])
    if trade is None or trade["side"] != side:
        trade = Trade(side=side, n=order["n"], units=[])
        trades[order["symbol"]] = trade
    for unit in trade["units"]:
        unit["stop"] += side * add_step * trade["n"]
    trade["units"].append(Unit(qty=shares, price=price, stop=price - side * stop_n * trade["n"]))


def unit_limit(symbol: str, side: int, counts: Counter[tuple[str, int]], params: dict[str, Any]) -> str | None:
    def load(symbols: list[str]) -> int:
        return sum(counts[(member, side)] for member in symbols)

    direction = side_name(side)
    if counts[(symbol, side)] >= params["max_units"]:
        return f"{symbol} already holds {params['max_units']} {direction} units"
    for groups, limit_key in (
        ("closely_correlated", "closely_correlated_units"),
        ("loosely_correlated", "loosely_correlated_units"),
    ):
        limit: int = params[limit_key]
        members: list[str]
        for group, members in params[groups].items():
            if symbol in members and load(members) >= limit:
                return f"{group} already holds {limit} {direction} units"
    total = sum(count for (_, held_side), count in counts.items() if held_side == side)
    if total >= params["direction_units"]:
        return f"the book already holds {params['direction_units']} {direction} units"
    return None


def entries(
    bars: History, trade: Trade | None, n: float, params: dict[str, Any]
) -> list[tuple[int, float, float, str]]:
    window: int = params["entry"]
    breakout = {1: float(bars.high[-window:].max()) + TICK, -1: float(bars.low[-window:].min()) - TICK}
    if trade is None:
        return [(side, breakout[side], n, f"{window}-day breakout {side_name(side)}") for side in (1, -1)]
    side = trade["side"]
    reverse = [(-side, breakout[-side], n, f"{window}-day breakout {side_name(-side)}")]
    held = len(trade["units"])
    if held >= params["max_units"]:
        return reverse
    last = trade["units"][-1]["price"]
    step: float = params["add_step"]
    pyramid = f"pyramid {side_name(side)} unit {held + 1} at {step:g}N of {trade['n']:.4f} past {last:.2f}"
    return [(side, last + side * step * trade["n"], trade["n"], pyramid), *reverse]


class DonchianBreakout(Bot):
    def on_step(self, ctx: Context) -> None:
        params = ctx.params
        trades: dict[str, Trade] = ctx.state.setdefault("trades", {})
        symbols = sorted(set(ctx.universe) | set(trades) | set(ctx.portfolio.positions))
        history = ctx.history(symbols, "1d", HISTORY_BARS, adjust="split")
        if "n" not in ctx.state or week_ends(ctx):
            weekly = {symbol: wilder_n(history[symbol], params["n_period"]) for symbol in symbols}
            ctx.state["n"] = {symbol: n for symbol, n in weekly.items() if n is not None}
        sheet: dict[str, float] = ctx.state["n"]
        self.book(ctx, trades)
        self.reconcile(ctx, trades, sheet)
        closes = {symbol: float(bars.close[-1]) for symbol, bars in history.items() if len(bars.close)}
        placed: dict[str, Placed] = {}
        for symbol, trade in trades.items():
            self.protect(ctx, symbol, trade, history[symbol], placed)
        self.enter(ctx, trades, sheet, history, closes, placed)
        ctx.state["placed"] = placed

    def book(self, ctx: Context, trades: dict[str, Trade]) -> None:
        placed: dict[str, Placed] = ctx.state.get("placed", {})
        fills: dict[str, list[Fill]] = {}
        for event in ctx.events:
            if isinstance(event, CorporateAction) and event.share_ratio is not None and event.symbol in trades:
                ratio = float(event.share_ratio)
                split(trades[event.symbol], ratio)
                for order in placed.values():
                    if order["symbol"] == event.symbol and order["kind"] == "unit":
                        order["n"] /= ratio
            elif isinstance(event, Fill) and event.client_id in placed:
                fills.setdefault(event.client_id, []).append(event)
        exits = [
            (placed[client_id], filled) for client_id, filled in fills.items() if placed[client_id]["kind"] == "exit"
        ]
        opened = [
            (placed[client_id], filled) for client_id, filled in fills.items() if placed[client_id]["kind"] == "unit"
        ]
        close_units(trades, exits)
        for order, filled in opened:
            open_unit(trades, order, filled, ctx.params["stop_n"], ctx.params["add_step"])

    def reconcile(self, ctx: Context, trades: dict[str, Trade], sheet: dict[str, float]) -> None:
        for symbol in sorted(set(trades) | set(ctx.portfolio.positions)):
            held = ctx.position(symbol).qty
            trade = trades.get(symbol)
            booked = trade["side"] * sum(unit["qty"] for unit in trade["units"]) if trade else 0
            if held == booked:
                continue
            ctx.log(f"{symbol} holds {held} shares but its units add up to {booked}: rebooked as one unit", "warn")
            if held == 0:
                del trades[symbol]
                continue
            side = 1 if held > 0 else -1
            n = trade["n"] if trade else sheet.get(symbol, 0.0)
            price = float(ctx.position(symbol).avg_cost)
            trades[symbol] = Trade(
                side=side, n=n, units=[Unit(qty=abs(held), price=price, stop=price - side * ctx.params["stop_n"] * n)]
            )

    def protect(self, ctx: Context, symbol: str, trade: Trade, history: History, placed: dict[str, Placed]) -> None:
        window: int = ctx.params["exit"]
        side = trade["side"]
        units = trade["units"]
        channel: float | None = None
        if len(history.close) >= window:
            channel = (
                float(history.low[-window:].min()) - TICK if side > 0 else float(history.high[-window:].max()) + TICK
            )
        levels: dict[tuple[float, str], list[int]] = {}
        for index, unit in enumerate(units):
            if channel is not None and (channel - unit["stop"]) * side > 0:
                key = (round(channel, 4), f"{window}-day exit")
            else:
                key = (round(unit["stop"], 4), f"{ctx.params['stop_n']:g}N stop")
            levels.setdefault(key, []).append(index)
        for (level, rule), indices in levels.items():
            shares = sum(units[index]["qty"] for index in indices)
            client_id = ctx.order(
                symbol,
                -side * shares,
                stop_price=level,
                reason=f"{rule}: {side_name(side)} {len(indices)} of {len(units)} units at {level:.2f}",
            )
            placed[client_id] = Placed(symbol=symbol, kind="exit", side=side, n=trade["n"], units=indices)

    def enter(
        self,
        ctx: Context,
        trades: dict[str, Trade],
        sheet: dict[str, float],
        history: dict[str, History],
        closes: dict[str, float],
        placed: dict[str, Placed],
    ) -> None:
        params = ctx.params
        window: int = params["entry"]
        equity = float(ctx.portfolio.equity)
        account = notional_account(ctx)
        counts: Counter[tuple[str, int]] = Counter(
            {(symbol, trade["side"]): len(trade["units"]) for symbol, trade in trades.items()}
        )
        committed = sum(
            abs(position.qty) * closes.get(symbol, 0.0) for symbol, position in ctx.portfolio.positions.items()
        )
        budget = params["max_gross"] * equity
        for symbol in ctx.universe:
            bars = history[symbol]
            n = sheet.get(symbol)
            if n is None or len(bars.close) < window or symbol not in closes:
                continue
            close = closes[symbol]
            shares = int(min(params["risk_per_unit"] * account / n, params["max_unit_weight"] * account / close))
            if shares <= 0:
                continue
            candidates = entries(bars, trades.get(symbol), n, params)
            extra = 0.0
            for side, price, unit_n, reason in candidates:
                if price <= 0:
                    continue
                blocked = unit_limit(symbol, side, counts, params)
                notional = shares * close
                if blocked is None and committed + max(extra, notional) > budget:
                    blocked = f"gross exposure would pass {params['max_gross']:g}x equity"
                if blocked is not None:
                    ctx.log(f"{symbol} {side_name(side)} unit skipped: {blocked}", "debug")
                    continue
                extra = max(extra, notional)
                counts[(symbol, side)] += 1
                client_id = ctx.order(
                    symbol,
                    side * shares,
                    stop_price=round(price, 4),
                    reason=f"{reason}: {shares} shares for N {n:.4f} and account {account:,.0f}",
                )
                placed[client_id] = Placed(symbol=symbol, kind="unit", side=side, n=unit_n, units=[])
            committed += extra


if __name__ == "__main__":
    run(DonchianBreakout)
