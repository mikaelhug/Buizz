# Scenario expectations

Hand-computed against `fixtures/market/`. Every rule used here is code in `crates/core`; decimal strings compare by value (`"0"` equals `"0.00"`), floats within `tolerance`.

## red_green over 10 daily sessions

`red_green.expected.json`. Universe AAA, BBB; 100,000.00 start cash; `weight = 0.25`; commission 0.005/share (min 1.00); slippage 5 bps; ADV cap 5% (never binds). AAA splits 2:1 ex 2024-11-26, BBB pays 0.50 ex 2024-11-25.

Rules applied (`core::order::target_weight_qty`, `CostModel`, `money`):

- At each session close the bot sees that day's bar. Red (`close < open`) with no position: target 0.25. Green (`close > open`): target 0. Doji: nothing.
- Target qty = `trunc(weight × equity / close)`; order = target − held. A zero delta is recorded as `rejected / zero_quantity`.
- The order fills at the next session open: `round4(open × (1 ± 0.0005))`, half-even. Cash moves by `round2(qty × price)` plus commission `round2(max(1.00, 0.005 × qty))`.
- Ex-date actions apply at the session open before fills; equity is marked at each close.

| Session close | AAA O/C | BBB O/C | Fills at this open | Cash | Holdings at close | Equity |
|---|---|---|---|---|---|---|
| 11-20 | 200.00/202.00 g | 50.00/49.00 r | deposit 100,000.00 | 100,000.00 | — | 100,000.00 |
| 11-21 | 203.00/199.00 r | 49.20/48.80 r | buy 510 BBB @ 49.2246 = 25,104.55 + 2.55 | 74,892.90 | BBB 510 × 48.80 | 99,780.90 |
| 11-22 | 198.00/197.00 r | 48.90/48.70 r | buy 125 AAA @ 198.0990 = 24,762.38 + 1.00 | 50,129.52 | AAA 125 × 197.00, BBB 510 × 48.70 | 99,591.52 |
| 11-25 | 197.50/196.00 r | 48.30/48.10 r | dividend 510 × 0.50 = +255.00 | 50,384.52 | AAA 125 × 196.00, BBB 510 × 48.10 | 99,415.52 |
| 11-26 | 98.50/97.00 r | 48.20/48.60 g | split AAA 125 → 250 | 50,384.52 | AAA 250 × 97.00, BBB 510 × 48.60 | 99,420.52 |
| 11-27 | 97.50/99.00 g | 48.50/48.50 doji | sell 510 BBB @ 48.4758 = 24,722.66 − 2.55 | 75,104.63 | AAA 250 × 99.00 | 99,854.63 |
| 11-29 (13:00) | 99.50/100.00 g | 48.60/48.90 g | sell 250 AAA @ 99.4502 = 24,862.55 − 1.25 | 99,965.93 | — | 99,965.93 |
| 12-02 | 100.50/99.50 r | 49.00/49.40 g | — | 99,965.93 | — | 99,965.93 |
| 12-03 | 99.00/101.00 g | 49.50/49.10 r | buy 251 AAA @ 99.0495 = 24,861.42 + 1.26 | 75,103.25 | AAA 251 × 101.00 | 100,454.25 |
| 12-04 | 101.50/102.00 g | 49.00/48.80 r | sell 251 AAA @ 101.4492 = 25,463.75 − 1.26; buy 511 BBB @ 49.0245 = 25,051.52 + 2.56 | 75,511.66 | BBB 511 × 48.80 | 100,448.46 |

Sizing at the deciding close: BBB 11-20 `trunc(25,000.00 / 49.00) = 510`; AAA 11-21 `trunc(24,945.2250 / 199.00) = 125`; AAA 12-02 `trunc(24,991.4825 / 99.50) = 251`; BBB 12-03 `trunc(25,113.5625 / 49.10) = 511`. Half-even cases: 24,762.375 → 24,762.38; 1.255 → 1.26; 2.555 → 2.56; 48.47575 → 48.4758; 99.45025 → 99.4502; 101.44925 → 101.4492.

Orders 1, 6, 7, 9 and 12 are zero-quantity rejections on green days without a position.

## Metric definitions

Equity points `E_1..E_n` are the session closes (n = 10) and `E_0` is the start cash, so the first session counts: returns `r_t = E_t / E_{t-1} − 1` for t = 1..n (10 returns). Benchmark returns `b_t` come from the benchmark equity curve (SPY closes scaled to start cash at the first close, rounded to cents), so they start at t = 2 and pair with `r_2..r_n`. `rf = percent / 100 / 252` from `rates.csv` (4.50%).

| Metric | Definition | Value |
|---|---|---|
| total_return | `E_n / start_cash − 1` | 0.0044846 |
| cagr | `(E_n / start_cash)^(252 / n) − 1`, absent when n < 252 | absent |
| volatility | `stdev(r, ddof=1) × √252` | 0.038766… |
| sharpe | `mean(r − rf) / stdev(r − rf, ddof=1) × √252` | 1.765925… |
| sortino | `mean(r − rf) / √(Σ min(r − rf, 0)² / 10) × √252` | 3.664590… |
| max_drawdown | `max (peak − E) / peak` over `E_0..E_n` (100,000.00 → 99,415.52) | 0.0058448 |
| calmar | `cagr / max_drawdown`, absent with cagr | absent |
| trades | closed round trips (flat → flat per symbol; a fill that crosses zero closes the trip with its pro-rata share and opens the next, a split to zero shares closes it) | 3 |
| win_rate | winning round trips / trades; pnl = Σ fill cash deltas + dividends: BBB −131.99, AAA 97.92, AAA 599.81 | 0.6667 |
| profit_factor | Σ wins / Σ |losses| = 697.73 / 131.99 | 5.286233… |
| exposure | mean of `gross_exposure / equity` over equity points | 0.248096… |
| turnover | Σ |round2(qty × price)| / mean equity = 174,828.83 / 99,889.766 | 1.750217… |
| beta | `cov(r_2..r_n, b) / var(b)`, ddof=1 | −0.617072… |
| alpha | `(mean(r_2..r_n) − beta × mean(b)) × 252` | 0.638220… |

## Lookahead boundaries

`lookahead.expected.json` lists what a probe bot must and must not see: daily step clocks, raw/split/all-adjusted history around the split and dividend (the dividend factor is `1 − 0.50 / 48.70`), the BBB 10-Q filed 2024-11-27 becoming visible at the 2024-11-29 open, fills never on the decision bar, minute step counts per `wake`, the day order expiring at the 13:00 close and which session an order decided at a session's last minute belongs to.
