# low_volatility

Medium · Gaps: G1, G2, G11

## Idea
The low-volatility anomaly: the least volatile large-cap stocks have earned market-like returns with much lower risk, so higher risk-adjusted returns. Each month, hold the lowest-volatility decile of the 1000 largest stocks.

## Sources
- Blitz & van Vliet (2007), "The Volatility Effect: Lower Risk Without Lower Return", *JPM*.
- Baker, Bradley & Wurgler (2011), "Benchmarks as Limits to Arbitrage", *FAJ*.
- Ang, Hodrick, Xing & Zhang (2006), on idiosyncratic volatility.
- Verify: the vol window (Blitz uses 3 years of weekly returns), the weighting (equal versus inverse vol) and sector concentration.

## Manifest
```toml
description = "Lowest-volatility decile of large caps"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 1500 }
params = { vol_weeks = 156, decile = 0.1, weighting = "equal" }
```

## Logic
- At the last session of each month, rank the dollar-volume universe by market cap (`SharesOutstanding` × close, split-adjusted) and keep the largest 1000.
- Compute 3-year weekly return volatility from total-return closes.
- Hold the lowest decile, equal weight. Reason: "vol {x}% in lowest decile".

## Gaps
- G1: month-end formation.
- G2: ETFs (SHV, BIL, short-bond funds) must be excluded or they dominate.
- G11: the paper uses index constituents. Dollar-volume ranking is the stated approximation.

## Done when
- Portfolio volatility is roughly 25–35% below SPY's over 10y, beta is around 0.6–0.7, and the PR compares the result with SPLV/USMV.
