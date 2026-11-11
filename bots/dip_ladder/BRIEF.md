# dip_ladder

Simple · Gaps: none

## Idea
The retail "buy the dip" ladder: rest limit buys at −2%, −4% and −6% below the prior close of SPY and QQQ, take profit at +3% above the average cost, and exit anything left after five sessions. It is a reality check on a popular habit and exercises limit fills on touch.

## Sources
- No academic origin. Use it as a falsification test of retail dip buying.
- Verify: research on overnight and intraday drops (e.g. Moskowitz et al. on short-horizon reversal) and evidence that buying dips loses to buy-and-hold over long periods.

## Manifest
```toml
description = "Resting limit buys at -2/-4/-6%, take profit or time exit"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "QQQ"] }
params = { rungs = [0.02, 0.04, 0.06], rung_weight = 0.16, take_profit = 0.03, max_hold = 5 }
```

## Logic
- After every close, for each symbol, place day-TIF limit buys at prior close × (1 − rung) for each rung not yet filled, each sized `rung_weight` of equity. Reason: "dip rung −{rung}%".
- For open positions, place a day-TIF limit sell at average cost × (1 + `take_profit`). Reason: "take profit".
- A position older than `max_hold` sessions is sold at the next open with a market order. Reason: "time exit".
- Orders are re-placed each step because time-in-force is day.

## Gaps
- None.

## Done when
- Limit fills follow the engine's touch rule, and the PR compares the result with buy-and-hold SPY and QQQ.
- The PR states that fill-on-touch ignores queue position and is optimistic.
