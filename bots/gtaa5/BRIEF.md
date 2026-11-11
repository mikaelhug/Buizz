# gtaa5

Simple · Gaps: G1

## Idea
Faber's tactical asset allocation: five equal 20% sleeves (US stocks, foreign stocks, bonds, REITs, commodities). Each sleeve is held while its month-end price is above its 10-month SMA and moves to T-bills otherwise. It roughly matches equity returns with bond-like drawdowns.

## Sources
- Faber (2007, updated 2013), "A Quantitative Approach to Tactical Asset Allocation", *Journal of Wealth Management*.
- Verify: post-publication performance (Faber's updates) and the monthly versus daily signal sensitivity.

## Manifest
```toml
description = "Faber 5-asset 10-month SMA timing"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "EFA", "IEF", "VNQ", "DBC", "BIL"] }
params = { assets = ["SPY", "EFA", "IEF", "VNQ", "DBC"], cash = "BIL", sma_months = 10 }
```

## Logic
- At the last session of each month, sample month-end total-return closes and compute the 10-month SMA for each asset.
- Above the SMA → target weight 0.2. At or below → that 0.2 goes to BIL. Reasons: "{asset} above 10m SMA" or "{asset} below 10m SMA, to cash".
- Orders fill at the next open. Assets without history yet hold cash.

## Gaps
- G1: needs to know the session is the month's last.

## Done when
- Over 10y, the drawdown is markedly smaller than an equal-weight buy-and-hold of the five, and the PR compares CAGR and volatility with Faber's update.
