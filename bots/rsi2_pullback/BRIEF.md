# rsi2_pullback

Simple · Gaps: G6

## Idea
Larry Connors' short-term pullback rule: in an uptrend (close above the 200-day SMA), buy when 2-period RSI is deeply oversold and sell when the price closes back above its 5-day SMA. It trades index ETFs, long only, with entries and exits at the close.

## Sources
- Connors & Alvarez (2009), *Short Term Trading Strategies That Work*, ch. 9 ("The 2-Period RSI").
- Connors & Alvarez (2008), *High Probability ETF Trading*.
- Verify: independent replications (QuantifiedStrategies, Alvarez Quant Trading) and post-2010 decay.

## Manifest
```toml
description = "Connors RSI(2) pullback above the 200-day SMA"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "QQQ", "IWM", "DIA"] }
params = { rsi_period = 2, entry_rsi = 5, trend_sma = 200, exit_sma = 5, max_positions = 4 }
```

## Logic
- RSI uses Wilder smoothing on total-return closes.
- Entry: close > SMA200 and RSI(2) < `entry_rsi` → buy at the close, equal weight 1/`max_positions`. Reason: "RSI2 {value} below {entry_rsi} in uptrend".
- Exit: close > SMA5 → sell at the close. Reason: "close above SMA5".
- No stop loss, as in the book.
- The book's headline rule is RSI(2) < 5. The agent reports the thresholds 2–10 the book also tabulates, without picking one by in-sample fit.

## Gaps
- G6: the book computes the signal on the close and trades that same close, which needs a pre-close decision and a market-on-close fill.

## Done when
- The trade count, win rate (the book reports above 80% on SPY) and average hold (2–5 days) are in line with the book over the overlapping years.
