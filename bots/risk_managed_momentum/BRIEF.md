# risk_managed_momentum

Complex · Gaps: G1, G2, G4, G10, G11, G17

## Idea
Cross-sectional momentum (winners minus losers, WML, on returns from t−12 to t−2 months) earns a large premium but suffers rare, violent crashes in rebounds after bear markets. Barroso & Santa-Clara scale WML each month to a constant 12% volatility target, using the realised variance of its own last 126 daily returns. That nearly doubles the Sharpe ratio and removes most of the crash risk. The Daniel–Moskowitz variant also forecasts WML's mean, which collapses in volatile bear markets, and sizes by μ/σ².

## Sources
- Barroso & Santa-Clara (2015), "Momentum Has Its Moments", *JFE* 116(1), plus the authors' erratum on the turnover formula (eq. 10). Primary.
- Daniel & Moskowitz (2016), "Momentum Crashes", *JFE* 122(2). Variant.
- Jegadeesh & Titman (1993), *JF*: base momentum.
- Ken French data library, "10 Portfolios Formed on Momentum" (daily, value-weighted, NYSE breakpoints): the comparison WML.
- Overlap: `momentum_12_1` is a long-only, unscaled example bot. This bot shares only the ranking signal.

## Manifest
```toml
description = "Vol-scaled winners-minus-losers with a crash guard"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 3000 }
params = { skip_days = 21, lookback_days = 252, min_months = 8, decile = 0.1, target_vol = 0.12, vol_days = 126, variant = "barroso" }
```

## Logic
- Universe: all NYSE, AMEX and Nasdaq common stocks in the source, so take the widest stock-only cross-section. A stock needs a valid price and shares outstanding at formation, and at least 8 monthly returns over the 11-month formation window (DM).
- At the last session of each month t, the signal is the cumulative total return from month t−12 to t−2: from 252 to 21 sessions back, skipping the most recent month.
- Deciles use NYSE breakpoints (French's deciles, which B&SC use). Winners are decile 10 and losers decile 1.
- Each leg is value-weighted by market cap at formation (`SharesOutstanding` × close, split-adjusted). WML = $1 long winners and $1 short losers.
- Track WML's daily return from buy-and-hold leg weights within the month. Keep the last 126 values in state.
- Barroso (primary):
  - Variance forecast σ̂²_t = 21 × Σ_{j=1..126} r²_{WML,d−j} / 126, a monthly variance with the mean not subtracted.
  - Scale_t = (0.12 / √12) / σ̂_t, fixed for the whole month. Long and short legs are each scale × equity.
  - B&SC's scale ranged from 0.13 to 2.00, averaging 0.90, so gross can reach 4×.
  - Reason: "WML decile {d}, scale {s}, σ̂ {v}".
- Daniel–Moskowitz variant (`variant = "dm"`):
  - Weight w_t = (1/2λ) μ̂_t / σ̂²_t.
  - μ̂ comes from DM Table 5 col. 4: R̃_WML = γ₀ + γ_int · I_B · σ̂²_m, with γ₀ = 1.973%/month and γ_int = −0.397. I_B = 1 if VTI's cumulative 24-month return is negative; σ̂²_m is the variance of the market's daily returns over the prior 126 days.
  - σ̂ is DM's forecast of next-month WML volatility: 0.0010 + 0.6114 σ̂_GJR + 0.2640 σ̂_126. GJR-GARCH(1,1) runs on daily WML with DM Appendix D parameters: μ = 0.86e-3, ω = 1.17e-6, α = 0.111, γ = −0.016, β = 0.896.
  - Verify the units of γ_int against the JFE tables before use.
  - λ is set so that ex-ante volatility targets 19% annualised (DM's choice), using only data before each formation, never the full run.
- Warm-up: the first trade comes after 126 daily WML returns have been tracked from the run start (about 6 months of formations, no orders). Rebuilding past formations from today's universe would add lookahead.
- Pitfalls:
  - Skip the most recent month.
  - Use adjusted closes for ranking and raw prices for fills.
  - Shares must be split-adjusted for the value weights.
  - Delisted losers must liquidate at last close.

## Gaps
- G1: month-end formation.
- G2: stocks only.
- G4: scale up to 2.0, so up to 4× gross.
- G10: the loser leg is small, distressed and often hard to borrow.
- G11: the CRSP universe; survivorship caveat.
- G17: NYSE listing per instrument, for NYSE decile breakpoints.

## Done when
- Over the run, the PR tabulates unscaled WML versus scaled WML: mean, volatility, Sharpe, skew, excess kurtosis, worst month and max drawdown.
  - B&SC (1927–2011): Sharpe 0.53 → 0.97; volatility 27.5% → 17.0%; skew −2.47 → −0.42; excess kurtosis 18.24 → 2.68; worst month −80% → −28%.
  - Realised volatility of the scaled series lands above the 12% target, as it did in B&SC. The check is a much more stable rolling 6-month volatility, not 12% exactly.
- Unscaled WML correlates above 0.9 with French's daily 10-minus-1 decile spread over the same days.
- The November 2020 rebound, and March–May 2009 if the window covers it, show smaller losses for scaled than unscaled WML.
- The `dm` variant is run on the same window. The PR reports whether the Sharpe ordering is dyn > cvol > WML, as in DM.
