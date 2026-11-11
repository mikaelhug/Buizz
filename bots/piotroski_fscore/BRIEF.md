# piotroski_fscore

Medium · Gaps: G2, G10

## Idea
Within the cheapest book-to-market quintile, a nine-point accounting score (F-score) separates winners from losers. Buy high-score value stocks (F ≥ 8) and short low-score ones (F ≤ 1), holding each position for one year from when its annual report becomes available.

## Sources
- Piotroski (2000), "Value Investing: The Use of Historical Financial Statement Information to Separate Winners from Losers", *JAR*.
- Verify: Piotroski & So (2012) and post-2000 out-of-sample replications. Large-cap results are weaker than small-cap ones.

## Manifest
```toml
description = "F-score within the cheapest book-to-market quintile"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 2000 }
params = { bm_quintile = 0.2, long_min = 8, short_max = 1, hold_sessions = 252 }
```

## Logic
- When a stock's annual report becomes available, compute book-to-market = `Equity` / market cap, and its quintile breakpoint from the current cross-section.
- The F-score's nine binary signals are:
  - ROA > 0, CFO > 0, ΔROA > 0, CFO > ROA (accruals);
  - Δleverage < 0, Δcurrent ratio > 0, no new equity issued;
  - Δgross margin > 0, Δasset turnover > 0.
- Use the Q4 or FY figures exactly as the paper defines them.
- Long when F ≥ 8 and short when F ≤ 1, equal weight, held 252 sessions or until the next annual report. Reasons: "F={f}, BM quintile 1".

## Gaps
- G2: stocks only.
- G10: the short leg is distressed small caps, where borrow is costly and sometimes unavailable.

## Done when
- The long leg beats the full value quintile, and the PR reports the high-minus-low spread against the paper and later replications.
