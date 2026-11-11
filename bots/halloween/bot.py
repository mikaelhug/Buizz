from calendar import month_name
from datetime import timedelta

from buizz import Bot, Context, run


def span(first: int, end: int) -> str:
    last = (end - 2) % 12 + 1
    return month_name[first] if first == last else f"{month_name[first]} through {month_name[last]}"


class Halloween(Bot):
    def on_step(self, ctx: Context) -> None:
        risk_on: str = ctx.params["risk_on"]
        risk_off: str = ctx.params["risk_off"]
        enter: int = ctx.params["enter_month"]
        leave: int = ctx.params["exit_month"]
        today = ctx.session.date
        following, after = (session.date for session in ctx.calendar(today, today + timedelta(days=14))[1:3])
        winter = (after.month - enter) % 12 < (leave - enter) % 12
        hold, other = (risk_on, risk_off) if winter else (risk_off, risk_on)
        if ctx.position(hold).qty > 0 and ctx.position(other).qty == 0:
            return
        season = span(enter, leave) if winter else span(leave, enter)
        if following.month == after.month:
            occasion = "initial allocation"
        elif winter:
            occasion = f"Halloween entry at the close of {following}"
        else:
            occasion = f"sell in May at the close of {following}"
        if hold not in ctx.bars:
            if ctx.position(other).qty != 0:
                ctx.order_target_weight(other, 0, at="close", reason=f"{occasion}: out of {other} for {season}")
            if ctx.state.get("unpriced") != hold:
                ctx.log(f"no {hold} bar yet: in cash instead of {hold} for {season}", level="warn")
                ctx.state["unpriced"] = hold
            return
        ctx.state.pop("unpriced", None)
        ctx.rebalance(
            {other: 0, hold: 1},
            reason={other: f"{occasion}: out of {other} for {season}", hold: f"{occasion}: {hold} for {season}"},
            exit_reason="not part of the Halloween switch",
            at="close",
        )


if __name__ == "__main__":
    run(Halloween)
