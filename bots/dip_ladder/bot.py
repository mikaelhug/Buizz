from decimal import Decimal
from typing import Any, NamedTuple

from buizz import Bot, Context, CorporateAction, Fill, run

type Lot = dict[str, Any]
type Lots = dict[str, dict[str, Lot]]


class Rules(NamedTuple):
    rungs: list[float]
    rung_weight: float
    take_profit: float
    max_hold: int


def apply_fill(lots: Lots, rung: str, fill: Fill) -> None:
    held = lots.setdefault(fill.symbol, {})
    lot = held.get(rung)
    price = float(fill.price)
    if lot is None:
        if fill.qty > 0:
            held[rung] = {"qty": fill.qty, "cost": price, "held": 0}
        return
    if fill.qty > 0:
        lot["cost"] = (lot["cost"] * lot["qty"] + price * fill.qty) / (lot["qty"] + fill.qty)
    lot["qty"] += fill.qty
    if lot["qty"] <= 0:
        del held[rung]


def apply_split(lots: Lots, action: CorporateAction) -> None:
    if action.kind["type"] != "split":
        return
    ratio = Decimal(action.kind["new_rate"]) / Decimal(action.kind["old_rate"])
    for lot in lots.get(action.symbol, {}).values():
        lot["qty"] = int(lot["qty"] * ratio)
        lot["cost"] = float(Decimal(str(lot["cost"])) / ratio)


def settle(ctx: Context, lots: Lots, placed: dict[str, str]) -> None:
    for event in ctx.events:
        if isinstance(event, Fill) and event.client_id in placed:
            apply_fill(lots, placed[event.client_id], event)
        elif isinstance(event, CorporateAction):
            apply_split(lots, event)
    for symbol in list(lots):
        if ctx.position(symbol).qty <= 0:
            del lots[symbol]
            continue
        for lot in lots[symbol].values():
            lot["held"] += 1


def close_lot(ctx: Context, symbol: str, rung: str, lot: Lot, rules: Rules) -> str:
    depth = f"-{rules.rungs[int(rung)]:.0%} rung"
    day = ctx.session.date.isoformat()
    if lot["held"] >= rules.max_hold:
        return ctx.order(
            symbol,
            -lot["qty"],
            client_id=f"{symbol}-exit{rung}-{day}",
            reason=f"time exit: {depth} held {lot['held']} sessions without reaching +{rules.take_profit:.0%}",
        )
    target = lot["cost"] * (1 + rules.take_profit)
    return ctx.order(
        symbol,
        -lot["qty"],
        limit_price=target,
        client_id=f"{symbol}-tp{rung}-{day}",
        reason=f"take profit: {depth} bought at {lot['cost']:.2f}, +{rules.take_profit:.0%} is {target:.2f}",
    )


class DipLadder(Bot):
    def on_step(self, ctx: Context) -> None:
        rules = Rules(ctx.params["rungs"], ctx.params["rung_weight"], ctx.params["take_profit"], ctx.params["max_hold"])
        lots: Lots = ctx.state.get("lots", {})
        settle(ctx, lots, ctx.state.get("orders", {}))
        equity = float(ctx.portfolio.equity)
        day = ctx.session.date.isoformat()
        orders: dict[str, str] = {}
        for symbol in ctx.universe:
            held = lots.get(symbol, {})
            for rung, lot in sorted(held.items()):
                orders[close_lot(ctx, symbol, rung, lot, rules)] = rung
            bar = ctx.bars.get(symbol)
            if bar is None:
                continue
            for index, depth in enumerate(rules.rungs):
                rung = str(index)
                limit = bar.close * (1 - depth)
                qty = int(rules.rung_weight * equity / limit)
                if rung in held or qty <= 0:
                    continue
                client_id = ctx.order(
                    symbol,
                    qty,
                    limit_price=limit,
                    client_id=f"{symbol}-buy{rung}-{day}",
                    reason=f"dip rung -{depth:.0%}: {limit:.2f} is {depth:.0%} below the prior close {bar.close:.2f}",
                )
                orders[client_id] = rung
        ctx.state["lots"] = lots
        ctx.state["orders"] = orders


if __name__ == "__main__":
    run(DipLadder)
