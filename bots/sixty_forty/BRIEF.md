# sixty_forty

Simple · Gaps: G1

## Idea
Strategic 60% US equities / 40% US aggregate bonds, rebalanced back to target at each month end. This is the yardstick every other bot is judged against: a strategy that can't beat a rebalanced 60/40 after costs, on risk-adjusted terms, isn't worth running.

## Sources
- Ilmanen, *Expected Returns* (2011), ch. 3, on the balanced portfolio.
- iShares AOR (Core Growth Allocation) and Vanguard balanced-fund methodology, as real-world implementations.
- Verify: the rebalancing premium and drift literature (Hallerbach 2014, "Disentangling rebalancing return").

## Manifest
```toml
description = "60/40 SPY/AGG, rebalanced at month end"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "AGG"] }
params = { equity_weight = 0.6, equity = "SPY", bonds = "AGG" }
benchmark = "AOR"
```

## Logic
- At the close of the last session of each month, order `target_weight` 0.6 SPY and 0.4 AGG, filling at the next open. Reason: "month-end rebalance".
- Start fully invested at the first step. Before AGG exists (2003-09), use the earliest available investment-grade bond proxy the agent can justify, or start the run in 2003.

## Gaps
- G1: needs to know the current session is the last one of the month.

## Done when
- Over 10y, CAGR and volatility match a monthly-rebalanced 60/40 SPY/AGG computed from total-return series, within costs.
- Turnover is roughly 10–20% a year, not more.
