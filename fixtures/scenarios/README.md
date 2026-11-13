# Scenario expectations

Hand-computed against `fixtures/market/`. Every rule used here is code in `crates/core`; decimal strings compare by value (`"0"` equals `"0.00"`), floats within `tolerance`.

## red_green over 10 daily sessions

`red_green.expected.json`. Universe AAA, BBB; 100,000.00 start cash; `weight = 0.25`; commission 0.005/share (min 1.00); default cost model: 5 bps floor, invariance half-spread, square-root impact with coefficient 1; ADV cap 5% (never binds). AAA splits 2:1 ex 2024-11-26, BBB pays 0.50 ex 2024-11-25.

Rules applied (`core::order::target_weight_qty`, `CostModel`, `Liquidity`, `money`):

- At each session close the bot sees that day's bar. Red (`close < open`) with no position: target 0.25. Green (`close > open`) with a position: target 0. Otherwise nothing.
- Target qty = `trunc(weight × equity / close)`; order = target − held.
- The order fills at the next session open: `round4(open × (1 ± slip))`, half-even, with `slip = max(0.0005, half-spread) + σ × √(qty / ADV)`. Cash moves by `round2(qty × price)` plus commission `round2(max(1.00, 0.005 × qty))`.
- `ADV`, `DV` (mean close × volume) and `σ` (sample stdev of close-to-close returns, rounded to 8 decimals) come from the symbol's daily bars before the fill session (all of them here, fewer than 20), split-adjusted at that session's open; `σ` needs two returns. Half-spread = `0.000821 × round8(∛((σ / 0.02)² × 40,000,000 / DV))`.
- Ex-date actions apply at the session open before fills; equity is marked at each close.

| Fill | Bars | ADV | DV | σ | Half-spread | Impact | slip |
|---|---|---|---|---|---|---|---|
| 11-21 buy 510 BBB | 11-20 | 1,200,000 | 58,800,000 | — | — | — | 0.0005 |
| 11-22 buy 125 AAA | 11-20..21 | 2,250,000 | 450,900,000 | — | — | — | 0.0005 |
| 11-27 sell 510 BBB | 11-20..26 | 1,160,000 | 56,407,000 | 0.00938936 | 4.42 bps | 1.9688 bps | 0.00069688 |
| 11-29 sell 250 AAA | 11-20..27, × 0.5 before 11-26 | 4,266,667 | 421,741,666.67 | 0.01414253 | 2.97 bps | 1.0826 bps | 0.00060826 |
| 12-03 buy 251 AAA | 11-20..12-02 | 3,862,500 | 382,325,000 | 0.01273931 | 2.86 bps | 1.0269 bps | 0.00060269 |
| 12-04 sell 251 AAA | 11-20..12-03 | 3,855,556 | 382,488,888.89 | 0.01326025 | 2.94 bps | 1.0699 bps | 0.00060699 |
| 12-04 buy 511 BBB | 11-20..12-03 | 1,053,333 | 51,369,000 | 0.00839843 | 4.24 bps | 1.8498 bps | 0.00068498 |

Both fixture stocks are liquid enough that their half-spread stays under the 5 bps floor, so each fill pays the floor plus its impact.

| Session close | AAA O/C | BBB O/C | Fills at this open | Cash | Holdings at close | Equity |
|---|---|---|---|---|---|---|
| 11-20 | 200.00/202.00 g | 50.00/49.00 r | deposit 100,000.00 | 100,000.00 | — | 100,000.00 |
| 11-21 | 203.00/199.00 r | 49.20/48.80 r | buy 510 BBB @ 49.2246 = 25,104.55 + 2.55 | 74,892.90 | BBB 510 × 48.80 | 99,780.90 |
| 11-22 | 198.00/197.00 r | 48.90/48.70 r | buy 125 AAA @ 198.0990 = 24,762.38 + 1.00 | 50,129.52 | AAA 125 × 197.00, BBB 510 × 48.70 | 99,591.52 |
| 11-25 | 197.50/196.00 r | 48.30/48.10 r | dividend 510 × 0.50 = +255.00 | 50,384.52 | AAA 125 × 196.00, BBB 510 × 48.10 | 99,415.52 |
| 11-26 | 98.50/97.00 r | 48.20/48.60 g | split AAA 125 → 250 | 50,384.52 | AAA 250 × 97.00, BBB 510 × 48.60 | 99,420.52 |
| 11-27 | 97.50/99.00 g | 48.50/48.50 doji | sell 510 BBB @ 48.4662 = 24,717.76 − 2.55 | 75,099.73 | AAA 250 × 99.00 | 99,849.73 |
| 11-29 (13:00) | 99.50/100.00 g | 48.60/48.90 g | sell 250 AAA @ 99.4395 = 24,859.88 − 1.25 | 99,958.36 | — | 99,958.36 |
| 12-02 | 100.50/99.50 r | 49.00/49.40 g | — | 99,958.36 | — | 99,958.36 |
| 12-03 | 99.00/101.00 g | 49.50/49.10 r | buy 251 AAA @ 99.0597 = 24,863.98 + 1.26 | 75,093.12 | AAA 251 × 101.00 | 100,444.12 |
| 12-04 | 101.50/102.00 g | 49.00/48.80 r | sell 251 AAA @ 101.4384 = 25,461.04 − 1.26; buy 511 BBB @ 49.0336 = 25,056.17 + 2.56 | 75,494.17 | BBB 511 × 48.80 | 100,430.97 |

Sizing at the deciding close: BBB 11-20 `trunc(25,000.00 / 49.00) = 510`; AAA 11-21 `trunc(24,945.2250 / 199.00) = 125`; AAA 12-02 `trunc(24,989.5900 / 99.50) = 251`; BBB 12-03 `trunc(25,111.0300 / 49.10) = 511`. Half-even cases: 24,762.375 → 24,762.38; 24,859.875 → 24,859.88; 1.255 → 1.26; 2.555 → 2.56.

## Metric definitions

Equity points `E_1..E_n` are the session closes (n = 10) and `E_0` is the start cash, so the first session counts: returns `r_t = E_t / E_{t-1} − 1` for t = 1..n (10 returns). Benchmark returns `b_t` come from the benchmark equity curve (SPY closes scaled to start cash at the close before the first session, rounded to cents), so they start at t = 1. The fixture has no SPY bar before 11-20, so the curve is scaled at the first close instead and `b_t` start at t = 2, pairing with `r_2..r_n`. `rf = percent / 100 / 252` from `rates.csv` (4.50%).

| Metric | Definition | Value |
|---|---|---|
| total_return | `E_n / start_cash − 1` | 0.0043097 |
| cagr | `(E_n / start_cash)^(252 / n) − 1`, absent when n < 252 | absent |
| volatility | `stdev(r, ddof=1) × √252` | 0.038562… |
| sharpe | `mean(r − rf) / stdev(r − rf, ddof=1) × √252` | 1.661262… |
| sortino | `mean(r − rf) / √(Σ min(r − rf, 0)² / 10) × √252` | 3.424271… |
| max_drawdown | `max (peak − E) / peak` over `E_0..E_n` (100,000.00 → 99,415.52) | 0.0058448 |
| calmar | `cagr / max_drawdown`, absent with cagr | absent |
| trades | closed round trips (flat → flat per symbol; a fill that crosses zero closes the trip with its pro-rata share and opens the next, a split to zero shares closes it) | 3 |
| win_rate | winning round trips / trades; pnl = Σ fill cash deltas + dividends: BBB −136.89, AAA 95.25, AAA 594.54 | 0.6667 |
| profit_factor | Σ wins / Σ |losses| = 689.79 / 136.89 | 5.039009… |
| exposure | mean of `gross_exposure / equity` over equity points | 0.248104… |
| turnover | Σ |round2(qty × price)| / mean equity = 174,825.76 / 99,885.00 | 1.750270… |
| beta | `cov(r_2..r_n − rf, b − rf) / var(b − rf)`, ddof=1 (a constant rf leaves it equal to `cov(r, b) / var(b)`) | −0.616097… |
| alpha | `(mean(r_2..r_n − rf) − beta × mean(b − rf)) × 252` = `(mean(r_2..r_n) − beta × mean(b)) × 252 − (1 − beta) × 0.045` | 0.559801… |

## Event study

`event_study.expected.json`. The fixture with BBB's bars ending at 2024-11-27 and BBB inactive, so its listing ends there. Benchmark SPY for every event; default costs; the daily watermark is the 2024-12-06 close, but no symbol has bars on 12-05 and 12-06, so the benchmark's missing bar leaves those sessions unmeasured.

Rules applied (`core::visibility::open_auction_session`, `core::study::round_trip_cost`):

- Entry is the open of the first session opening after `at`; exit for a horizon of h sessions is the close h − 1 sessions later, on `all`-adjusted prices: AAA's prices before its 2:1 split (ex 11-26) halve, BBB's before its 0.50 dividend (ex 11-25) are multiplied by `1 − 0.50 / 48.70`. After BBB's last session the last close (11-27, 48.50) stands.
- Round trip cost = `2 × (slip + round2_half_even(max(1.00, 0.005 × q)) / (q × open))` with `q = trunc(10,000 / raw open)` and `slip` the engine's for a fill of `q` at the entry session (red_green's rules: the liquidity of the bars before entry, both legs at it). Entries with fewer than three bars before them have no volatility, so `slip = 0.0005`; AAA 12-03 (bars 11-20..12-02, σ 0.01273931, ADV 3,862,500) adds `σ × √(101 / 3,862,500)` = 0.0000651437 and BBB 11-26 (bars 11-20..11-25, σ 0.00543911, ADV 1,162,500) adds `σ × √(207 / 1,162,500)` = 0.0000725799, both half-spreads under the floor.
- Abnormal return = stock return − SPY return; net subtracts the round trip.

| Event `at` | Entry | Raw open | q, commission | Round trip | 1 session stock / SPY | 5 sessions stock / SPY |
|---|---|---|---|---|---|---|
| AAA 11-21 15:00Z | 11-22 | 198.00 | 50, 1.00 | 0.00120202 | 197/198 − 1 / 595.50/593.00 − 1 | 100.00/99.00 − 1 / 602.50/593.00 − 1 |
| BBB 11-21 09:00 ET | 11-21 | 49.20 | 203, 1.02 | 0.00120425 | 48.80/49.20 − 1 / 593.10/590.50 − 1 | 48.50/(49.20 × 0.989733) − 1 / 598.80/590.50 − 1 |
| AAA 12-02 14:30Z (the open itself) | 12-03 | 99.00 | 101, 1.00 | 0.00133031 | 101.00/99.00 − 1 / 603.90/603.40 − 1 | not measured |
| ZZZ 11-22 15:00Z | 11-25 | — | — | — | skipped `not_traded` | |
| BBB 11-25 15:00Z | 11-26 | 48.20 | 207, 1.04 | 0.00135363 | 48.60/48.20 − 1 / 600.40/597.90 − 1 | 48.50/48.20 − 1 (delisted, 11-27 close) / 603.90/597.90 − 1 |

Per horizon: mean, median and hit rate (share above 0) over events; the t-statistic is over calendar-month means of entry (November: events 1, 2, 5; December: event 3) with `ceil((h − 1) / 21)` Newey-West lags, so for 1 session with two months `t = ȳ / √(γ₀ / 2)`, `γ₀ = Σ (y − ȳ)² / 2`; for 5 sessions there is one month and no t-statistic. The curve is the mean abnormal return after each session 1 to 9 over the events measured that far. A second study of the same events without a benchmark gives every event IWM (the fixture has no S&P 500 membership) and, with no IWM bars, skips them as `benchmark_not_traded`; it is the second distinct study, so the luck line is `√(2 ln 2)`.

## Lookahead boundaries

`lookahead.expected.json` lists what a probe bot must and must not see: daily step clocks, raw/split/all-adjusted history around the split and dividend (the dividend factor is `1 − 0.50 / 48.70`), the BBB 10-Q filed 2024-11-27 becoming visible at the 2024-11-29 open, fills never on the decision bar, minute step counts per `wake`, the day order expiring at the 13:00 close which session an order decided at a session's last minute belongs to, the calendar a bot may see ahead (the 2024-11-29 early close, nothing past 366 days), past universes refused until the session before their date has closed and unchanged by later bars, and macro values (`macro.csv`: a CPI revision published 2024-11-26, a 10-year yield published 2024-12-02, weekly jobless claims released on Thursdays and on Wednesday 2024-11-27 before Thanksgiving, the Treasury rate of each date) only from the open after they were published.
