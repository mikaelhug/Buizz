# turn_of_month

Simple · Gaps: G1, G6

## Idea
US equity returns cluster around the turn of the month: from the last trading day of a month (day −1) through the third trading day of the next (day +3). Hold SPY for those four sessions and T-bills otherwise.

## Sources
- Ariel (1987), "A Monthly Effect in Stock Returns", *JFE*.
- Lakonishok & Smidt (1988), "Are Seasonal Anomalies Real? A Ninety-Year Perspective", *RFS*.
- McConnell & Xu (2008), "Equity Returns at the Turn of the Month", *FAJ* (the −1 to +3 window).
- Verify: persistence after 2008, and the payroll and pension-flow explanation.

## Manifest
```toml
description = "SPY over trading days -1 to +3 of each month"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "BIL"] }
params = { first_day = -1, last_day = 3 }
```

## Logic
- At the close of trading day −2 (the second-last session of the month): buy SPY with market-on-close, so the position earns day −1's return. Reason: "turn of month entry".
- At the close of trading day +3: switch back to BIL with market-on-close. Reason: "turn of month exit".
- Count trading days from the exchange calendar, never calendar days.

## Gaps
- G1: needs to know which session is −2 and which is +3 ahead of time.
- G6: the window is defined close-to-close.

## Done when
- Exactly four sessions in SPY per month across the 10y run, including months with holidays at the turn.
- The PR reports the window's average return against non-window days and against McConnell & Xu.
