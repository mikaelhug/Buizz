# munger

Complex · Gaps: G2, G3, G16

## Idea
Charlie Munger's approach, as he described it: buy a few wonderful businesses at fair prices and sit on them. In his 1989 letter Buffett credits Munger with "a wonderful company at a fair price" beating "a fair company at a wonderful price".

Munger published no formula, so this bot uses the quantitative research that operationalises his words:
- **Wonderful** means a long record of high, stable returns on capital and gross margins, plus cash generation and financial strength: Gray & Carlisle's "franchise power" and financial strength, built for exactly this, with ROIC and its persistence as Mauboussin measures them.
- **Fair price** means not paying a premium: Novy-Marx's "quality at a value price", with EBIT/TEV as the price measure, the best performer in Gray & Carlisle's tests.
- **Concentration and sitting**: a handful of names, equal weight at entry, no rebalancing, sold only when quality breaks or the price becomes absurd.

This is distinct from `buffett`, which is FKP's diversified (about 50 names), monthly-rebalanced, 1.6×-levered QMJ + BAB + value portfolio. `munger` is unlevered, holds about 5 names on a multi-year franchise record, and almost never trades.

## Sources
- Munger (1994), "A Lesson on Elementary, Worldly Wisdom", USC: few bets, "sit on your ass". Munger (1998), "Investment Practices of Leading Charitable Foundations": "just three fine domestic corporations". Both are in *Poor Charlie's Almanack*. Daily Journal Corp. 13F filings show 4–5 positions, held for years.
- Buffett (1989), Berkshire Hathaway shareholder letter (wonderful company at a fair price).
- Gray & Carlisle (2012), *Quantitative Value*, ch. 5–7. The 8-year franchise power: geometric-mean ROA and ROC, sum of FCF over assets, and the larger of gross-margin growth and gross-margin stability. Financial strength: a 10-signal FS-score. Price: EBIT/TEV.
- Novy-Marx (2013), "The Other Side of Value", *JFE* (gross profitability). Novy-Marx (2014), "Quality Investing", working paper (quality bought without paying a premium is value).
- Mauboussin & Callahan (2014), "Calculating Return on Invested Capital", Credit Suisse. Mauboussin, Callahan & Majd (2016), *The Base Rate Book*, Credit Suisse (how ROIC persists and fades).
- Cohen, Polk & Silli (2010), "Best Ideas" (managers' highest-conviction few stocks outperform). Barber & Odean (2000), "Trading Is Hazardous to Your Wealth", *JF* (the turnover drag).

## Manifest
```toml
description = "Munger: a handful of durable franchises at fair prices, held for years"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 1000 }
params = { holdings = 5, record_years = 8, quality_top = 0.10, value_floor = 0.50, sell_quality_floor = 0.30, sell_value_floor = 0.10, rerank_month = 4 }
```

## Logic
- Exclude financials and utilities (SIC 6000–6999 and 4900–4949), as Greenblatt and Gray & Carlisle do: ROIC and EBIT/TEV mean nothing for them.
- Per fiscal year, from annual (FY) facts only:
  - NOPAT = `OperatingIncome` × (1 − t).
    - The tax rate t = `IncomeTax` / `PretaxIncome`, clamped to [0, 0.5].
    - When `PretaxIncome` ≤ 0, t is the statutory rate: 35% to 2017, 21% from 2018.
  - Invested capital = `Equity` + `LongTermDebt` + short-term debt (G16) − `CashAndEquivalents`.
  - ROIC = NOPAT / the average of opening and closing invested capital.
  - ROA = `NetIncome` / `TotalAssets`.
  - FCF = `OperatingCashFlow` − `Capex`. Check the sign: EDGAR reports capex as a positive outflow.
  - Gross margin = `GrossProfit` / `Revenue`, or (`Revenue` − `CostOfRevenue`) / `Revenue`.
  - Gross profitability = `GrossProfit` / `TotalAssets`.
- Require `record_years` consecutive fiscal years of all inputs, with ROIC > 0 in every year.
- Franchise power is the average percentile, across the universe, of:
  - 8-year geometric-mean ROIC;
  - 8-year geometric-mean ROA;
  - 8-year sum of FCF / current `TotalAssets`;
  - the larger of the gross-margin growth percentile (8-year geometric growth) and the gross-margin stability percentile (mean / standard deviation).
- Financial strength = Gray & Carlisle's FS-score (10 binary signals), as a percentile.
- Quality = the average of the franchise-power and financial-strength percentiles. Report gross profitability next to it as Novy-Marx's single-measure check.
- Price: EBIT/TEV = `OperatingIncome` / (market cap + `LongTermDebt` + short-term debt + preferred + minority interest − `CashAndEquivalents`).
  - Market cap = split-adjusted `SharesOutstanding` × close.
  - Short-term debt, preferred and minority interest need G16.
- Buy:
  - At the yearly re-rank (first session of `rerank_month`, after most December 10-Ks), candidates are the top `quality_top` by quality whose EBIT/TEV is at or above the universe median (`value_floor`): a fair price, not a premium.
  - Fill empty slots with the highest EBIT/TEV candidates, each at 1/`holdings` of equity. Never rebalance winners.
  - Reason: "quality pct {q}, 8y ROIC {r}, EBIT/TEV {y}".
- Sell: Munger published no sell rule, so these buffers implement "sell only when the business breaks or the price is absurd". Check each holding when its annual report arrives:
  - "quality broke": the holding falls out of the top `sell_quality_floor` by quality, or ROIC turns negative;
  - "price absurd": EBIT/TEV falls into the universe's bottom `sell_value_floor`.
- Pitfalls:
  - Shares must be adjusted for splits after `period_end`. Several share classes must be summed.
  - XBRL 10-K comparatives reach two to three years before 2009/2011, so an 8-year record is first valid around 2016 for large filers and 2018 for all filers.
  - A missing metric (an extension tag) excludes the name; it is never zero.
  - Delisted holdings liquidate at their last close.

## Gaps
- G2: stocks only.
- G3: excluding financials and utilities needs SIC codes.
- G16: invested capital and TEV need short-term debt, preferred stock and minority interest.

## Done when
- The PR reports annual turnover and the average holding period (target: turnover under 15% a year, holdings kept for years, as Daily Journal's were).
- The PR regresses returns on the Fama–French five factors and AQR's QMJ, and expects positive RMW/QMJ loadings with HML at or above zero.
- The PR compares the 5-name portfolio with the same rules at 30 names (the cost or benefit of concentration, against Cohen–Polk–Silli), with a top-decile gross-profitability portfolio (Novy-Marx) and with SPY and BRK-B. It reports single-name drawdowns honestly.
