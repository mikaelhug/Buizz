# gbm_ranker

Complex · Gaps: G1, G2, G3, G5, G7, G10, G11, G12, G14, G15, G16

## Idea
Gu, Kelly & Xiu's gradient-boosted regression trees (GBRT+H):
- Predict each stock's next-month excess return from 94 firm characteristics, their interactions with 8 macro predictors, and 74 industry dummies (920 inputs).
- Use shallow boosted trees with Huber loss, tuned on a rolling validation window and refitted every year.
- Each month, go long the top predicted decile and short the bottom decile, value-weighted.

## Sources
- Gu, Kelly & Xiu (2020), "Empirical Asset Pricing via Machine Learning", *RFS* 33(5), and its Internet Appendix (Table A.5 for hyperparameters, Table A.6 for the characteristics).
- Green, Hand & Zhang (2017), "The Characteristics that Provide Independent Information about Average U.S. Monthly Stock Returns", *RFS*: the characteristic definitions and SAS code that GKX adapt.
- Welch & Goyal (2008), *RFS*: the 8 macro predictors.
- Replications and decay: Avramov, Cheng & Metzker (2023), "Machine Learning vs. Economic Restrictions", *Management Science*; Leippold, Wang & Zhou (2022), "Machine learning in the Chinese stock market", *JFE*.
- Variant: Poh, Lim, Zohren & Roberts (2021), "Building Cross-Sectional Systematic Strategies by Learning to Rank", *Journal of Financial Data Science*.

## Manifest
```toml
description = "Gu–Kelly–Xiu GBRT+H stock selector, annual refit"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 6000 }

[params]
train_start_years = 18
validation_years = 12
refit = "yearly"
max_depth = [1, 2]
learning_rate = [0.01, 0.1]
max_trees = 1000
huber_quantile = 0.999
deciles = 10
weighting = "value"
long_short = true
objective = "huber"
seed = 11
```

## Logic
- Universe: all NYSE, AMEX and NASDAQ common stocks, including penny stocks and financials, as GKX use (about 6,200 a month). `top_dollar_volume` only has to be large enough to cover the listed universe.
- Features at each month end t, for each stock:
  - All 94 characteristics of GKX Table A.6, built with GHZ's definitions. They include `baspread`: GHZ's code takes it from CRSP ASKHI/BIDLO, which hold the daily high and low on days with trades, so daily bars suffice (verify against the SAS code).
  - Each characteristic is ranked cross-sectionally into [−1, 1] every month. A missing value is replaced by the cross-sectional median, which is 0.
  - The interactions c ⊗ (1, x_t) with the 8 Welch–Goyal predictors: dp, ep, bm, ntis, tbl, tms, dfy, svar.
  - 74 dummies for 2-digit SIC codes.
  - In total 94 × 9 + 74 = 920 inputs.
- Information lags, as in GKX: a monthly characteristic is used at most 1 month old, a quarterly one at least 4 months after period end, and an annual one at least 6 months after. Apply these on top of EDGAR's filing-date visibility.
- Target: the stock's total return over month t+1 minus the T-bill rate.
- Sample scheme:
  - Train on at least 18 years and validate on the next 12. The test year follows.
  - Each year, extend the training window by one year and roll the 12-year validation window forward.
  - No cross-validation, which keeps the time order.
  - Drop the last month of each training block whose target falls in the validation block, and do the same at the validation/test boundary.
- Model: LightGBM GBRT matching GKX.
  - Use `objective = "huber"` with δ at the 99.9% quantile, `max_depth` ∈ {1, 2} (`num_leaves` = 2^depth), learning rate ∈ {0.01, 0.1}, and 1 to 1000 trees.
  - Choose all three by validation MSE.
  - Bagging, feature subsampling and L1/L2 penalties stay off, so the model is plain boosting.
- Determinism: `deterministic = true`, `force_row_wise = true` and `num_threads = 1`. Derive every LightGBM seed (`seed`, `bagging_seed`, `feature_fraction_seed`, `data_random_seed`) from `seed`. Sort rows by (month, symbol), use CPU only and pin the LightGBM version.
- Portfolio, at the last session of each month: sort into deciles by predicted return. Hold the top decile long and the bottom decile short, each leg weighted by market cap. Reason: "pred {p}, decile {d}, top SHAP {f1,f2,f3}".
- State: the model does not fit in 64KB. On resume it is retrained bit-identically from the same data, and state holds only the last refit year.
- Variants:
  - `long_short = false`: the top decile alone, which had a value-weighted Sharpe of 0.69 in GKX.
  - `weighting = "equal"`: GKX Table A.9.
  - `objective = "lambdarank"` (Poh et al. 2021): LightGBM LambdaMART on monthly cross-sectional decile labels, with the same features and split.
- Pitfalls:
  - Use point-in-time universes and fundamentals for every training month (G12, G14).
  - Delisting returns belong in the target.
  - The training panel is about 30 years × 12 months × 5,000 stocks × 920 features. The agent measures memory and refit time on the Buizz host before building.

## Gaps
- G1: month-end formation.
- G2: stocks only.
- G3: SIC codes for the 74 dummies and for `indmom`, `herf`, `sin`, `ms` and the industry-adjusted characteristics (`bm_ia`, `cfp_ia`, `chatoia`, `chempia`, `chpmia`, `mve_ia`, `pchcapx_ia`).
- G5: lightgbm.
- G7: earnings-announcement dates for `ear` and `aeavol`.
- G10: the short decile.
- G11: survivorship caveat.
- G12: past cross-sections for every training month.
- G14: 30 years of history before the first test year (18 years of training plus 12 of validation), so fundamentals before 2009.
- G15: the Welch–Goyal macro series as point-in-time monthly data.
- G16: XBRL concepts outside `FundamentalMetric`:
  - employees, for `hire` and `chempia`;
  - convertible debt, for `convind`;
  - secured debt, for `secured` and `securedind`;
  - buildings and land, for `realestate`;
  - advertising, for `ms`.

## Done when
- The PR reports the out-of-sample R² per year and pooled, measured against a zero forecast without demeaning. It compares it with GKX GBRT+H: 0.34% on all stocks, and 0.52–0.70% for trees and networks on the top 1,000.
- It reports the value-weighted decile table and the H-L spread. GKX GBRT+H: 0.99%/month, SD 4.22%, annualised Sharpe 0.81, max drawdown 48.8%, monthly turnover 144%. It also reports the spread excluding microcaps, as in Avramov et al.
- It reports variable importance as the drop in R² when a feature is set to 0. It compares that with GKX's leading groups:
  - price trend: mom1m, mom12m, chmom, indmom, maxret, mom36m;
  - liquidity: turn, std_turn, mvel1, dolvol, ill, zerotrade, baspread;
  - risk: retvol, idiovol, beta, betasq;
  - valuation: ep, sp, agr, nincr.

  It also counts the distinct features the trees use (GKX: about 30 early, 50 later).
- An embargo audit shows that no feature, lag or target crosses the clock, and that a refit on resume produces an identical model hash.
