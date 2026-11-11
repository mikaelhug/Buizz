# trend_following

Medium · Gaps: G1, G4

## Idea
Time-series momentum: for each asset, go long if its past 12-month excess return is positive and short if negative. Each position is scaled so the asset contributes 40% annualised volatility on its own, which gives a diversified portfolio near 10–12% volatility. It rebalances monthly.

## Sources
- Moskowitz, Ooi & Pedersen (2012), "Time Series Momentum", *JFE*.
- Hurst, Ooi & Pedersen (2017), "A Century of Evidence on Trend-Following Investing", *JPM*.
- Verify: the ETF-universe adaptation, and the ex-ante volatility estimator (an EWMA with a 60-day centre of mass on daily returns).

## Manifest
```toml
description = "Time-series momentum, vol-scaled, long/short"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "IWM", "EFA", "EEM", "EWJ", "TLT", "IEF", "SHY", "LQD", "GLD", "SLV", "USO", "DBA", "UUP", "FXE", "BIL"] }
params = { lookback_months = 12, asset_vol = 0.40, vol_com_days = 60, cash = "BIL" }
```

## Logic
- At the last session of each month, compute r12 minus BIL's r12.
- Weight per asset = sign(excess r12) × `asset_vol` / σ_ex-ante, divided by the number of assets as in the paper's equal-risk aggregation. Reason: "12m excess {r}% → {long|short}".
- Fills at the next open. Gross exposure is whatever the volatility scaling implies, often above 1×.

## Gaps
- G1: month-end formation.
- G4: the vol scaling routinely needs gross above 1×.

## Done when
- The portfolio's realised volatility is within a few points of the target, with positive skew and crisis alpha in 2020 and 2022.
- The PR compares the result with the paper's diversified TSMOM.
