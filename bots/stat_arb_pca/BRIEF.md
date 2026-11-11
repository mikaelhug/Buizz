# stat_arb_pca

Complex · Gaps: G2, G3 (ETF variants), G4, G6, G10, G11

## Idea
Avellaneda–Lee statistical arbitrage:
- Explain each stock's daily returns by the top eigenportfolios of the stock correlation matrix (or by its sector ETF in a variant).
- Model the cumulated residual as an Ornstein–Uhlenbeck process and trade its standardised s-score.
- Bang-bang: buy cheap residuals and short rich ones at a fixed size per name, kept beta-neutral with SPY, at 2+2 leverage.

## Sources
- Avellaneda & Lee (2010), "Statistical Arbitrage in the US Equities Market", *Quantitative Finance* 10(7). The NYU preprint has the appendix with the estimation formulas.
- Guijarro-Ordonez, Pelger & Zanotti (2022), "Deep Learning Statistical Arbitrage" (arXiv 2106.04028): an independent replication of the PCA + OU threshold model, 2002–2016.
- Yeo & Papanicolaou (2017), "Risk control of mean-reversion time in statistical arbitrage", *Risk and Decision Analysis*.

## Manifest
```toml
description = "Avellaneda–Lee PCA residual s-scores"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 3000 }

[params]
min_market_cap = 1e9
corr_window = 252
factors = 15
explained_variance = 0.0
regression_window = 60
kappa_min = 8.4
s_open = 1.25
s_close_long = -0.50
s_close_short = 0.75
leverage_per_side = 2.0
expected_names_per_side = 100
trading_time = false
volume_window = 10
factor_model = "pca"
```

## Logic
- Universe on each trade date: common stocks with market cap above $1B on that date (`SharesOutstanding` × close), which is how the paper avoids survivorship. `top_dollar_volume` is only the superset. A stock enters the correlation only with a complete 252-day history.
- PCA (daily, at the close): standardise each stock's returns over the last 252 sessions, Y = (R − R̄)/σ. Build the correlation matrix and take the top `factors` = 15 eigenvectors v⁽ʲ⁾. The eigenportfolio weights are Q_i⁽ʲ⁾ = v_i⁽ʲ⁾/σ_i, and the factor returns are F_j = Σ_i Q_i⁽ʲ⁾ R_i.
- Residuals: regress each stock's last 60 daily returns on the F_j with an intercept. Cumulate the residuals into X_k, k = 1…60. X_60 = 0 by construction.
- OU fit (paper appendix): fit X_{n+1} = a + b·X_n + ζ over n = 1…59. Then κ = −ln(b)·252, m = a/(1 − b), σ_eq = √(Var(ζ)/(1 − b²)).
- Reject the stock when κ ≤ 252/30 = 8.4, i.e. a mean-reversion time of half the window or more. A rejected stock opens no trade, and an open trade in it is closed.
- Centred s-score: m̄_i = m_i − mean_j(m_j) across the stocks that pass. Then s_i = −m̄_i/σ_eq,i. The paper used this centring in every simulation.
- Rules: buy to open when s < −1.25, short to open when s > +1.25. Close a long when s > −0.50, close a short when s < +0.75. Reason: "s {s}, κ {k}, m̄ {m}".
- Sizing is bang-bang. Each new position is Λ·equity with Λ = `leverage_per_side`/`expected_names_per_side` = 2/100, set only when the position opens. The result is 2 dollars long and 2 short per dollar of equity on average.
- Hedge: rebalance a SPY position daily to keep the whole book beta-neutral. The paper hedged only with SPY in the PCA and synthetic-ETF runs, and hedged per sector with ETFs only in the actual-ETF run.
- Costs in the paper: 5 bps per side, all trades at that day's close.
- Variants (each reported separately, same rules):
  - `explained_variance = 0.55`: keep as many eigenvectors as explain 55% of the trace (10–30 over time). The paper found it slightly worse than 15.
  - `factor_model = "etf"`: regress each stock on its own sector ETF from the paper's 15 (Table 3), and hedge each position per sector with that ETF. Some of them no longer trade, so the agent documents the successor it maps each one to. Needs G3.
  - `factor_model = "synthetic_etf"`: cap-weighted sector indices built from the universe, hedged with SPY. Needs G3.
  - `trading_time = true`: replace each return by R·⟨δV⟩/δV_t, where ⟨δV⟩ is the trailing 10-session mean volume. Its best result was with ETF factors (Sharpe 1.51 over 2003–2007); with PCA the gain was not significant.
- Pitfalls:
  - The paper trades at the same close it measures. Next-open fills test a different, decayed strategy (G6).
  - Fix the BLAS thread count, sort symbols and fix the eigenvector signs, so the eigendecomposition is bit-reproducible.
  - Drift: the paper's modified s-score (s − α/(κσ_eq)) is not the tested rule. Keep α = 0.
  - Recompute everything from history each day. State holds only the open-position bookkeeping that the portfolio cannot reconstruct.

## Gaps
- G2: a stocks-only cross-section that still lets the bot trade SPY and the sector ETFs as hedges.
- G3: the actual-ETF and synthetic-ETF variants.
- G4: 2+2 leverage (4× gross) is the paper's book. Sharpe is leverage-invariant, but returns and drawdowns are not.
- G6: the paper trades at the signal close, and the edge decays over days (average reversion time about 7 days).
- G10: shorting about 100 names at a time.
- G11: survivorship caveat (delisted names).

## Done when
- Over 10y the PR reports, per year, the Sharpe before and after costs, turnover, average holding period and position counts. It sets them against:
  - A&L 15-PCA: Sharpe 1.44 over 1997–2007 and 0.9 over 2003–2007;
  - A&L ETF trading-time: 1.51 over 2003–2007;
  - Guijarro-Ordonez et al. OU + threshold on 15 PCA factors: Sharpe 0.62 before costs over 2002–2016.

  It discusses the decay.
- Estimation sanity against the paper:
  - average mean-reversion time about 7 days and σ_eq about 300 bps;
  - 10–30 factors needed to explain 55%;
  - realised volatility near the paper's roughly 10% at 2+2.
- A rerun reproduces the same result hash.
