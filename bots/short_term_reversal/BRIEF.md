# short_term_reversal

Medium · Gaps: G1, G2, G10

## Idea
Last week's losers outperform last week's winners next week. Each week, go long the bottom decile and short the top decile of prior-week returns among large caps, dollar neutral. The gross edge is real, and the brief's real question is whether it survives costs.

## Sources
- Lehmann (1990), "Fads, Martingales, and Market Efficiency", *QJE* (weekly).
- Jegadeesh (1990), "Evidence of Predictable Behavior of Security Returns", *JF* (monthly).
- Lo & MacKinlay (1990), "When Are Contrarian Profits Due to Stock Market Overreaction?", *RFS*.
- Verify: de Groot, Huij & Zhou (2012), "Another look at trading costs and short-term reversal profits", *JBF*.

## Manifest
```toml
description = "Weekly market-neutral reversal deciles"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 1000 }
params = { decile = 0.1, gross = 1.0, weighting = "equal" }
```

## Logic
- At the last session of each week, compute each stock's total return over that week.
- Long the bottom decile and short the top decile, equal weight, each leg 50% of gross. Reasons: "week loser rank {k}" and "week winner rank {k}".
- Fills at the next open. Turnover is near 100% of each leg weekly.
- The agent also evaluates Lo & MacKinlay's weights (proportional to minus the market-relative return) and reports both.

## Gaps
- G1: week-end formation, where holidays move the last session.
- G2: stocks only.
- G10: the short leg is often in small, volatile names, where borrow is costly.

## Done when
- The PR shows gross versus net returns and the break-even cost per trade, and compares them with de Groot et al.
