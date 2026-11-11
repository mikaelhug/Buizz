# intraday_momentum_spy

Medium · Gaps: G6

## Idea
Market intraday momentum: SPY's return from the previous close to 10:00 predicts its return over the last half hour (15:30–16:00). Take a position in that direction at 15:30 and close it at the close. The prior 15:00–15:30 return is a second predictor.

## Sources
- Gao, Han, Li & Zhou (2018), "Market Intraday Momentum", *JFE* 129(2).
- Baltussen, Da, Lammers & Martens (2021), "Hedging demand and market intraday momentum", *JFE* (the gamma-hedging explanation).
- Verify: the QuantConnect replication across ETFs (IWM, IYR) and performance after 2013.

## Manifest
```toml
description = "First half-hour predicts last half-hour"
timeframe = "1m"
wake = "every:30m"
universe = { symbols = ["SPY"] }
params = { use_r12 = false, size = 1.0 }
```

## Logic
- At 10:00, r1 = 10:00 price / previous close − 1 (the 09:59 bar's close is the 10:00 price). Store it in state.
- At 15:30: if r1 > 0, go long `size`; if r1 < 0, go short. With `use_r12`, require sign agreement with the 15:00–15:30 return. Reason: "r1 {x}% → last half hour {long|short}".
- Exit with a market-on-close order for the whole position. Reason: "flat at the close".
- On half days, shift the windows to the early close as the paper does, or skip the day. The agent checks which.

## Gaps
- G6: the paper's last-half-hour return ends at the official 16:00 close.

## Done when
- The sign hit rate and annualised return are in line with the paper's out-of-sample figures for 2016 onward, and the PR reports net-of-cost results.
