# bollinger_reversion

Simple · Gaps: G2, G6

## Idea
Buy liquid stocks that close below the lower Bollinger Band (20-day SMA − 2σ) while still above their 200-day SMA, and exit when price returns to the middle band. It is short-horizon mean reversion inside a long-term uptrend.

## Sources
- Bollinger (2001), *Bollinger on Bollinger Bands*.
- Lento, Gradojevic & Wright (2007), "Investment information content in Bollinger Bands?", *Applied Financial Economics*.
- Jegadeesh (1990) and Lehmann (1990), for the short-term reversal behind the rule.
- Verify: results on single stocks versus indices, and the effect of the trend filter.

## Manifest
```toml
description = "Lower-band reversion in liquid stocks with a trend filter"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 100 }
params = { band_period = 20, band_sigma = 2.0, trend_sma = 200, max_positions = 10, max_hold = 10 }
```

## Logic
- Entry: close < SMA20 − 2σ and close > SMA200 → buy at the close, equal weight 1/`max_positions`. If more names qualify than slots, rank by the deepest band penetration. Reason: "close {z}σ below the 20-day mean".
- Exit: close ≥ SMA20 → sell at the close. Reason: "back at the middle band". Also exit after `max_hold` sessions. Reason: "time exit".

## Gaps
- G2: the universe must be common stocks only.
- G6: entries and exits trade the signal's close.

## Done when
- The PR reports win rate, average hold and profit factor, and compares the trend-filtered rule with the unfiltered one.
