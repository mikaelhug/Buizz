# overnight_drift

Simple · Gaps: G6

## Idea
Since the 1990s nearly all of the US equity premium has accrued overnight (close to open), not intraday. Hold SPY from each close to the next open and be flat during the session. The cost of two trades a day is the real test.

## Sources
- Cliff, Cooper & Gulen (2008), "Return Differences between Trading and Non-trading Hours: Like Night and Day".
- Kelly & Clark (2011), "Returns in Trading versus Non-trading Hours: The Difference is Day and Night", *JAF*.
- Lou, Polk & Skouras (2019), "A Tug of War: Overnight versus Intraday Expected Returns", *JFE*.
- Verify: the effect after 2019 and its net-of-cost viability.

## Manifest
```toml
description = "Long SPY close-to-open"
timeframe = "1m"
wake = "every:1m"
universe = { symbols = ["SPY"] }
params = { entry = "close", exit = "open" }
```

## Logic
- Before the close (by 15:50): a market-on-close buy of 100% SPY. Reason: "hold overnight".
- Before the open: a market-on-open sell of the whole position. Reason: "flat for the session".
- Half days follow the calendar's early close.
- The wake only needs the pre-close and pre-open points. If G6 lands with a pre-open wake, the agent narrows the wake to those times.

## Gaps
- G6: the strategy is defined on the official close and open prints. A market order at the 15:59 bar and the 09:30 bar is a different strategy and is not acceptable.

## Done when
- Gross returns over the overnight segments match SPY's overnight return series for the same days.
- The PR shows gross versus net under the default cost model.
