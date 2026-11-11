# meta_allocator

Complex · Gaps: G1, G4, G5, G6, G13

## Idea
Run three low-correlation strategy sleeves inside one bot, scale each to the same ex-ante volatility, and hold them in equal risk (1/N in risk units). The sleeves are the three distinct return sources:
- trend: `trend_following`, which is time-series momentum with positive skew and crisis alpha;
- diversified risk premia: `hrp_allocation`, a long-only cross-asset HRP portfolio;
- short-horizon mean reversion: `ibs_reversion`.

`dual_momentum` is not a sleeve. It is the same 12-month time-series momentum signal as `trend_following`, applied to two equity indices, so it adds little but correlation.

The evidence favours static diversification over performance-chasing weights. DeMiguel, Garlappi & Uppal find no optimiser that reliably beats 1/N out of sample, and Asness et al. find contrarian factor timing adds little to a diversified mix. Weighting by trailing Sharpe is therefore not used. The one timing rule with broad out-of-sample support is time-series factor momentum, and it is run as the `sleeve_momentum` variant and compared with the static mix.

## Sources
- DeMiguel, Garlappi & Uppal (2009), "Optimal Versus Naive Diversification", *RFS*: estimation error swamps optimisation gains, so the base is 1/N.
- Maillard, Roncalli & Teïletche (2010), "The Properties of Equally Weighted Risk Contribution Portfolios", *JPM*: inverse-volatility weights equal ERC when correlations are equal.
- Asness, Chandra, Ilmanen & Israel (2017), "Contrarian Factor Timing is Deceptively Difficult", *JPM*.
- Harvey, Hoyle, Korgaonkar, Rattray, Sargaison & Van Hemert (2018), "The Impact of Volatility Targeting", *JPM*: inverse-volatility scaling.
- Moreira & Muir (2017), "Volatility-Managed Portfolios", *JF*, with Cederburg, O'Doherty, Wang & Yan (2020), *JFE*. Moreira and Muir's inverse-variance timing mostly fails in real-time out-of-sample tests across 103 strategies, so sleeves are vol-targeted (1/σ), not variance-timed (1/σ²).
- Ehsani & Linnainmaa (2022), "Factor Momentum and the Momentum Factor", *JF*. Factors earn about 53 bp a month after a positive year and about 1 bp after a negative one. Their time-series rule goes long factors with a positive prior one-year return and short those with a negative one, equal-weighted and rebalanced monthly.
- Gupta & Kelly (2019), "Factor Momentum Everywhere", *JPM*, an independent replication across 65 factors.

## Manifest
```toml
description = "Equal-risk allocation across trend, risk-premia and mean-reversion sleeves"
timeframe = "1d"
wake = "bar"
universe = { symbols = ["SPY", "QQQ", "IWM", "EFA", "EEM", "EWJ", "TLT", "IEF", "SHY", "LQD", "HYG", "GLD", "SLV", "USO", "DBA", "DBC", "UUP", "FXE", "VNQ", "BIL"] }
params = { sleeves = ["trend_following", "hrp_allocation", "ibs_reversion"], sleeve_vol = 0.10, vol_com_days = 60, timing = "none", momentum_months = 12 }
```

## Logic
- Universe: the union of the three sleeves' shipped `bot.toml` universes. The list above is their union as briefed; re-derive it from the shipped manifests.
- Each sleeve produces target weights exactly as its own bot does, by importing its signal code (G13), with its own cadence: trend and HRP at month-end, IBS at the close every day. HRP runs as its unlevered base variant.
- Keep each sleeve's daily return series in state: the return of its target weights after costs, starting from its first valid signal.
- At the last session of each month:
  - σ_s = annualised EWMA volatility of sleeve s's daily returns, centre of mass `vol_com_days` (the same estimator as `trend_following`);
  - scale k_s = `sleeve_vol` / σ_s;
  - book weight per symbol = Σ_s (1/N) × k_s × w_s,symbol.
  - Reason: "equal risk: trend ×{k1}, hrp ×{k2}, ibs ×{k3}".
- `timing = "sleeve_momentum"` multiplies each k_s by the sign of the sleeve's own return over the trailing `momentum_months` (Ehsani & Linnainmaa's long/short rule). A negative sign inverts that sleeve's weights. Reason: "sleeve momentum {s}: 12m {r}% → {long|short}". The PR also reports the long/flat form, which the paper says earns about the same average return, since losers earn about zero.
- Intra-month, sleeves update their own weights on their own schedule, with k_s held fixed until the next month-end.
- Net the sleeves' targets per symbol and per execution time: IBS orders go market-on-close, while monthly trend and HRP changes fill at the next open.
- Pitfalls:
  - A sleeve's return series must come from its own simulated book, not from the netted book. Otherwise the scaling feeds back into itself.
  - Before a sleeve has `3 × vol_com_days` of returns, give it no weight rather than an unstable σ. Before it has `momentum_months` of returns, the momentum multiplier is 1.
  - Netting can turn two sleeves' opposite SPY trades into one small order. Size from the netted target, but log each sleeve's contribution.
  - The state limit is 64 KiB. Keep about 300 days of three float32 return series plus the EWMA state, not full histories.
  - Gross exposure follows from k_s and the trend sleeve's own leverage. Report its distribution.

## Gaps
- G13: importing the sleeves' signal code.
- G1: month-end formation (trend, HRP and the allocation).
- G4: gross above 1× (the trend sleeve's own scaling, and k_s > 1 for the low-vol HRP sleeve).
- G5: scipy for HRP's linkage.
- G6: IBS enters and exits at the close it signals on, so the bot needs a pre-close wake and market-on-close orders.

## Done when
- Each sleeve's internal return series matches its standalone bot's backtest over the same period: daily correlation > 0.99 and annual return within 0.5 pp. Implementation starts only after the three sleeves have shipped.
- Each sleeve's realised volatility is within ±2 pp of `sleeve_vol`. The PR reports the pairwise sleeve correlations.
- The PR compares the equal-risk book with each sleeve alone, with 1/N capital weights and with `timing = "sleeve_momentum"`, on Sharpe, volatility, max drawdown and turnover.
- The PR reports each sleeve's mean monthly return after a positive versus a negative trailing 12 months, against Ehsani & Linnainmaa's 53 bp versus 1 bp.
