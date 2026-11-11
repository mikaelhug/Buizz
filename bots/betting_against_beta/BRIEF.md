# betting_against_beta

Complex · Gaps: G1, G2, G4, G10, G11

## Idea
Leverage-constrained investors bid up high-beta stocks, so low-beta stocks earn higher risk-adjusted returns. The BAB factor goes long low-beta stocks levered to beta 1 and short high-beta stocks de-levered to beta 1. It is beta-neutral ex ante but not dollar-neutral: the long side is larger and financed at the margin rate.

## Sources
- Frazzini & Pedersen (2014), "Betting Against Beta", *JFE* 111(1) (FP). US equities 1926–2012, Table III.
- Novy-Marx & Velikov (2022), "Betting against betting against beta", *JFE* 143(1) (NMV): replication and construction critique.
- AQR data set "Betting Against Beta: Equity Factors, Monthly" (USA column): the maintained series to compare against.

## Manifest
```toml
description = "Frazzini–Pedersen betting against beta"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 3000 }
params = { vol_days = 252, vol_min = 120, corr_days = 1260, corr_min = 750, shrink = 0.6, market = "VTI", weighting = "rank" }
```

## Logic
- Universe: FP use every CRSP common stock (share codes 10 and 11), microcaps included, so take the widest stock-only cross-section the platform serves. Market = CRSP value-weighted index; VTI tracks the CRSP US Total Market index.
- Beta, from total-return (`adjust = "all"`) closes:
  - σ_i and σ_m: standard deviation of 1-day log returns over the last 252 days, with at least 120 valid days.
  - ρ: correlation of overlapping 3-day log returns, r³_t = Σ_{k=0..2} ln(1 + r_{t+k}), over the last 5 years (1260 days), with at least 750 valid days.
  - β_TS = ρ σ_i / σ_m, then shrink: β = 0.6 β_TS + 0.4 × 1. Stocks without enough history are left out.
- At the last session of each month, rank all betas ascending: z_i = rank(β_i), z̄ = mean rank, k = 2 / Σ|z_i − z̄|.
  - High-beta weights w_H = k (z − z̄)⁺; low-beta weights w_L = k (z − z̄)⁻. Each leg's weights sum to 1, and the split is at the median.
  - Leg betas: β_L = w_L·β and β_H = w_H·β, using the shrunk betas.
- Targets as fractions of equity: long w_L,i / β_L in each low-beta stock, short w_H,i / β_H in each high-beta stock. Typical gross is about 1.4–1.6× long and 0.6–0.8× short. The net long cash is borrowed.
- Hold shares until the next month-end rebalance. Reasons: "beta {b}, low leg, rank weight {w}" and "beta {b}, high leg, rank weight {w}".
- Variant `weighting = "value"` (NMV §2.1): the same median split and beta-1 scaling, but value-weighted legs (market cap = `SharesOutstanding` × close). NMV find that FP's rank weighting is effectively equal weighting and drives most of BAB's paper performance.
- Pitfalls:
  - Log returns and overlapping 3-day sums must come from adjusted closes. A missing day breaks a 3-day window; do not fill it.
  - The 750-day correlation minimum excludes about three years of each IPO, by design.
  - NMV: the FP beta is a hybrid of a 5-year beta and a 1y/5y volatility ratio, so it is biased in ways market volatility predicts. The market's FP beta averages 1.05, and BAB is mis-hedged when recent volatility is high. Report realised beta by sub-period.

## Gaps
- G1: month-end formation.
- G2: stocks only; ETFs would distort both the ranks and the median split.
- G4: the low-beta leg is levered above 1× with financing.
- G10: high-beta shorts.
- G11: FP's universe is all CRSP common stocks; current listings bias the cross-section.

## Done when
- Ex-ante beta is 0 at every formation, and realised beta from a daily regression on VTI over the run is within ±0.1 (FP Table III reports −0.06).
- Monthly returns are compared with AQR's USA BAB series over the same months, reporting correlation, mean, volatility and Sharpe. FP's full-sample benchmarks are 0.70%/month, 10.7% volatility and Sharpe 0.78 (1926–2012).
- The `value` variant is run on the same window. The PR reports whether its Sharpe falls to less than half the rank-weighted one, as NMV find (0.49 versus 1.08 over 1968–2017, with FF5 alpha 24 bp/month and not significant).
- Results are reported net of the cost model, against NMV's net 48 bp/month and Sharpe 0.47.
