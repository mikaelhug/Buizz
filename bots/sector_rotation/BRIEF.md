# sector_rotation

Medium · Gaps: G1

## Idea
Industry momentum: each month, hold the three strongest US sector SPDRs by blended 3/6/12-month total return. A sector with negative absolute momentum (below T-bills) leaves its slot in T-bills.

## Sources
- Moskowitz & Grinblatt (1999), "Do Industries Explain Momentum?", *JF*.
- Faber (2010), "Relative Strength Strategies for Investing", Cambria working paper.
- Verify: sector-ETF replications, and how XLC (2018) and XLRE (2015) entering the set changes it.

## Manifest
```toml
description = "Top-3 sector SPDRs by blended momentum"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["XLB", "XLC", "XLE", "XLF", "XLI", "XLK", "XLP", "XLRE", "XLU", "XLV", "XLY", "BIL"] }
params = { top = 3, lookbacks_months = [3, 6, 12], cash = "BIL" }
```

## Logic
- At the last session of each month, score each sector by the average of its 3-, 6- and 12-month total returns. A sector needs 12 months of history to be scored.
- Take the top 3 at weight 1/3 each. Reason: "rank {k} by blended momentum".
- If a selected sector's 12-month return is below BIL's, its third goes to BIL. Reason: "{sector} below T-bills".

## Gaps
- G1: month-end signal date.

## Done when
- Holdings change monthly with sensible persistence, and the PR compares the result with an equal-weight sector portfolio and with SPY.
