# graham

Complex · Gaps: G2, G16 (net-net, primary); G3, G14, G15 (defensive); G15 (1976)

## Idea
Benjamin Graham's value investing, in his own quantified rules. All three methods buy groups of stocks below a conservative appraisal and rely on diversification, not on analysing each name.

The primary method is the **net-net**: buy at no more than two-thirds of net current asset value. Graham chose it himself. In 1976 he said he was "no longer an advocate of elaborate techniques of security analysis" and called net-current-asset buying "a foolproof method of systematic investment", with about 20% a year over 30-odd years. It also has the strongest independent evidence: Oppenheimer (1986) and later replications.

Two variants, also Graham's own:
- **Defensive** (*The Intelligent Investor*, 1973, ch. 14): his seven criteria for the defensive investor.
- **1976 rule**: earnings yield at least twice the AAA bond yield and a company that owns twice what it owes.

## Sources
- Graham (1973), *The Intelligent Investor*, 4th rev. ed.: ch. 5 (10–30 issues for the defensive investor), ch. 14 (defensive criteria), ch. 15 (net current asset bargains).
- Graham (1976), "A Conversation with Benjamin Graham", *FAJ* 32(5): net-nets, the "seven times earnings" group method, sell at +50–100% or after 2–3 years.
- Graham (1976), "The Simplest Way to Select Bargain Stocks", *Medical Economics*, Sept. 20 (reprinted in Lowe, *The Rediscovered Benjamin Graham*, 1999): the AAA rule, the balance-sheet test, 30 stocks, and the exits.
- Oppenheimer (1986), "Ben Graham's Net Current Asset Values: A Performance Update", *FAJ* 42(6). Method: 1970–1983, buy at ≤ 2/3 NCAV on the November price, enter at the end of December, hold 12 months, equal weight. Result: 29.4% a year arithmetic mean (28.2% geometric) against 11.5% for the NYSE-AMEX index.
- Replications: Vu (1988), *J. Business Finance & Accounting*; Xiao & Arnold (2008), UK; Montier (2008), global; Carlisle (2014), *Deep Value*. The AAII "Graham's simplest approach" test (1998–2005) covers the 1976 rule.

## Manifest
```toml
description = "Graham net-nets at two-thirds of NCAV, with defensive and 1976 variants"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 6000 }
params = { method = "net_net", exit = "graham", min_names = 30, net_net_ratio = 0.667, exit_gain = 0.5, exit_calendar_years = 2, size_floor_sales_1972_usd = 100000000, size_floor_utility_assets_1972_usd = 50000000, defensive_min_names = 10, defensive_max_names = 30, max_pe_1976 = 10, min_pe_cap_1976 = 7, min_equity_to_assets_1976 = 0.5 }
```

## Logic
- **Net-net** (primary):
  - NCAV = `CurrentAssets` − `TotalLiabilities` − preferred stock (G16). Use the latest balance sheet, quarterly or annual.
  - Market cap = split-adjusted `SharesOutstanding` × close.
  - Buy when market cap ≤ `net_net_ratio` × NCAV.
  - Each new position gets 1/`min_names` of equity: Graham's minimum of 30 issues, with the rest in cash when fewer qualify. When qualifiers exceed cash, take the lowest price/NCAV first.
  - Exit (`exit = "graham"`): at +50% from cost, or at the end of the second calendar year after purchase, whichever comes first.
  - Reasons: "price {x} of NCAV", "+50% target", "2-year limit".
  - Comparison run (`exit = "oppenheimer"`): on the first session of each year, hold every qualifier at equal weight for 12 months.
- **Defensive** (variant): all seven ch. 14 criteria, as Graham states them.
  - **Size**: sales ≥ $100M for industrials, or total assets ≥ $50M for public utilities, in 1972 dollars. Scale each year by CPI-U (annual average of the prior calendar year ÷ the 1972 average of 41.8), so about $510M in 2010 and $730M in 2024.
  - **Financial condition**:
    - industrials: `CurrentAssets` ≥ 2 × `CurrentLiabilities`, and `LongTermDebt` ≤ `CurrentAssets` − `CurrentLiabilities`;
    - utilities: total debt ≤ 2 × `Equity`.
  - **Earnings stability**: `EpsDiluted` > 0 in each of the last 10 fiscal years.
  - **Dividend record**: uninterrupted dividends for at least 20 years. Derive each year's payments from the ratio of `adjust = "all"` to `adjust = "split"` closes: every step in that ratio is an ex-dividend date.
  - **Growth**: the 3-year average EPS of the latest three fiscal years is at least 4/3 × the 3-year average ten years earlier (FY t−2..t against t−12..t−10).
  - **P/E**: price ≤ 15 × the average EPS of the last three fiscal years.
  - **Price to assets**: price ≤ 1.5 × the last reported book value per share, or (P/E above) × P/B ≤ 22.5.
  - **Portfolio**: once a year, equal-weight all passers, at least 10 and at most 30 issues (ch. 5). If more than 30 pass, take the lowest P/E × P/B. At each yearly review, sell names that no longer pass; Graham gives no other sell rule.
  - Reason: "passes all 7 defensive criteria, P/E {pe}".
- **1976 rule** (variant):
  - Maximum P/E = 1 / (2 × the Moody's AAA yield). It is never above `max_pe_1976` (10), and a P/E of 7 is always acceptable.
  - P/E uses trailing 12-month EPS. Derive Q4 as FY − (Q1 + Q2 + Q3).
  - Require `Equity` / `TotalAssets` ≥ 0.5, i.e. the company owns twice what it owes.
  - Sizing and exits follow the net-net rules.
- Pitfalls:
  - EPS and share counts must be adjusted for every split after each `period_end`. Older 10-Ks are not restated.
  - XBRL 10-Ks carry 3 years of income statement and 2 years of balance sheet as comparatives, so usable history starts about two years before 2009/2011.
  - A missing metric (often an extension tag) fails the test. It is never zero.
  - Net-nets are mostly micro-caps, small reporting companies (XBRL from 2011) and often Chinese reverse mergers. Delisted names must liquidate at their last close, not vanish.
  - Several share classes: sum every class on the cover page.

## Gaps
- G2: stocks only.
- G16: NCAV deducts preferred stock (Oppenheimer's definition), and utility debt needs short-term debt.
- G3 (defensive): Graham applies different size and debt tests to industrials and public utilities. This needs SIC codes.
- G14 (defensive): 10 years of EPS (13 for the growth test) need fundamentals before XBRL. Without them the first valid signal is about 2020. The 20-year dividend record comes from price history, as long as daily bars reach 20 years before the signal.
- G15 (defensive, 1976): the 1976 rule needs the AAA yield, and the defensive size floor needs CPI-U. Both are FRED series, so G15 should carry CPI-U too.
- Capacity: the ADV participation cap will bind on net-nets. The PR must show it, not hide it.

## Done when
- Net-net: the PR shows qualifiers per year (Graham counted over 300, about 10% of the list, in January 1976; Oppenheimer's smallest year had 18). It also shows the `exit = "oppenheimer"` run's 12-month cohort statistics next to Oppenheimer's Table I/VI: mean annual return, losing years, loss-making against profitable firms, and price/NCAV quintiles. Both runs are compared with IWM and SPY, alongside capacity under the ADV cap.
- Defensive: the PR shows passers per year and returns against a book-to-market decile and SPY, over the years where all seven criteria are computable.
- 1976: the PR compares returns with Graham's stated 15%-a-year expectation and the AAII test, and states that the periods don't overlap.
