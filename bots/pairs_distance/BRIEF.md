# pairs_distance

Medium · Gaps: G2, G10, G3 (industry variant)

## Idea
The Gatev–Goetzmann–Rouwenhorst distance method:
- **Formation:** over 12 months, match every stock with the partner whose normalised price path is closest (minimum sum of squared deviations), and take the 20 closest pairs.
- **Trading:** over the next 6 months, open a dollar-neutral position when the spread diverges by 2 historical σ, and close it when the prices cross.

Six overlapping portfolios run at once, one started each month.

## Sources
- Gatev, Goetzmann & Rouwenhorst (2006), "Pairs Trading: Performance of a Relative-Value Arbitrage Rule", *RFS*.
- Do & Faff (2010), "Does Simple Pairs Trading Still Work?", *FAJ* (the decay).
- Verify: Krauss (2017), "Statistical Arbitrage Pairs Trading Strategies: Review and Outlook".

## Manifest
```toml
description = "Gatev–Goetzmann–Rouwenhorst distance pairs"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 500 }
params = { formation_sessions = 252, trading_sessions = 126, top_pairs = 20, open_sigma = 2.0, portfolios = 6 }
```

## Logic
- Formation: total-return indices normalised to 1 at the start. Stocks must trade every day of the formation period.
- Trade: when |spread| > 2σ_formation, short the higher leg and long the lower, equal dollars. Reason: "pair {a}/{b} spread {z}σ".
- Close: when the normalised prices cross, or at the end of the trading period. Reasons: "pair converged" or "trading period ended".
- Each of the six overlapping portfolios holds 1/6 of capital.
- Pairs are rebuilt as the universe shifts, and formation for a portfolio uses only data before its start.

## Gaps
- G2: ETF lookalikes (SPY/IVV/VOO) would take every slot.
- G10: shorts need per-name borrow.
- G3: GGR's industry-restricted variant needs industry codes. The unrestricted base case does not.

## Done when
- The PR reports the trade count, the fraction converging, average days open and the excess return, compared with GGR and Do & Faff for overlapping years.
