# multi_factor_composite

Complex · Gaps: G1, G2, G3, G10, G11, G16

## Idea
Long/short large caps on one integrated score that combines four documented styles, each measured relative to the stock's industry:
- value: enterprise multiple, book-to-market;
- momentum: 12-2 month return;
- quality: gross profitability, low accruals;
- low risk: volatility.

Integrating the signals into one score before sorting beats running four separate portfolios side by side, because it holds stocks that are good on several styles at once and nets offsetting trades. Each month, go long the top quintile and short the bottom quintile, dollar-neutral in the stock legs and beta-neutral through a market hedge.

No single paper defines this exact portfolio. Each signal follows its source below; the combination follows Fitzgibbons et al.; the legs and hedge follow the standard sort and Novy-Marx & Velikov. Weights are fixed ex ante (equal across styles), never tuned (Novy-Marx 2016).

## Sources
- Value: Loughran & Wellman (2011), "New Evidence on the Relation between the Enterprise Multiple and Average Stock Returns", *JFQA*.
- Value: Asness & Frazzini (2013), "The Devil in HML's Details", *JPM* (B/M with current market equity).
- Momentum: Jegadeesh & Titman (1993), *JF*; Asness, Moskowitz & Pedersen (2013), "Value and Momentum Everywhere", *JF* (value and momentum combine because they are negatively correlated).
- Quality: Novy-Marx (2013), "The Other Side of Value", *JFE* (GP/A).
- Quality: Sloan (1996), *TAR*, with accruals measured from the cash-flow statement per Hribar & Collins (2002), *JAR*. The balance-sheet version carries measurement error.
- Low risk: Blitz & van Vliet (2007), *JPM* (3-year weekly volatility, as in `low_volatility`).
- Industry-relative signals: Asness, Porter & Stevens (2000), "Predicting Stock Returns Using Industry-Relative Firm Characteristics"; Novy-Marx (2013) §4.
- Integrate versus mix: Fitzgibbons, Friedman, Pomorski & Serban (2017), "Long-Only Style Investing: Don't Just Mix, Integrate", *JOI*.
- Costs and buffers: Novy-Marx & Velikov (2016), "A Taxonomy of Anomalies and Their Trading Costs", *RFS*.
- Hedging: Novy-Marx & Velikov (2022), *JFE* (hedge beta directly with the value-weighted market, not by levering legs).
- Robustness: Hou, Xue & Zhang (2020), "Replicating Anomalies", *RFS* (value-weighted results); Novy-Marx (2016), "Backtesting Strategies Based on Multiple Signals", NBER w21329 (overfitting in composites).

## Manifest
```toml
description = "Industry-neutral value, momentum, quality, low-vol long/short"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 1000 }
params = { quantile = 0.2, winsor = 0.01, styles = { value = 0.25, momentum = 0.25, quality = 0.25, low_risk = 0.25 }, construction = "integrate", weighting = "equal", hedge = "SPY", beta_days = 252, buffer = 0.0 }
```

## Logic
- Universe: about the 1000 largest liquid stocks. AMP use the largest stocks covering about 90% of market cap. Market cap = `SharesOutstanding` × close, split-adjusted, summed per company for dual-class issuers.
- Measures at the last session of each month. TTM = the last four fiscal quarters, with Q4 = FY − 9M because 10-Qs carry year-to-date values.
  - EBITDA/EV = (`OperatingIncome` + `DepreciationAmortization`, TTM) / EV.
    - EV = market cap + `LongTermDebt` + short-term debt + preferred stock − `CashAndEquivalents`.
    - Short-term debt and preferred stock are G16. Without them, EV is understated for firms with large maturing debt.
  - B/M = `Equity` / current market cap.
  - Momentum = total return from 252 to 21 sessions back.
  - GP/A = `GrossProfit` (TTM; else `Revenue` − `CostOfRevenue`) / `TotalAssets`.
  - Accruals = −(`NetIncome` − `OperatingCashFlow`, TTM) / average `TotalAssets` (this year and last).
  - Low risk = −σ of 156 weekly total returns.
- Per measure: winsorise at 1%/99%, then z-score within industry. Industry = Fama–French 49 industries from SIC (G3). Rank-based z (as in AFP) keeps tiny industries stable; the agent states its minimum industry size.
- Style score = mean z of its measures. Composite = Σ style weight × style z.
- Legs:
  - Long the top quintile of the composite, short the bottom quintile, equal-weighted, $1 long and $1 short.
  - Hedge to zero ex-ante beta with SPY. The beta is a holdings-based daily OLS beta of the stock book over the last 252 days (direct hedge, NMV 2022).
  - Reason: "composite z {x}: value {v}, mom {m}, qual {q}, risk {r}".
- Variants:
  - `construction = "mix"`: four quintile long/short sleeves, one per style, at 25% of capital each (the Fitzgibbons comparison).
  - `weighting = "value"`: value-weighted legs (the HXZ robustness check).
  - `buffer = 0.1`: buy into the top 20%, hold until a name leaves the top 30%; the short leg mirrors it. This buy/hold spread is NMV 2016's most effective cost mitigation.
- Pitfalls:
  - Fundamentals are visible only from the open after filing, and XBRL starts in 2009–2011. Average assets and TTM need 2 years, so the first signal is about 2011–2013.
  - Use split-adjusted shares.
  - Negative EBITDA or book equity stays in the rank as low value, never dropped.
  - Industry z-scores must use the same date's cross-section.

## Gaps
- G1: month-end formation.
- G2: stocks only.
- G3: industry-relative z-scores.
- G10: short-leg borrow.
- G11: the papers use CRSP or Russell universes.
- G16: short-term debt and preferred stock for EV.

## Done when
- Each style's standalone quintile long/short return is reported over the run. Its monthly correlation is reported against AQR's published US factors for the same months: HML Devil (value), UMD (momentum), QMJ (quality) and BAB (low risk).
- The value–momentum sleeve correlation is negative, as AMP report.
- The composite's Sharpe, realised beta (|β| ≤ 0.1 against SPY) and annual turnover are reported.
- `integrate` versus `mix` is reported on the same window: does integration add return and information ratio, as Fitzgibbons et al. find?
- The `value` and `buffer` variants are reported net of the cost model.
