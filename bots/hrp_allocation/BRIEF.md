# hrp_allocation

Complex · Gaps: G5, G4 (levered variant)

## Idea
López de Prado's hierarchical risk parity (HRP):
1. Tree-cluster the assets on a distance built from their correlations.
2. Quasi-diagonalise the covariance matrix by the cluster leaf order.
3. Allocate by recursive bisection, splitting weight between the halves in inverse proportion to their variances.

It never inverts the covariance matrix. In the paper's Monte Carlo it beat minimum-variance (CLA) and inverse-variance (IVP) on out-of-sample variance. The bot applies it to a cross-asset ETF set, re-estimated every 22 sessions on 260 sessions of returns.

## Sources
- López de Prado (2016), "Building Diversified Portfolios that Outperform Out of Sample", *JPM* 42(4); SSRN 2708678, whose appendix code is the reference implementation.
- Replications: Raffinot (2017), "Hierarchical Clustering-Based Asset Allocation", *JPM*; Jain & Jain (2019), "Can Machine Learning-Based Portfolios Outperform Traditional Risk-Based Portfolios?", *Risks*.
- Baselines: Bailey & López de Prado (2013), the CLA implementation; Maillard, Roncalli & Teïletche (2010), equal risk contribution.
- Levered variant: Asness, Frazzini & Pedersen (2012), "Leverage Aversion and Risk Parity", *FAJ*.

## Manifest
```toml
description = "Hierarchical risk parity across asset classes"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "EFA", "EEM", "TLT", "IEF", "LQD", "HYG", "GLD", "DBC", "VNQ"] }
params = { lookback = 260, rebalance_sessions = 22, method = "hrp", target_vol = 0.0 }
```

## Logic
- Every 22 sessions, counted from the first session with a full window, estimate the covariance V and correlation ρ from the last 260 daily total returns. That is the paper's yearly window and monthly re-estimation, so no calendar is needed.
- Stage 1, tree clustering:
  - d_ij = √(½(1 − ρ_ij)).
  - Then the distance of distances, d̃_ij = √Σ_n (d_ni − d_nj)², the Euclidean distance between columns of D.
  - Single linkage (nearest point) on d̃. The paper's code gets this by passing the square matrix D to `scipy.cluster.hierarchy.linkage(D, "single")`, which treats its rows as observations. Compute d̃ explicitly and pass it condensed, and check the result matches.
- Stage 2: order the assets by the dendrogram's leaves (`getQuasiDiag`).
- Stage 3, recursive bisection on that ordered list:
  - Split each list into halves by position, with the first half of length int(n/2). The paper does not cut along the dendrogram.
  - Each half's variance is w̃′Vw̃, where w̃ is the inverse-variance weights within the half.
  - α = 1 − V₁/(V₁ + V₂). The first half's weights are multiplied by α and the second's by 1 − α.
- Target the weights, long-only, summing to 1. Reason: "HRP {w}, leaf {k}, split α {a}".
- `method` runs the fair baselines through the same code path, window, schedule and costs:
  - `ivp`: inverse variance;
  - `cla`: long-only, fully invested minimum variance, the paper's CLA;
  - `erc`: equal risk contribution;
  - `ew`: equal weight.
- Levered variant (`target_vol` > 0): scale the HRP book so its ex-ante volatility, from the same V, equals `target_vol`. Asness et al. lever risk parity to a balanced portfolio's risk; a sensible target is the ex-ante volatility of 60/40 SPY/IEF.
- Pitfalls:
  - Use total returns and the same 260 sessions for V and ρ.
  - Assets join the set only once they have 260 sessions of history (HYG and DBC start in 2006–2007).
  - Break ties in linkage deterministically by fixing the column order.

## Gaps
- G5: scipy.
- G4: only the levered variant, which goes above 1× gross.

## Done when
- The weight function reproduces the paper's Monte Carlo (Appendix A.4: 10 series, 520 observations, 260-observation window, rebalancing every 22, 10,000 runs). Out-of-sample variance should be about 72% higher for CLA than HRP and about 38% higher for IVP (paper: 0.1157, 0.0928 and 0.0671), within simulation error.
- Over 10y the PR compares HRP with IVP, CLA, ERC and EW on out-of-sample volatility, Sharpe, maximum drawdown, turnover and weight concentration (the paper's numerical example: top-5 weights 62.6% for HRP against 92.7% for CLA).
- Weights sum to 1 in the unlevered run.
