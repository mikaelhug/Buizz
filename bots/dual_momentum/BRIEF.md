# dual_momentum

Medium · Gaps: G1

## Idea
Antonacci's Global Equities Momentum (GEM) combines two kinds of momentum:
- **Absolute momentum:** if US stocks' 12-month return beats T-bills, stay in equities, otherwise move to aggregate bonds.
- **Relative momentum:** when in equities, hold whichever of US or international stocks has the higher 12-month return.

It rebalances monthly.

## Sources
- Antonacci (2014), *Dual Momentum Investing*.
- Antonacci (2013), "Risk Premia Harvesting Through Dual Momentum", SSRN.
- Verify: ETF-era replications (e.g. Portfolio Visualizer, Allocate Smartly), the lookback sensitivity and whipsaw cost.

## Manifest
```toml
description = "Antonacci Global Equities Momentum"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "EFA", "BIL", "AGG"] }
params = { us = "SPY", intl = "EFA", cash = "BIL", bonds = "AGG", lookback_months = 12 }
```

## Logic
- At the last session of each month, compute 12-month total returns.
- If r(SPY) > r(BIL): hold the larger of r(SPY) and r(EFA) at weight 1. Reason: "{winner} leads, equities beat T-bills".
- Otherwise: hold AGG at weight 1. Reason: "equities below T-bills, to bonds".
- The book uses ACWI ex-US. The agent checks whether EFA, VEU or ACWX is the faithful proxy for the run's years.

## Gaps
- G1: month-end signal date.

## Done when
- The switch dates match a reference GEM replication over the same years.
- The PR reports the number of switches and the drawdown versus SPY.
