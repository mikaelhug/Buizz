import numpy as np
from buizz_bots.hrp import Matrix, bisection_shares, inverse_variance, quasi_diagonal
from scipy.optimize import minimize

from buizz import Bot, Context, run

SESSIONS_PER_YEAR = 252
MIN_ASSETS = 2
METHODS = {"hrp", "ivp", "cla", "erc", "ew"}


def minimum_variance(covariance: Matrix) -> Matrix:
    count = len(covariance)
    scaled = covariance / np.mean(np.diag(covariance))

    def variance(weights: Matrix) -> float:
        return float(weights @ scaled @ weights)

    def gradient(weights: Matrix) -> Matrix:
        return 2 * scaled @ weights

    def invested(weights: Matrix) -> float:
        return float(weights.sum() - 1)

    result = minimize(
        variance,
        np.full(count, 1 / count, dtype=np.float64),
        jac=gradient,
        method="SLSQP",
        bounds=[(0, 1)] * count,
        constraints=[{"type": "eq", "fun": invested, "jac": lambda _: np.ones(count)}],
        options={"ftol": 1e-15, "maxiter": 1000},
    )
    if not result.success:
        raise RuntimeError(f"minimum variance did not converge: {result.message}")
    weights = np.clip(result.x, 0, None)
    return weights / weights.sum()


def equal_risk_contribution(covariance: Matrix) -> Matrix:
    count = len(covariance)
    scaled = covariance / np.mean(np.diag(covariance))
    variances = np.diag(scaled)
    weights = 1 / np.sqrt(variances)
    for _ in range(10_000):
        previous = weights.copy()
        for i in range(count):
            others = scaled[i] @ weights - variances[i] * weights[i]
            weights[i] = (np.sqrt(others**2 + 4 * variances[i] / count) - others) / (2 * variances[i])
        if np.max(np.abs(weights - previous)) < 1e-12 * np.max(weights):
            return weights / weights.sum()
    raise RuntimeError("equal risk contribution did not converge in 10,000 sweeps")


def allocate(method: str, covariance: Matrix) -> tuple[Matrix, list[str]]:
    count = len(covariance)
    if method == "hrp":
        order = quasi_diagonal(covariance)
        shares = bisection_shares(covariance, order)
        weights = np.array([np.prod(path) for path in shares])
        return weights, [
            f"HRP {weights[i]:.1%}, leaf {order.index(i) + 1} of {count}, split alpha "
            + " x ".join(f"{share:.2f}" for share in shares[i])
            for i in range(count)
        ]
    if method == "ivp":
        weights = inverse_variance(covariance)
    elif method == "cla":
        weights = minimum_variance(covariance)
    elif method == "erc":
        weights = equal_risk_contribution(covariance)
    else:
        weights = np.full(count, 1 / count)
    return weights, [f"{method.upper()} {weight:.1%}" for weight in weights]


class HrpAllocation(Bot):
    def on_start(self, ctx: Context) -> None:
        if ctx.params["method"] not in METHODS:
            raise ValueError(f"method must be one of {sorted(METHODS)}, got {ctx.params['method']!r}")

    def on_step(self, ctx: Context) -> None:
        lookback: int = ctx.params["lookback"]
        every: int = ctx.params["rebalance_sessions"]
        target_vol = float(ctx.params["target_vol"])
        since: int | None = ctx.state.get("sessions_since_estimate")
        if since is not None and since + 1 < every:
            ctx.state["sessions_since_estimate"] = since + 1
            return
        symbols = list(ctx.universe)
        history = ctx.history(symbols, "1d", lookback + 1, adjust="all")
        full = [symbol for symbol in symbols if len(history[symbol].start) == lookback + 1]
        if not full:
            return
        latest = max(full, key=lambda symbol: history[symbol].start[-1])
        eligible = [symbol for symbol in full if np.array_equal(history[symbol].start, history[latest].start)]
        if len(eligible) < MIN_ASSETS:
            return
        closes = np.column_stack([history[symbol].close for symbol in eligible])
        covariance = np.cov(closes[1:] / closes[:-1] - 1, rowvar=False)
        weights, reasons = allocate(ctx.params["method"], covariance)
        scale = target_vol / np.sqrt(SESSIONS_PER_YEAR * weights @ covariance @ weights) if target_vol > 0 else 1.0
        targets = dict.fromkeys(symbols, 0.0) | {
            symbol: float(scale * weight) for symbol, weight in zip(eligible, weights, strict=True)
        }
        levered = f", levered x{scale:.2f} to {target_vol:.0%} ex-ante volatility" if target_vol > 0 else ""
        window = f"{lookback}-session window to {ctx.session.date}"
        why = {symbol: f"{reason}{levered}, {window}" for symbol, reason in zip(eligible, reasons, strict=True)}
        ctx.rebalance(
            targets,
            reason={symbol: why.get(symbol, f"no full {window}") for symbol in symbols},
            exit_reason="not part of the cross-asset set",
        )
        ctx.state["sessions_since_estimate"] = 0


if __name__ == "__main__":
    run(HrpAllocation)
