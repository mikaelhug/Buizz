# halloween

Simple · Gaps: G1

## Idea
"Sell in May and go away": hold equities from the last close of October to the last close of April, and T-bills for the other six months. The effect is documented across 37 markets and centuries of UK data.

## Sources
- Bouman & Jacobsen (2002), "The Halloween Indicator, 'Sell in May and Go Away': Another Puzzle", *AER*.
- Jacobsen & Zhang (2018), "The Halloween Indicator: Everywhere and All the Time" (out-of-sample update).
- Verify: whether the effect survives after 2002 in US large caps; the critiques by Maberly & Pierce (2004).

## Manifest
```toml
description = "SPY November–April, T-bills May–October"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "BIL"] }
params = { risk_on = "SPY", risk_off = "BIL", enter_month = 11, exit_month = 5 }
```

## Logic
- At the close of the last October session: switch fully to SPY filling at the next open. Reason: "Halloween entry".
- At the close of the last April session: switch fully to BIL filling at the next open. Reason: "sell in May".
- Before BIL exists (2007-05), hold cash.

## Gaps
- G1: needs to identify the last session of October and April.
- Next-open execution follows the BRIEFS.md rule for holds of a month or more, so G6 does not apply.

## Done when
- The equity curve is flat (or earns the T-bill rate) May–October and tracks SPY November–April.
- The PR compares the winter-minus-summer return spread with Jacobsen & Zhang's US figure for the same years.
