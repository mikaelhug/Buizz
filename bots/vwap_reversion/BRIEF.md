# vwap_reversion

Medium · Gaps: G9

## Idea
In very liquid large caps, large intraday deviations from session VWAP tend to revert as liquidity providers lean against them. Fade moves beyond k standard deviations of the price-minus-VWAP distribution, exit at VWAP, protect each trade with a stop, and be flat at the close.

## Sources
- Berkowitz, Logue & Noser (1988), "The Total Cost of Transactions on the NYSE", *JF* (VWAP as the benchmark).
- Heston, Korajczyk & Sadka (2010), "Intraday Patterns in the Cross-section of Stock Returns", *JF*.
- This is practitioner-led. The agent must find credible, cost-aware evidence. If there is none, the PR says so and recommends dropping the bot.

## Manifest
```toml
description = "Intraday fade of deviations from VWAP"
timeframe = "1m"
wake = "every:5m"
universe = { symbols = ["AAPL", "MSFT", "NVDA", "AMZN", "GOOGL", "META", "AVGO", "JPM", "XOM", "UNH"] }
params = { k = 2.5, band_window = 30, stop_k = 4.0, no_entry_after = "15:30", max_positions = 5 }
```

## Logic
- Session VWAP from 1m bars, (H+L+C)/3 × volume, cumulative from the open. The deviation band is σ of (price − VWAP) over the last `band_window` minutes.
- Entry: price above VWAP + kσ → short; below VWAP − kσ → long; 1/`max_positions` weight each, and no entries after 15:30. Reason: "{z}σ from VWAP".
- Exit: a limit at VWAP, refreshed each wake. Reason: "back to VWAP". A protective stop at `stop_k`σ. Reason: "stop".
- Flatten everything on the last wake before the close. Reason: "flat at the close".

## Gaps
- G9: the protective stop must fill intrabar. Checking it every five minutes changes the risk profile.

## Done when
- The PR reports the hit rate, average win/loss, net-of-cost return and the evidence search, with a clear keep-or-drop recommendation.
