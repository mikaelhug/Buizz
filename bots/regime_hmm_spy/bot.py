from datetime import UTC, datetime

import numpy as np
from hmmlearn.hmm import GaussianHMM
from numpy.typing import NDArray

from buizz import Bot, Context, run

STOCKS, CASH = "SPY", "BIL"
EM_ITERATIONS = 100


def high_volatility_at_end(percent_returns: NDArray[np.float64], starts: int, seed: int) -> bool:
    observations = percent_returns.reshape(-1, 1)
    fits = [
        GaussianHMM(n_components=2, covariance_type="diag", n_iter=EM_ITERATIONS, random_state=seed + start).fit(
            observations
        )
        for start in range(starts)
    ]
    model = max(fits, key=lambda fit: fit.score(observations))
    _, states = model.decode(observations, algorithm="viterbi")
    return int(states[-1]) == int(np.argmax(model.covars_.reshape(2, -1)[:, 0]))


class RegimeHmmSpy(Bot):
    def on_step(self, ctx: Context) -> None:
        window: int = ctx.params["window"]
        votes_needed: int = ctx.params["votes"]
        history = ctx.history([STOCKS], "1d", window + votes_needed, adjust="all")[STOCKS]
        returns = 100 * np.diff(np.log(history.close))
        dates = [datetime.fromtimestamp(int(ns) / 1e9, UTC).date().isoformat() for ns in history.start[1:]]
        if len(returns) < window:
            return
        through: str | None = ctx.state.get("through")
        votes: list[bool] = ctx.state.get("votes", []) if through in dates else []
        first = dates.index(through) + 1 if through in dates else max(window - 1, len(returns) - votes_needed)
        for day in range(first, len(returns)):
            fitted = returns[day + 1 - window : day + 1]
            votes = [*votes, high_volatility_at_end(fitted, ctx.params["starts"], ctx.params["seed"])][-votes_needed:]
        ctx.state.update(votes=votes, through=dates[-1])
        if len(votes) < votes_needed:
            return
        turbulent = sum(votes)
        held = CASH if turbulent > votes_needed - turbulent else STOCKS
        if held == ctx.state.get("held"):
            return
        why = (
            f"{turbulent} of the last {votes_needed} Viterbi end states of {window}-session SPY fits to {dates[-1]} "
            f"were high volatility"
        )
        ctx.rebalance(
            {STOCKS if held == CASH else CASH: 0.0, held: 1.0},
            reason=f"{'risk-off' if held == CASH else 'risk-on'}, hold {held}: {why}",
            exit_reason="neither SPY nor BIL",
        )
        ctx.state["held"] = held


if __name__ == "__main__":
    run(RegimeHmmSpy)
