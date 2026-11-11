# noise_boundary_spy

Complex · Gaps: G4, G6, G18 (caveat)

## Idea
Zarattini, Aziz & Barbon's intraday momentum for SPY:
- Build a "noise area" around the open from the average absolute move from the open at each time of day over the previous 14 sessions, widened by any overnight gap.
- At each half-hour mark, follow a break out of the area: long above, short below, flat inside.
- Exit when price crosses back through the current band or the session VWAP (checked only at the half-hour marks), size the day by SPY's 14-day volatility up to 4× leverage, and be flat at the close.

The paper reports, May 2007 to April 2024 net of costs: 1,985% total return, 19.6% a year, 14.3% volatility, Sharpe 1.33, max drawdown 25%, daily hit ratio 43%, 7,668 trades (1.8 a day).

## Sources
- Zarattini, Aziz & Barbon (2024, revised 3 Feb 2025), "Beat the Market: An Effective Intraday Momentum Strategy for S&P500 ETF (SPY)", SFI Research Paper 24-97, SSRN 4824172 (open PDF on alexandria.unisg.ch). Section 3 defines the rules, Tables 3–4 and A1 the results, FAQ Q4/Q24 the monthly returns, Q22 the update to December 2024.
- The authors' Python backtest (concretumgroup.com, linked in FAQ Q1): VWAP on HLC/3, entry only when price is beyond both band and VWAP, $0.35 minimum commission per order.
- Gao, Han, Li & Zhou (2018), "Market Intraday Momentum", *JFE*, for background.

## Manifest
```toml
description = "Intraday noise-area momentum on SPY"
timeframe = "1m"
wake = "every:10m"
universe = { symbols = ["SPY"] }
params = { lookback = 14, vol_multiplier = 1.0, target_daily_vol = 0.02, max_leverage = 4.0, vol_window = 14 }
```
Run with the paper's costs: `commission_per_share = 0.0035`, `commission_min = 0.35`, slippage $0.001 a share (≈ 0.025 bps at SPY's 2016–2024 prices until G18 lands).

## Logic
- Times are clock times of the wake. The price "at HH:MM" is the close of the 1m bar starting at HH:MM − 1, visible at HH:MM. A market order then fills at the open of the HH:MM bar, which is exactly the paper's execution price (its slippage is measured against the open of minute HH:MM).
- Noise area for day t and mark HH:MM:
  - move(t−i, HH:MM) = |close(t−i, HH:MM) / open(t−i, 9:30) − 1| for i = 1..14;
  - σ(t, HH:MM) = mean of those 14 moves;
  - UB = max(open(t, 9:30), close(t−1, 16:00)) × (1 + `vol_multiplier` × σ); LB = min(open(t, 9:30), close(t−1, 16:00)) × (1 − `vol_multiplier` × σ).
- Session VWAP = cumulative Σ(volume × (high + low + close)/3) / Σ volume over regular-session bars from 9:30.
- Shares for the day, fixed at the first mark: round(equity at the previous close × min(`max_leverage`, `target_daily_vol` / σ_SPY) / open(t, 9:30)). σ_SPY is the sample standard deviation (n − 1) of SPY's last 14 daily close-to-close returns, ending at t − 1.
- Decision marks are 10:00, 10:30, …, 15:30 (12 marks, 10:00–12:30 on half days). At each mark, with p the price at the mark:
  - p > max(UB, VWAP) → long the day's shares. Reason: "above noise area by {x}%, VWAP {v}".
  - p < min(LB, VWAP) → short the day's shares. Reason: "below noise area by {x}%, VWAP {v}".
  - Otherwise flat. A long or short held into a mark where p is back inside max(UB, VWAP) or min(LB, VWAP) is closed. Reason: "trailing stop at {band|VWAP} {level}".
  - A crossover reverses the position in one order. Re-entry later the same day is allowed.
- At close − 10 min (15:50, 12:50 on half days), submit a market-on-close order that flattens whatever is held. Reason: "flat at the close".
- Pitfalls:
  - Stops are evaluated only at the marks (paper, Section 3: "Stop losses can also be triggered only on semi-hourly intervals"). Do not add intrabar stops.
  - Alpaca minute bars include extended hours. Open(9:30), VWAP and the moves use regular-session bars only, and close(t−1, 16:00) is the regular-session close.
  - The paper does not say whether close(t−1) is dividend-adjusted. SPY's quarterly ex-dividend open fakes a 0.3–0.5% gap. State which choice the bot makes and show the ex-date days.
  - Half days have no marks after 12:30. For later marks, σ averages the last 14 sessions that had that minute, as the authors' rolling-by-minute code does.
  - Size by `qty`, not `target_weight`: the share count is fixed for the day, and `target_weight` resizes against the latest close on every re-entry.
  - The 14-day σ needs 14 × 390 minute bars of warm-up; the first valid day is the 15th session of data.
  - Positions never cross the close, so no overnight financing or borrow accrues. Check that the ledger charges none on intraday shorts.

## Gaps
- G4: up to 4× gross intraday, whenever σ_SPY falls below 0.5% a day. No overnight financing.
- G6: the paper exits at the official close (16:00). The bot submits the market-on-close order at the 15:50 wake.
- G18 (caveat): the cost model has no per-share slippage ($0.001) and no monthly-volume commission tier (IBKR's $0.0020 a share above 300k shares a month, which the paper applies on about 20% of days). The bps approximation is within 0.02 bps a side, and flat $0.0035 is slightly conservative.
- G9 is not needed: stops are checked only at the half-hour marks.

## Done when
- Sample days match the paper's figures:
  - 2022-01-31 (long) and 2022-04-29 (short) enter at 10:30, not at the first touch;
  - on 2022-01-20 the long is stopped by VWAP at 13:00, near break-even (Figure 5b);
  - the best trade, +9.1%, is on 2018-10-10 (Table 4).
- Calendar-year returns for 2016–2024 are each within ±5 pp of FAQ Q24 (−12.8, −6.9, 61.1, 6.9, 26.8, 34.8, 24.4, 37.2, 32.2), and the 2016–2024 CAGR is within ±3 pp of the table's 20.6%.
- Over 2016–2024, the PR reports Sharpe, volatility (paper 14.3%), max drawdown (paper 25%), daily hit ratio (paper 43–44%), trades a day (paper 1.8), skew (paper +1.22) and worst day (paper −4.5%), each against the paper's full-sample figure, plus SPY buy-and-hold over the same years.
