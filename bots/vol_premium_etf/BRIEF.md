# vol_premium_etf

Complex · Gaps: G6, G8

## Idea
The volatility risk premium: the VIX overstates subsequent realised volatility about 80% of the time, and the VIX futures curve is usually in contango, so short-volatility ETPs earn a roll yield. Unconditional short volatility gives that back in crises (XIV's collapse on 5 February 2018). Zarattini, Mele & Aziz time it with two signals measured at 15:45 and executed at the close:
- the expected premium, eVRP = VIX − 10-day realised SPY volatility;
- the term structure: contango if VIX < VIX3M, backwardation if VIX > VIX3M.

Exposure is sized by the VIX level. This is Strategy 4, "eVRP + BoC + Sizing". Over January 2008 to May 2025, net of 5 bps per trade, they report 16.3% CAGR, 16.4% volatility, Sharpe 1.00, max drawdown −31%, and 0.15 correlation with the S&P 500.

## Sources
- Zarattini, Mele & Aziz (2025), "The Volatility Edge: A Dual Approach for VIX ETNs Trading", SFI Research Paper 25-91, SSRN 5316487. Section 3 gives the assumptions, Section 4 and Table 2 the rules, Table 3 the results and Appendix Table 4 the calendar-year returns. The Colab code is linked in Section 6.
- Whaley (2013), "Trading Volatility: At What Cost?", *JPM*, for why VIX ETPs bleed and why short-vol ETPs carry crash risk.
- Simon & Campasano (2014), "The VIX Futures Basis: Evidence and Trading Strategies", *Journal of Derivatives*, for the evidence that the basis predicts VX futures returns. It is not a variant of this bot. Its rule sells or buys the nearest VX future with at least 10 days to expiry when the daily roll is beyond ±0.10, holds five days and hedges with E-mini S&P futures. That needs futures trading, which Buizz does not offer, and belongs in its own bot.

## Manifest
```toml
description = "VIX ETP volatility premium timed by eVRP and the VIX term structure"
timeframe = "1m"
wake = "every:15m"
universe = { symbols = ["SPY", "SVXY", "VIXY"] }
params = { variant = "evrp_boc_sizing", rv_window = 10, vix_divisor = 100, base_size = 0.20, rebalance_band = 0.02 }
```
Run with the paper's costs: 5 bps per trade (`slippage_bps = 5`, `commission_per_share = 0`, `commission_min = 0`).

## Logic
- Instruments map onto the paper's proxies:
  - VIXSHORT (−1× daily the short-term VIX futures index, excess return) → SVXY. SVXY was −1× until 2018-02-27 and −0.5× from 2018-02-28, so from that date hold 2 × the target exposure in SVXY.
  - VIXLONG (the short-term VIX futures index, total return) → VIXY. It has continuous history since 2011 and no creation halt, unlike VXX in March 2022.
  - The rest of the book is uninvested cash, as in the paper (no T-bills, no SPY).
- Act only at the wake where clock = session close − 15 min (15:45, or 12:45 on half days). The paper signals at 15:45 and fills at the close (12:45 and 13:00 on early-close days).
- eRV = sample standard deviation of SPY's last 10 daily returns × √252 × 100. The most recent return runs from the previous close to the 15:44 bar's close. eVRP = VIX − eRV, with VIX and VIX3M read at 15:44.
- Target exposure, as in the paper's Table 2 for `variant` (the default is `evrp_boc_sizing`):

  | Condition | `passive` | `evrp` | `evrp_boc` | `evrp_boc_sizing` |
  |---|---|---|---|---|
  | eVRP > 0, VIX < VIX3M | short 20% | short 20% | short 20% | short VIX/100 |
  | eVRP ≤ 0, VIX < VIX3M | short 20% | cash | short 10% | short 0.5 × VIX/100 |
  | eVRP ≤ 0, VIX > VIX3M | short 20% | cash | long 20% | long VIX/100 |
  | otherwise | short 20% | cash | cash | cash |

  "Short" means VIXSHORT exposure and "long" means VIXLONG exposure.
- Trade only when the held exposure differs from the target by more than `rebalance_band` (2 pp), or the sign or instrument changes. Fill with a market-on-close order. Reason: "eVRP {e}, VIX/VIX3M {r} → {short|long|cash} {x}%".
- Pitfalls:
  - The daily VIX and VIX3M closes print at 16:15, after the decision. Using them is lookahead. The signal needs the 15:44 index values.
  - The SVXY leverage change: before 2018-02-28 the exposure is the SVXY weight; from 2018-02-28 it is half the SVXY weight. The rebalance band works in exposure, not in weight.
  - SVXY and VIXY have frequent splits and reverse splits. Compute returns with split-adjusted history and size orders against raw prices.
  - Real ETP fees (1.15% for SVXY and VIXY in the paper's Table 1) exceed the proxies' 0.80% and 0.50%, which costs about 0.1% a year at the average exposure of about 17%. Report this; do not correct for it.
  - Strategy 3 uses eVRP ≤ 0 and Strategy 4 uses eVRP < 0. Treat eVRP = 0 as ≤ 0 in both. VIX = VIX3M falls to "otherwise" (cash).

## Gaps
- G6: market-on-close orders. The paper signals at 15:45 and executes at that day's close. Filling at the next open would eat the overnight gap, which is where the 5–6 February 2018 loss happened.
- G8: 1-minute VIX and VIX3M from 2016 (the 15:44 values), not only daily Cboe series. VX futures settlements are not needed by this bot.

## Done when
- For 2016–2024 (the 1m window), `evrp_boc_sizing` calendar-year returns are within ±5 pp of the paper's Table 4: 12.1, 15.2, −7.0, 3.5, 42.3, 26.1, 1.8, 22.9, 1.4.
- Over the same years, the four variants' Sharpe ratios rank as in Table 3 (0.48 < 0.56 < 0.87 < 1.00 over 2008–2025). `evrp_boc_sizing` trades about 79 times a year, short about 90% of days and long about 4%.
- The PR shows the daily positions and P&L from 1–9 February 2018 and 20 February to 31 March 2020, against `passive` (unconditional short volatility) and SPY. It also reports max drawdown, the paper's 20-day-median adjusted drawdown (−12.4% over the full sample) and correlation with SPY (paper 0.15).
