# regime_hmm

Complex · Gaps: G5

## Idea
Nystrup et al.'s regime-based asset allocation (RBAA):
- A two-state Gaussian hidden Markov model with time-varying parameters, estimated adaptively by exponential forgetting, identifies low- and high-volatility regimes in daily developed-market stock returns.
- An online step algorithm decodes each regime once it is certain.
- The allocation tilts a 10-asset 60/40 portfolio to risk-on in calm regimes and to risk-off in turbulent ones, and rebalances only when the regime changes.

## Sources
- Nystrup, Hansen, Larsen, Madsen & Lindström (2017), "Dynamic Allocation or Diversification: A Regime-Based Approach to Multiple Assets", *Journal of Portfolio Management* 44(2). This is the primary specification, reprinted as Paper D of Nystrup's DTU PhD thesis (2017), which is open access.
- Nystrup, Madsen & Lindström (2017), "Long Memory of Financial Time Series and Hidden Markov Models with Time-Varying Parameters", *Journal of Forecasting* 36(8): the recursive, adaptive estimator.
- Narasimhan, Viola & Shilman (2006), the online step decoding algorithm; Lystig & Hughes (2002), the exact HMM score and information.
- Precursor: Nystrup, Hansen, Madsen & Lindström (2015), "Regime-Based versus Static Asset Allocation: Letting the Data Speak", *JPM* 42(1).
- Replication: Shu, Yu & Mulvey (2024), "Downside risk reduction using regime-switching signals: a statistical jump model approach", *Journal of Asset Management* (arXiv 2402.05272), with a hmmlearn HMM baseline on the S&P 500 over 1990–2023. Bulla et al. (2011), *Journal of Asset Management*: the sliding-window and median-filter approach.
- Not used: Kritzman, Page & Turkington (2012) find regimes in turbulence, inflation and growth series, not in equity returns, so they do not define this bot.

## Manifest
```toml
description = "Nystrup regime-based allocation: adaptive 2-state HMM on DM stocks"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["URTH", "EEM", "RWO", "HYG", "EMHY", "USO", "GLD", "LQD", "TIP", "BNDX"] }

[params]
signal = "URTH"
p = 0.5
effective_memory = 520
threshold = 0.9998
init_sessions = 252
saa = { URTH = 0.25, EEM = 0.05, RWO = 0.10, HYG = 0.05, EMHY = 0.05, USO = 0.05, GLD = 0.05, LQD = 0.10, TIP = 0.10, BNDX = 0.20 }
risky = ["URTH", "EEM", "RWO", "HYG", "EMHY", "USO", "GLD"]
seed = 7
```

## Logic
- Assets: ETF proxies of the paper's 10 indices. MSCI World is URTH, MSCI EM is EEM, developed real estate is RWO, US high yield is HYG, EM high yield is EMHY, GSCI crude oil is USO, GSCI gold is GLD, US corporate is LQD, world inflation-linked is TIP and global government is BNDX. The agent checks each proxy against its index and documents any mismatch.
- Signal: daily log returns of the developed-market stock proxy (`signal`).
- Model: two states with Gaussian conditional distributions and transition matrix Γ. θ = (μ₁, μ₂, σ₁², σ₂², γ₁₂, γ₂₁), estimated on transformed scales: log variances, logit probabilities.
- Estimation, from the J. Forecasting paper:
  - Initialise by maximum likelihood on the first `init_sessions` returns (the paper used 1997), from several seeded starts, keeping the best likelihood.
  - Then, each day, take one recursive Newton step: θ̂_t = θ̂_{t−1} + A/min(t, N_eff) · I_t⁻¹ ∇ℓ̃_t.
  - ℓ̃_t is the log-likelihood weighted by λ^{t−n}, with λ = 1 − 1/N_eff and N_eff = 520. The score comes from Lystig–Hughes over a trailing span of about 10·N_eff observations. I_t is the recursively averaged outer product of the scores.
  - The agent takes A and the start-up rules from the paper.
- Decoding with the online step algorithm:
  - After each close T, compute Pr(X_t = i | Y_1…Y_T) for the oldest unclassified day t. If the maximum exceeds `threshold`, classify t and move on to t + 1. Otherwise wait for the next observation.
  - When several days are classified at once, the latest one decides.
  - The paper's threshold is 1 − 1/T ≈ 0.9998. A fixed 0.9998 keeps it free of lookahead.
- Allocation: w = p·w_regime + (1 − p)·w_SAA.
  - w_on spreads 100% over the risky assets in proportion to their SAA weights (25, 5, 10, 5, 5, 5, 5 out of 60). w_off spreads it over the bond assets (10, 10, 20 out of 40).
  - With p = 0.5 the risky share is 80% in risk-on and 30% in risk-off.
  - Rebalance only when the classified regime differs from the one the current allocation rests on. Reason: "regime {r} on {date}, confidence {c}".
- Execution: the paper trades at the close of T + 1, a one-day delay. Buizz fills at the open of T + 1. Regimes last weeks to years (median detection delay 25 days), so under the execution rule this needs no G6. The PR adds a run with one extra day of delay.
- Variants:
  - `p = 0.8`: the Sharpe-maximising allocation in the paper.
  - `spy_switch` (Shu, Yu & Mulvey's HMM baseline):
    - universe SPY and BIL;
    - each day, refit a 2-state Gaussian HMM (`hmmlearn`) on the last 3,000 daily log total returns, from 10 k-means++ starts, keeping the best likelihood;
    - take the last Viterbi state of that window and apply Bulla's median filter (high-volatility only when the mean of the last 6 states exceeds 0.5);
    - hold 100% SPY in the low-volatility state and 100% BIL otherwise.
- Pitfalls:
  - Lookahead lives in the data, not in the algorithm's name. Pr(X_t | Y_1…Y_T) for t < T, and Viterbi on a window ending at the clock, are causal. Parameters or state paths computed from any observation after the clock are not.
  - The recursive estimate depends on its start date. Anchor the initialisation at the signal series' first `init_sessions`, never at the run start, and replay the recursion deterministically on resume.
  - State holds θ̂, I_t, the oldest unclassified day and the current regime, well under 64KB.

## Gaps
- G5: scipy, and hmmlearn for the initial fit and the `spy_switch` variant. hmmlearn has no exponential forgetting, so the adaptive estimator is the bot's own code.

## Done when
- A lookahead self-check shows that classifications made at T are identical whether or not data after T exists.
- Over 10y the PR compares the RBAA portfolio at p = 0.5 with the 60/40 SAA portfolio, rebalanced at the same change points, and reports return, volatility, Sharpe, maximum drawdown, Calmar and annual turnover. Paper (1998–2015): RBAA Sharpe 0.87 against 0.70 for SAA, maximum drawdown 17% against 30%, turnover 0.92 a year.
- It reports regime statistics against the paper: about 1.9 regime changes a year (34 in 18 years), risk-on about 61% of the time, and median classification and detection delays of 7 and 25 days.
- It runs the median-filter decoder as a baseline: the median of the last 21 filtered most-likely states, with the same HMM (paper: Sharpe 0.76, turnover 1.06).
- It runs `spy_switch` and compares it with Shu et al.'s S&P 500 HMM result: Sharpe 0.54 against 0.48 for buy-and-hold, maximum drawdown −28.9% against −55.2%, turnover 141%.
