# buffett

Complex · Gaps: G1, G2, G4, G10, G11, G13, G15, G16, G17

## Idea
Frazzini, Kabiller & Pedersen (FKP) show that Berkshire Hathaway's alpha becomes insignificant once its returns are explained by:
- market, size, value and momentum;
- betting against beta (BAB): safe, low-beta stocks;
- quality minus junk (QMJ): cheap, profitable, high-quality stocks.

Buffett gets his return by levering these tilts about 1.6-to-1 (working paper) or 1.7-to-1 (*FAJ*), financed cheaply through insurance float.

FKP's systematic Buffett-style portfolio holds those factor tilts in Berkshire's proportions, at Berkshire's market beta and idiosyncratic volatility. It is a diversified long/short portfolio, not a 50-stock picker. This bot builds it at stock level. It is the research-faithful Buffett, distinct from the `value_quality` example bot, a simple SDK demo.

## Sources
- Frazzini, Kabiller & Pedersen (2018), "Buffett's Alpha", *FAJ* 74(4), 1976–2017: Sharpe 0.79, leverage about 1.7-to-1.
  - NBER w19681 (1976–2011): §7 for the systematic portfolio, Appendix C for the real-time version, Tables 2, 4 and 5 for the numbers below.
- Asness, Frazzini & Pedersen (2019), "Quality Minus Junk", *RAS* 24(1), Appendix 1: QMJ definitions.
- Frazzini & Pedersen (2014), "Betting Against Beta", *JFE*; built here as in `betting_against_beta`.
- Asness & Frazzini (2013), "The Devil in HML's Details", *JPM*: HML with current market equity, as in AQR's factors.

## Manifest
```toml
description = "Frazzini–Kabiller–Pedersen systematic Buffett portfolio"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 3000 }
params = { beta = 0.68, idio_vol = 0.224, loadings = { mkt = 0.27, smb = -0.15, hml = 0.46, umd = -0.05, bab = 0.29, qmj = 0.43 }, variant = "long_short", long_only_holdings = 50 }
```

## Logic
- Constants from FKP w19681, Berkshire stock 1976–2011; replace them with the *FAJ* 1976–2017 table values if obtained.
  - β_B = 0.68 is Berkshire's univariate market beta (Table 2).
  - Active loadings are Table 4 col. 3 with the market loading reduced by β_B: 0.95 − 0.68 = 0.27.
  - σ_I = 22.4% is Berkshire's idiosyncratic volatility (Table 2).
- At the last session of each month, build each factor's stock weights:
  - MKT: value-weighted universe.
  - SMB, HML and UMD: Fama–French 2×3 sorts. Size splits at the NYSE median; B/M or 12-2 momentum at the 30th and 70th NYSE percentiles. Value-weighted.
  - BAB: FP rank weights with the 1/β_L and 1/β_H scaling, from the `betting_against_beta` code.
  - QMJ: 2×3 sort on size (NYSE median) and quality (30th and 70th percentiles), value-weighted. QMJ = ½(small quality + big quality) − ½(small junk + big junk).
- Active book a = Σ loading_f × w_f. Scale c = σ_I / σ̂(a), where σ̂(a) is the annualised volatility of today's active weights applied to the last 252 days of returns (holdings-based, so no lookahead).
- Target weights = (β_B + c × 0.27) on MKT + c × (the other loadings) on their factors, netted per stock. In the working paper this reaches about 3.7× gross (Table 2). Reason: "net tilt {w}: qmj {q}, bab {b}, hml {h}".
- QMJ (AFP 2019): quality = z(profitability + growth + safety). z(·) is the cross-sectional rank standardised to mean 0 and sd 1, re-standardised at each level. A missing measure is dropped from its average. A stock needs 5 years of history.
  - Profitability: GPOA, ROE, ROA, CFOA, GMAR and ACC.
    - GPOA = (`Revenue` − `CostOfRevenue`) / `TotalAssets`, or `GrossProfit` / `TotalAssets`.
    - ROE = IB / BE and ROA = IB / `TotalAssets`. IB ≈ `NetIncome` (it includes discontinued operations).
    - BE = `Equity` − preferred stock (G16).
    - CFOA = (IB + `DepreciationAmortization` − ΔWC − `Capex`) / `TotalAssets`.
    - GMAR = gross profit / `Revenue`.
    - ACC = −(ΔWC − D&A) / `TotalAssets`.
    - WC = `CurrentAssets` − `CurrentLiabilities` − (`CashAndEquivalents` + short-term investments) + short-term debt + income taxes payable. The last three are G16.
  - Growth: 5-year change in residual per-share GPOA, ROE, ROA and CFOA, minus r_f × lagged book or assets, scaled by the t−5 denominator; plus (gp_t − gp_t−5) / sale_t−5. Per-share uses split-adjusted `SharesOutstanding`. The risk-free rate r_f comes from `rate_daily` (G15).
  - Safety:
    - BAB = −β (FP estimator).
    - LEV = −(`LongTermDebt` + short-term debt + minority interest + preferred stock) / `TotalAssets` (G16).
    - Ohlson O, negated. It needs short-term debt (G16); the CPI deflator is the same for all stocks on a date, so it cancels in the ranks.
    - Altman Z = (1.2 WC + 1.4 retained earnings + 3.3 EBIT + 0.6 ME + sales) / AT, with retained earnings from G16 and EBIT ≈ `OperatingIncome`.
    - EVOL = −σ of quarterly ROE over 60 quarters, with at least 12.
  - The payout variant adds EISS, DISS and NPOP as a fourth component (AFP alternative definition). EISS = −Δlog split-adjusted shares; DISS = −Δlog total debt (G16); NPOP = Σ5y (IB − ΔBE) / Σ5y gross profit.
- Variant `long_only` (FKP §7 and Appendix C): sort on the stock weights of the active tilts c × a. Hold the top 50, equal-weighted and unlevered, rebalanced monthly. Needs neither G4 nor G10.
- Variant `realtime` (FKP Appendix C): each month, re-estimate β_B, the loadings and σ_I from BRK-B against the factor returns using only data up to t, with at least 60 months. Factor history before the run needs G12 and G14 (or published factor series).
- Pitfalls:
  - XBRL facts start in 2009–2011, and QMJ growth needs 5 years and EVOL 12 quarters, so the first faithful signal is about 2014–2016.
  - Dual-class issuers (one CIK, several symbols) need company-level market cap.
  - Fundamentals are visible only from the open after filing.

## Gaps
- G1: month-end formation.
- G2: stocks only.
- G4: about 3.7× gross with financing; the leverage is the result, not a variant.
- G10: shorts in every factor leg.
- G11: the CRSP universe.
- G13: reuse the `betting_against_beta` estimator and BAB factor rather than copying them.
- G15: the T-bill rate from `rate_daily`, for residual-income growth.
- G16: missing fundamentals for QMJ.
- G17: NYSE listing for size and percentile breakpoints.

## Done when
- A monthly regression of the bot's excess returns on its own six factors (or AQR's published MKT, SMB, HML, UMD, BAB and QMJ for the same months) recovers market beta ≈ 0.68 and loadings in the target proportions.
- The correlation with BRK-B's monthly excess returns is reported against FKP Table 5's 0.47 for Berkshire stock (and 0.73 for its 13F portfolio). Volatility is compared with BRK-B's.
- The `long_only` variant reports Sharpe and beta against FKP Table 2: 0.45 and 0.83 for the Berkshire-style long-only portfolio, 1976–2011.
