# ml_trend_quarterly

Complex · Gaps: G1, G2, G5, G12

## Idea
The research version of a retail strategy seen on X: "a self-built machine-learning model for trend/momentum, backtested on 100k+ quarters".
- The post describes the rules: fully rule-based, data-driven selection, quarterly rotation, "100% trend and momentum", about 25 stocks, and a target of 35% a year.
- The account's model, features and code are not public. The bot builds the strongest published form of that idea on US stocks and tests the target against it.
- A gradient-boosted tree model reads only price and volume history: past-return windows, trend shape and volatility.
- It predicts each stock's return over the next quarter.
- At each quarter end it buys the 25 top-ranked stocks long-only and holds them for the quarter.
- "100k+ quarters" fits a panel of stock-quarter rows: about 2,000 stocks × 5 years × 12 monthly formation dates is about 120,000 rows.

## The claim, read critically
- 35% a year is the account's target, not a track record. The live record covers about two years.
- 35% is about twice the top 10% of stocks in every published quarterly price-trend model. In Jiang, Kelly and Xiu's best quarterly models, that top group earned 13–16% a year before costs, 2001–2019.
- Plain momentum without ML already captures much of the effect. The bot has to beat `momentum_12_1` to show that the ML layer adds anything.
- Likely sources of inflation in backtests like this:
  - Many configurations tried and the best one reported.
  - Universes without delisted stocks.
  - Illiquid small caps whose prices cannot be traded.
  - Labels that overlap the test period.
- 100k rows sounds large, but 10 years of quarterly trading gives only 40 independent cross-sections.

## Sources
- Moritz & Zimmermann (2016), "Tree-Based Conditional Portfolio Sorts: The Relation Between Past and Future Stock Returns", working paper:
  - Random forests on past-return deciles alone.
  - US stocks, 1963–2012, tested out of sample over 1968–2012.
  - The equal-weighted top-minus-bottom decile spread earned 2.30% a month, with a Sharpe ratio of 2.96 and monthly turnover of 318%.
  - The six most recent one-month returns matter most.
- Jiang, Kelly & Xiu (2023), "(Re-)Imag(in)ing Price Trends", *Journal of Finance* 78(6):
  - A CNN on price-chart images.
  - Table II gives quarterly results (prediction horizon R60) for 2001–2019.
  - The quarterly benchmark numbers below come from it.
- Gu, Kelly & Xiu (2020), *RFS*: the training and validation scheme. Price-trend features lead their variable importance.
- Price-trend features:
  - Jegadeesh & Titman (1993): 12-1 momentum.
  - Novy-Marx (2012): 12-7 intermediate momentum.
  - George & Hwang (2004): closeness to the 52-week high.
  - Bali, Cakici & Whitelaw (2011): MAX, the largest recent daily return.
  - Heston & Sadka (2008): the return 12 months ago.
- Overfitting checks: Bailey & López de Prado (2014), "The Deflated Sharpe Ratio"; Bailey, Borwein, López de Prado & Zhu (2017), "The Probability of Backtest Overfitting".

## Manifest
```toml
description = "Gradient-boosted trend/momentum model, top 25 stocks held each quarter"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 3000 }

[params]
train_years = 5
validation_years = 1
refit = "yearly"
horizon_sessions = 63
max_depth = [1, 2, 3]
learning_rate = [0.01, 0.1]
max_trees = 1000
top_n = 25
model = "gbrt"
seed = 7
```

## Logic
- Universe: US common stocks only (G2), the top 3,000 by dollar volume as-of, including stocks that later delisted.
- Features for each stock at each month end:
  - The 25 one-month returns over the past two years, Moritz–Zimmermann's R(g,1) for g = 0…24.
  - Price-trend features:
    - 5-day return;
    - returns over 3, 6, 12-1 and 12-7 months;
    - close/52-week high;
    - close/SMA50 and close/SMA200;
    - slope × R² of a 90-session regression on log price;
    - MAX over 21 sessions;
    - 63-session realised volatility;
    - change in 63-session volume vs 252-session volume;
    - log dollar volume.
  - Every feature is ranked cross-sectionally into [−1, 1] each month. A missing value is set to 0.
  - Total-return history only (`adjust = "all"`). Nothing is read after the formation close.
- Target: the stock's 63-session forward total return, as a cross-sectional percentile rank.
- Training rows:
  - Every month end gives rows whose quarterly targets overlap, as Jiang–Kelly–Xiu sample 60-day windows more often than quarterly.
  - The training window is the past 5 years, as Moritz–Zimmermann use, followed by 1 validation year.
  - Purge the 3 months of rows before each validation and test boundary whose targets cross it.
  - No shuffled cross-validation.
- Model:
  - LightGBM regression. Choose `max_depth` ∈ {1, 2, 3} (`num_leaves` = 2^depth), learning rate ∈ {0.01, 0.1} and 1–1000 trees by validation MSE.
  - Bagging and feature subsampling stay off.
  - Refit once a year at the first quarter end.
  - Use the same determinism settings as `gbm_ranker`: `deterministic`, `force_row_wise` and `num_threads = 1`, every seed derived from `seed`, rows sorted by (month, symbol), and a pinned LightGBM version.
- Portfolio:
  - At the last session of each calendar quarter (G1), rank stocks by prediction.
  - Hold the top 25 at equal weight and exit names that drop out.
  - Fill at the next open. It is a quarterly rebalance, so the execution rule applies and no G6 is needed.
  - Reason: "pred rank {r}/{n}, top features {f1,f2,f3}".
- State: the model does not fit in 64KB. On resume, retrain bit-identically from the same data. State keeps only the last refit date and the holdings.
- Variant `model = "cnn"`: Jiang–Kelly–Xiu, the I20/R60 and I5/R60 transfer models.
  - OHLC images with a moving-average line and volume bars.
  - A 70/30 random train/validation split over 8 years before the test, then the model is held fixed.
  - Five retrained CNNs averaged, Adam at 1e-5, batch 128, batch norm, 50% dropout on the fully connected layer, and early stopping after 2 epochs.
  - Needs torch (G5).

## Gaps
- G1: quarter-end and month-end formation.
- G2: stocks only.
- G5: lightgbm, plus torch for the CNN variant.
- G12: past cross-sections for every training month. That includes daily bars before 2016 for every listed and delisted stock, so the first test year has 5 years of training behind it. For the CNN variant it needs 8 years.

## Done when
- The PR reports CAGR, Sharpe, maximum drawdown and turnover on the 10y preset, each net of costs, and compares them with:
  - SPY;
  - the `momentum_12_1` example bot over the same window;
  - an equal-weight portfolio of the universe;
  - the 35% target.
- It reports the out-of-sample decile table, equal-weighted at each quarter, including the top-minus-bottom spread. It compares this with Jiang–Kelly–Xiu's quarterly results (2001–2019, gross):

  | Strategy | Top-decile return a year | Top-minus-bottom Sharpe |
  |---|---|---|
  | I5/R60 | 16% | 1.30 |
  | I20/R60 | 13% | 0.37 |
  | I60/R60 | 15% | 0.43 |
  | MOM/R60 | 13% | 0.06 |

- It repeats the results without the smallest 1,000 stocks in the universe, which shows how much of the return depends on small caps.
- It reports feature importance as the drop in validation R² when a feature is zeroed. It compares that with Moritz–Zimmermann: the most recent six one-month returns and the return 12 months ago dominate.
- It reports an overfitting audit:
  - the number of configurations tried;
  - the deflated Sharpe ratio;
  - the probability of backtest overfitting from CSCV.
- It includes an embargo audit showing that no feature or target crosses the clock, and that a resumed refit reproduces the same model hash.
