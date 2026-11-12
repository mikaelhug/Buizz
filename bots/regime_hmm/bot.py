import math
from dataclasses import dataclass
from typing import Any

import numpy as np
from buizz_bots.monthly import session_days
from numpy.typing import NDArray
from scipy.optimize import minimize
from scipy.special import expit, logit

from buizz import Bot, Context, run

Array = NDArray[np.float64]

TUNING = 1.25
SPAN_MEMORIES = 10
STARTS = 10
MAX_BARS = 10_000
FINITE_STEP = 1e-5
PARAMETERS = 6
VARIANTS = {"rbaa", "median_filter"}
ON, OFF = "risk-on", "risk-off"


@dataclass(frozen=True)
class Hmm:
    means: Array
    variances: Array
    leave: Array

    @classmethod
    def of(cls, theta: Array) -> "Hmm":
        return cls(theta[0:2], np.exp(theta[2:4]), expit(theta[4:6]))

    @property
    def transition(self) -> Array:
        return np.array([[1 - self.leave[0], self.leave[0]], [self.leave[1], 1 - self.leave[1]]])

    @property
    def stationary(self) -> Array:
        return self.leave[::-1] / self.leave.sum()

    @property
    def calm(self) -> int:
        return int(np.argmin(self.variances))


def scaled_densities(hmm: Hmm, returns: Array) -> tuple[Array, Array, Array]:
    deviations = returns[:, None] - hmm.means
    log_density = -0.5 * (np.log(2 * np.pi * hmm.variances) + deviations**2 / hmm.variances)
    peak = log_density.max(axis=1)
    return np.exp(log_density - peak[:, None]), peak, deviations


def forward(hmm: Hmm, scaled: Array) -> tuple[Array, Array]:
    (stay0, move0), (move1, stay1) = hmm.transition.tolist()
    first, second = hmm.stationary.tolist()
    filtered = [(first, second)]
    norms: list[float] = []
    for density0, density1 in scaled.tolist():
        joint0 = (first * stay0 + second * move1) * density0
        joint1 = (first * move0 + second * stay1) * density1
        norm = joint0 + joint1
        first, second = joint0 / norm, joint1 / norm
        filtered.append((first, second))
        norms.append(norm)
    return np.array(filtered), np.array(norms)


def weighted_score(theta: Array, returns: Array, weights: Array) -> tuple[float, Array, Array]:
    hmm = Hmm.of(theta)
    scaled, peak, deviations = scaled_densities(hmm, returns)
    filtered, norms = forward(hmm, scaled)
    count = len(returns)
    density_gradient = np.zeros((count, PARAMETERS, 2))
    density_gradient[:, 0, 0] = deviations[:, 0] / hmm.variances[0]
    density_gradient[:, 1, 1] = deviations[:, 1] / hmm.variances[1]
    density_gradient[:, 2, 0] = (deviations[:, 0] ** 2 / hmm.variances[0] - 1) / 2
    density_gradient[:, 3, 1] = (deviations[:, 1] ** 2 / hmm.variances[1] - 1) / 2
    slope = hmm.leave * (1 - hmm.leave)
    transition_gradient = np.zeros((PARAMETERS, 2, 2))
    transition_gradient[4, 0] = [-slope[0], slope[0]]
    transition_gradient[5, 1] = [slope[1], -slope[1]]
    total = hmm.leave.sum()
    start_slope = np.array([-hmm.leave[1] * slope[0], hmm.leave[0] * slope[1]]) / total**2
    start_gradient = np.zeros((PARAMETERS, 2))
    start_gradient[4:6] = np.column_stack([start_slope, -start_slope])
    previous = filtered[:-1]
    ratio = scaled / norms[:, None]
    carried = (
        np.einsum("ti,kij->tkj", previous, transition_gradient)
        + (previous @ hmm.transition)[:, None, :] * density_gradient
    ) * ratio[:, None, :]
    propagator = hmm.transition[None, :, :] * ratio[:, None, :]
    derivative = start_gradient
    cumulative = np.empty((count, PARAMETERS))
    for t in range(count):
        derivative = derivative @ propagator[t] + carried[t]
        cumulative[t] = derivative.sum(axis=1)
    scores = np.diff(cumulative, axis=0, prepend=np.zeros((1, PARAMETERS)))
    return float(weights @ (np.log(norms) + peak)), weights @ scores, scores[-1]


def filtered_state(theta: Array, returns: Array) -> int:
    hmm = Hmm.of(theta)
    filtered, _ = forward(hmm, scaled_densities(hmm, returns)[0])
    return int(np.argmax(filtered[-1]))


def online_step(theta: Array, returns: Array, decided: int | None, threshold: float) -> list[tuple[int, float]]:
    hmm = Hmm.of(theta)
    with np.errstate(divide="ignore"):
        log_densities = np.log(scaled_densities(hmm, returns)[0])
        log_transition = np.log(hmm.transition)
        log_stationary = np.log(hmm.stationary)
    states = np.arange(2)
    classified: list[tuple[int, float]] = []
    for start in range(len(returns)):
        score = (log_stationary if decided is None else log_transition[decided]) + log_densities[start]
        origin = states
        for day in range(start + 1, len(returns)):
            best = np.argmax(score[:, None] + log_transition, axis=0)
            score = score[best] + log_transition[best, states] + log_densities[day]
            origin = origin[best]
        weight = np.exp(score - score.max())
        mass = [float(weight[origin == state].sum() / weight.sum()) for state in states]
        state = int(np.argmax(mass))
        if mass[state] <= threshold:
            break
        classified.append((state, mass[state]))
        decided = state
    return classified


def forgetting_weights(count: int, memory: int) -> Array:
    return (1 - 1 / memory) ** np.arange(count - 1, -1, -1, dtype=np.float64)


def initial_fit(returns: Array, memory: int, seed: int) -> tuple[Array, Array]:
    weights = forgetting_weights(len(returns), memory)
    mean = float(weights @ returns / weights.sum())
    variance = float(weights @ (returns - mean) ** 2 / weights.sum())

    def objective(theta: Array) -> tuple[float, Array]:
        with np.errstate(all="ignore"):
            loglik, gradient, _ = weighted_score(theta, returns, weights)
        if not (np.isfinite(loglik) and np.all(np.isfinite(gradient))):
            return np.inf, np.zeros(PARAMETERS)
        return -loglik, -gradient

    def hessian(theta: Array) -> Array:
        columns = [
            (weighted_score(theta + step, returns, weights)[1] - weighted_score(theta - step, returns, weights)[1])
            / (2 * FINITE_STEP)
            for step in np.eye(PARAMETERS) * FINITE_STEP
        ]
        matrix = np.column_stack(columns)
        return (matrix + matrix.T) / 2

    rng = np.random.default_rng(seed)
    best: tuple[float, Array, Array] | None = None
    for _ in range(STARTS):
        start = np.concatenate(
            [
                mean + rng.normal(0, np.sqrt(variance) / 10, 2),
                np.log(variance * np.array([rng.uniform(0.2, 0.8), rng.uniform(1.5, 4.0)])),
                logit(rng.uniform(0.005, 0.1, 2)),
            ]
        )
        result = minimize(objective, start, jac=True, method="BFGS")
        theta = np.asarray(result.x, dtype=np.float64)
        if not np.isfinite(result.fun):
            continue
        curvature = hessian(theta)
        if not np.all(np.linalg.eigvalsh(curvature) < 0):
            continue
        if best is None or -result.fun > best[0]:
            best = (-float(result.fun), theta, curvature)
    if best is None:
        raise RuntimeError(f"none of the {STARTS} starts of the initial fit reached a local maximum")
    return best[1], -best[2] / weights.sum()


class RegimeHmm(Bot):
    def on_start(self, ctx: Context) -> None:
        if ctx.params["variant"] not in VARIANTS:
            raise ValueError(f"variant must be one of {sorted(VARIANTS)}, got {ctx.params['variant']!r}")
        if not math.isclose(sum(ctx.params["saa"].values()), 1):
            raise ValueError("the saa weights must sum to 1")

    def on_step(self, ctx: Context) -> None:
        signal = self.hmm_signal(ctx)
        if signal is not None:
            self.allocate(ctx, *signal)

    def hmm_signal(self, ctx: Context) -> tuple[str, str] | None:
        symbol: str = ctx.params["signal"]
        memory: int = ctx.params["effective_memory"]
        initial: int = ctx.params["init_sessions"]
        threshold = float(ctx.params["threshold"])
        median_days: int = ctx.params["median_days"]
        online = ctx.params["variant"] == "rbaa"
        history = ctx.history([symbol], "1d", MAX_BARS, adjust="all")[symbol]
        returns = np.diff(np.log(history.close))
        dates: list[str] = session_days(history).astype(str).tolist()[1:]
        state = ctx.state
        if "theta" not in state and (len(returns) <= initial or not self.fit(ctx, returns, dates)):
            return None
        if state["through"] not in dates:
            raise RuntimeError(f"{symbol} history no longer holds {state['through']}, the last estimated day")
        theta = np.array(state["theta"], dtype=np.float64)
        information = np.array(state["information"], dtype=np.float64)
        through = dates.index(state["through"])
        unclassified = through + 1 if state["unclassified"] is None else dates.index(state["unclassified"])
        classified: dict[str, Any] | None = state["classified"]
        votes: list[str] = state["votes"]
        span = SPAN_MEMORIES * memory
        for day in range(through + 1, len(returns)):
            count = day + 1
            window = returns[max(0, count - span) : count]
            _, gradient, latest = weighted_score(theta, window, forgetting_weights(len(window), memory))
            step = 1 / min(count, memory)
            information += step * (np.outer(latest, latest) - information)
            theta = theta + TUNING * step * np.linalg.solve(information, gradient)
            calm = Hmm.of(theta).calm
            if online:
                decided = None if classified is None else calm if classified["regime"] == ON else 1 - calm
                decisions = online_step(theta, returns[unclassified:count], decided, threshold)
                for state_index, certainty in decisions:
                    classified = {
                        "date": dates[unclassified],
                        "regime": ON if state_index == calm else OFF,
                        "confidence": certainty,
                        "at": dates[day],
                    }
                    unclassified += 1
                if decisions and day == len(returns) - 1:
                    regimes = ", ".join(ON if state_index == calm else OFF for state_index, _ in decisions)
                    ctx.log(f"classified {dates[unclassified - len(decisions)]} onwards: {regimes}", level="debug")
            else:
                votes = [*votes, ON if filtered_state(theta, window) == calm else OFF][-median_days:]
        state.update(
            theta=theta.tolist(),
            information=information.tolist(),
            through=dates[-1],
            unclassified=dates[unclassified] if unclassified < len(dates) else None,
            classified=classified,
            votes=votes,
        )
        if online:
            if classified is None:
                return None
            return classified["regime"], (
                f"{classified['regime']} regime on {classified['date']}, confidence {classified['confidence']:.5f}, "
                f"classified at the close of {classified['at']}"
            )
        if len(votes) < median_days:
            return None
        regime = ON if votes.count(ON) > median_days // 2 else OFF
        return regime, f"{regime} regime by the median of the {median_days} filtered states to {dates[-1]}"

    def fit(self, ctx: Context, returns: Array, dates: list[str]) -> bool:
        initial: int = ctx.params["init_sessions"]
        state = ctx.state
        end = len(returns) if state.get("fit_failed") else initial
        fitted = f"{initial} returns of {ctx.params['signal']}, {dates[end - initial]} to {dates[end - 1]}"
        try:
            theta, information = initial_fit(
                returns[end - initial : end], ctx.params["effective_memory"], int(ctx.params["seed"])
            )
        except RuntimeError as error:
            ctx.log(f"initial fit on the {fitted} failed, retrying on the latest ones: {error}", level="warn")
            state["fit_failed"] = True
            return False
        state.pop("fit_failed", None)
        state.update(
            theta=theta.tolist(),
            information=information.tolist(),
            through=dates[end - 1],
            unclassified=dates[end] if end < len(dates) else None,
            classified=None,
            votes=[],
        )
        ctx.log(f"initial fit on the {fitted}")
        return True

    def allocate(self, ctx: Context, regime: str, why: str) -> None:
        delay: int = ctx.params["delay"]
        state = ctx.state
        if regime == state.get("held"):
            state["pending"] = None
            self.restore(ctx, regime)
            return
        pending: dict[str, Any] | None = state.get("pending")
        if pending is None or pending["regime"] != regime:
            pending = {"regime": regime, "why": why, "wait": delay}
        if pending["wait"] > 0:
            state["pending"] = {**pending, "wait": pending["wait"] - 1}
            return
        targets = self.targets(ctx, regime)
        delayed = f", traded {delay} session{'s' if delay != 1 else ''} late" if delay else ""
        ctx.rebalance(
            targets,
            reason={symbol: f"{pending['why']}{delayed}: {symbol} {weight:.1%}" for symbol, weight in targets.items()},
            exit_reason="not part of the regime allocation",
        )
        ctx.log(f"{state.get('held') or 'no allocation'} -> {regime}: {pending['why']}")
        state.update(held=regime, pending=None)

    def restore(self, ctx: Context, regime: str) -> None:
        band = float(ctx.params["drift_band"])
        targets = self.targets(ctx, regime)
        equity = float(ctx.portfolio.equity)
        ordered = {order.symbol for order in ctx.portfolio.working}
        weights = {
            symbol: ctx.position(symbol).qty * ctx.bars[symbol].close / equity
            for symbol in targets
            if symbol in ctx.bars and symbol not in ordered
        }
        off = [
            f"{symbol} {weight:.1%} against {targets[symbol]:.1%}"
            for symbol, weight in weights.items()
            if (weight == 0 and targets[symbol] > 0) or abs(weight - targets[symbol]) > band
        ]
        if not off:
            return
        why = f"{regime} regime held, {', '.join(off)}"
        ctx.rebalance(
            targets,
            reason={symbol: f"{why}: {symbol} {weight:.1%}" for symbol, weight in targets.items()},
            exit_reason="not part of the regime allocation",
        )
        ctx.log(f"back to the {regime} allocation: {', '.join(off)}")

    def targets(self, ctx: Context, regime: str) -> dict[str, float]:
        saa: dict[str, float] = {symbol: float(weight) for symbol, weight in ctx.params["saa"].items()}
        p = float(ctx.params["p"])
        risky = set(ctx.params["risky"])
        side = {symbol for symbol in saa if (symbol in risky) == (regime == ON)}
        share = sum(saa[symbol] for symbol in side)
        return {
            symbol: p * (weight / share if symbol in side else 0.0) + (1 - p) * weight for symbol, weight in saa.items()
        }


if __name__ == "__main__":
    run(RegimeHmm)
