# donchian_breakout

Simple · Gaps: G4, G9

## Idea
Turtle Trading System 2, applied to a cross-asset ETF basket:
- Enter on a 55-day breakout and exit on a 20-day counter-breakout.
- Size positions in volatility units (N), pyramid up to four units, and protect each unit with a 2N stop.
- Trade long and short.

## Sources
- Faith (2003), *The Original Turtle Trading Rules*, and Faith (2007), *Way of the Turtle*.
- Donchian (1960), on channel breakouts.
- Verify: Clenow, *Following the Trend* (2013), on modern trend-following with N sizing on ETFs and futures.

## Manifest
```toml
description = "Turtle System 2 with N sizing, pyramiding and 2N stops"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "QQQ", "IWM", "EFA", "EEM", "TLT", "IEF", "GLD", "SLV", "USO", "DBA", "UUP"] }
params = { entry = 55, exit = 20, n_period = 20, risk_per_unit = 0.01, max_units = 4, add_step = 0.5, stop_n = 2.0 }
```

## Logic
- N = 20-day Wilder average of the true range.
- Unit size = `risk_per_unit` × equity / N shares.
- Entry: a stop order at the 55-day high (long) or low (short), filled intrabar. Reason: "55-day breakout".
- Pyramid: add one unit at each +½N from the last entry, up to 4 units per market, 6 per correlated group and 12 per direction.
- Stop: 2N from the latest entry, moved up for all units when one is added. Reason: "2N stop".
- Exit: a 20-day low (long) or high (short), as a stop order. Reason: "20-day exit".

## Gaps
- G9: entries, stops and exits are all stop orders filled intrabar.
- G4: four units at 1% risk on low-N ETFs exceed 1× gross, and the original rules trade futures on margin.

## Done when
- The PR shows unit counts, correlated-group limits respected, and drawdowns typical of trend-following (deep and long).
