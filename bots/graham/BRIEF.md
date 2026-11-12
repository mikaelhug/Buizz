# graham: defensive variant

Complex · Gaps: G14

The net-net method (primary) and the 1976 rule (`method = "rule_1976"`) are in `bot.py`. This brief is what is left: Graham's defensive-investor criteria as a third `method`, blocked by G14.

## Sources
- Graham (1973), *The Intelligent Investor*, 4th rev. ed.: ch. 5 (10–30 issues for the defensive investor), ch. 14 (defensive criteria).

## Params
`defensive_min_names = 10`, `defensive_max_names = 30`, `size_floor_sales_1972_usd = 100000000`, `size_floor_utility_assets_1972_usd = 50000000`.

## Logic
All seven ch. 14 criteria, as Graham states them:
- **Size**: sales ≥ $100M for industrials, or total assets ≥ $50M for public utilities, in 1972 dollars. Scale each year by CPI-U (`ctx.macro(["cpiaucns"])`, annual average of the prior calendar year ÷ the 1972 average of 41.8), so about $510M in 2010 and $730M in 2024. Utilities by SIC code (`ctx.instruments`, current values only).
- **Financial condition**:
  - industrials: `current_assets` ≥ 2 × `current_liabilities`, and `long_term_debt` ≤ `current_assets` − `current_liabilities`;
  - utilities: total debt (`short_term_debt` + `long_term_debt`) ≤ 2 × `equity`.
- **Earnings stability**: `eps_diluted` > 0 in each of the last 10 fiscal years.
- **Dividend record**: uninterrupted dividends for at least 20 years. Derive each year's payments from the ratio of `adjust = "all"` to `adjust = "split"` closes: every step in that ratio is an ex-dividend date.
- **Growth**: the 3-year average EPS of the latest three fiscal years is at least 4/3 × the 3-year average ten years earlier (FY t−2..t against t−12..t−10).
- **P/E**: price ≤ 15 × the average EPS of the last three fiscal years.
- **Price to assets**: price ≤ 1.5 × the last reported book value per share, or (P/E above) × P/B ≤ 22.5.
- **Portfolio**: once a year, equal-weight all passers, at least 10 and at most 30 issues (ch. 5). If more than 30 pass, take the lowest P/E × P/B. At each yearly review, sell names that no longer pass; Graham gives no other sell rule.
- Reason: "passes all 7 defensive criteria, P/E {pe}".
- EPS must be adjusted for every split after it was filed; the 13 years of EPS the growth test needs come mostly from 10-Ks filed before later splits.

## Gaps
- G14: 10 years of EPS (13 for the growth test) need fundamentals before XBRL (2009 for large filers, 2011 for all). Without them the first valid signal is about 2022–2024. The 20-year dividend record also needs daily bars from 20 years before the signal.

## Done when
The PR shows passers per year and returns against a book-to-market decile and SPY, over the years where all seven criteria are computable.
