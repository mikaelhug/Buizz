# alpha_lab

Exercise · Gaps: G1, G2, G5, G12, G13, G19, G20, G21, G22, G23

## Idea
An overnight exercise that tests which data and which model complexity actually predict US stock returns.
- Nobody knows in advance which mix is best, so nothing is ruled out up front. Every combination of six data groups is crossed with four model complexities, three horizons and two universes, and every trial is run.
- It is one parameterised bot. Each trial is a walk-forward backtest run by a sweep harness (G23).
- In the morning it reports which data groups add value, whether complexity pays, and which few configurations deserve the locked holdout.

## Data groups
Each group has a canonical signal from its paper, plus the full feature set used by the fitted models.

| Group | Data | Canonical signal | Features for fitted models | Gap |
|---|---|---|---|---|
| P | Daily prices and volume | 12-1 momentum (Jegadeesh & Titman 1993) | The `ml_trend_quarterly` price features | none |
| F | Report numbers (EDGAR XBRL) | Gross profitability (Novy-Marx 2013) | Every `FundamentalMetric` as a ratio to price or assets, ranked | none |
| I | Insider trades (Forms 3/4/5) | Opportunistic insider purchases (Cohen, Malloy & Pomorski 2012) | Net buying by opportunistic and routine insiders over 1, 3 and 12 months; buyer count; officer vs director | G19 |
| T | Report text (10-K, 10-Q) | Similarity to the prior filing (Cohen, Malloy & Nguyen 2020, "Lazy Prices") | Cosine and Jaccard similarity by section; Loughran–McDonald tone, uncertainty and litigiousness (Loughran & McDonald 2011) | G20 |
| N | News (headlines and bodies) | Fraction of negative words (Tetlock 2007; Tetlock, Saar-Tsechansky & Macskassy 2008) | Article count and tone over 1, 5 and 21 days; SESTM sentiment trained only on data before each test year (Ke, Kelly & Xiu 2019) | G21 |
| G | Google search attention | Abnormal search volume: log SVI minus the log median of the prior 8 weeks (Da, Engelberg & Gao 2011) | Abnormal search volume over 1, 4 and 12 weeks; its change | G22 |

- Every value is visible only from its own timestamp: a filing's acceptance time, an article's publish time, a search snapshot's recording time. A value seen during the run is never revised afterwards.
- Text and news models may not use a language model whose training data overlaps the test period. Dictionary methods and models fitted inside the walk-forward are allowed.

## Grid
| Axis | Values | Count |
|---|---|---|
| Data | Every non-empty subset of {P, F, I, T, N, G} | 63 |
| Model | `rule`: equal-weight average of the canonical signal ranks; `linear`: ridge regression on ranked features; `gbrt`: LightGBM as in `gbm_ranker`; `nn`: Gu–Kelly–Xiu NN3 | 4 |
| Horizon | 5, 21 and 63 sessions; the rebalance frequency matches the horizon | 3 |
| Universe | Top 500 and top 3,000 US common stocks by dollar volume | 2 |

That's 1,512 trials. All are run, including combinations the literature says should not work, such as news on a quarterly horizon.

## Logic
- Universe: US common stocks only (G2), as-of, including stocks that later delisted.
- Target: the forward total return over the horizon, as a cross-sectional percentile rank.
- Walk-forward:
  - Train on 5 years and validate on 1, then test the next year, rolling forward yearly.
  - Purge rows whose targets cross a boundary.
  - Tune hyperparameters on validation only.
  - Determinism and seeds as in `gbm_ranker`.
- Portfolio: at each rebalance (G1), hold the top 25 at equal weight, long-only, filled at the next open. The decile spread is measured from the predictions as a forecast check; it is not traded.
- Costs: the standard cost model and ADV cap on every trial.
- Feature code lives in shared bot code (G13). `alpha_lab` reuses the price features of `ml_trend_quarterly` and the fundamental features of `gbm_ranker` instead of copying them.
- Windows:
  - The main factorial runs on the common history of P, F, I, T and N. News sets the start: about 2015, so the first test year is about 2021.
  - G has no reproducible history before its recorder starts. Each trial that includes G runs over G's window, alongside its twin without G over the same window. G's value is measured only between those matched pairs.
- Order: cheap models first (`rule`, `linear`), then `gbrt`, then `nn`.
  - The harness estimates the night's runtime from a pilot slice.
  - It resumes the next night from cached results, keyed by config hash.
  - Each data group's feature panel is built once per window and shared across trials.

## Morning report
- A table of trials ranked by deflated Sharpe ratio (Bailey & López de Prado 2014). Columns: net Sharpe, CAGR, maximum drawdown, turnover, out-of-sample rank IC and decile spread.
- The probability of backtest overfitting for the whole sweep, from CSCV (Bailey, Borwein, López de Prado & Zhu 2017).
- The marginal value of each data group: its Shapley value across the 63 subsets, per model, horizon and universe. The full factorial makes this exact, and it answers which data matters.
- Complexity curves: the same data under `rule`, `linear`, `gbrt` and `nn`.
- Canonical-signal checks: each group's paper signal alone, compared with the sign and size the paper reports. A group whose canonical signal fails gets its feed audited before its trials are trusted.
- Finalists:
  - At most three configurations go to the locked holdout, once each.
  - They need a positive deflated Sharpe and must beat both `momentum_12_1` and their own `rule` twin.
  - The holdout runs are counted.

## Gaps
- G1: weekly, monthly and quarterly rebalance dates.
- G2: stocks only.
- G5: lightgbm, plus torch for `nn`.
- G12: past cross-sections for every training month.
- G13: shared feature code with `ml_trend_quarterly` and `gbm_ranker`.
- G19: insider transactions.
- G20: filing text.
- G21: news.
- G22: search attention.
- G23: the sweep harness.

## Done when
- One sweep of all 1,512 trials completes. Every trial is in the trial ledger with its config hash, metrics and return series.
- The morning report has the five parts above and is reproducible from the ledger.
- The holdout is touched only by the finalists, and the count is recorded.
- Results are reported in the PR and on the website, not in the repo.
