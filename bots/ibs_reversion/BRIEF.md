# ibs_reversion

Simple · Gaps: G6

## Idea
Internal bar strength, IBS = (close − low) / (high − low), measures where a day closed in its range. Index ETFs that close near the low tend to rise the next day. Buy at the close when IBS is low and exit when it is high.

## Sources
- Pagonidis (2014), "The IBS Effect: Mean Reversion in Equity ETFs", NAAIM Wagner Award paper.
- Levy (1967), on intraday position-in-range as a predictor.
- Verify: replications on SPY and QQQ, and how the effect depends on high IBS volatility regimes.

## Manifest
```toml
description = "Internal bar strength mean reversion on index ETFs"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "QQQ"] }
params = { entry_ibs = 0.2, exit_ibs = 0.8 }
```

## Logic
- Entry: IBS < `entry_ibs` → buy at the close, equal weight across symbols. Reason: "IBS {value} near the low".
- Exit: IBS > `exit_ibs` → sell at the close. Reason: "IBS {value} near the high".
- Skip days where high equals low.

## Gaps
- G6: entries and exits are at the same close the signal is computed on.

## Done when
- The next-day return after low-IBS closes is positive and its size is comparable with Pagonidis's table for the overlapping period.
