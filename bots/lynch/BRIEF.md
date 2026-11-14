# lynch

Complex · Gaps: none (the story, store visits and scuttlebutt are a stated infidelity)

Peter Lynch's method as *One Up on Wall Street* and *Beating the Street* describe it: sort each company into one of six categories, buy it at the price its category calls for, hold many of them, and sell each for its own category's reason. Lynch gave no formula for the categories, so they are read from the XBRL record; the behaviour is `buizz_bots.persona`. Implemented in `bot.py`.

## Sources
- Lynch with Rothchild (1989), *One Up on Wall Street* ([Simon & Schuster](https://www.simonandschuster.com/books/One-Up-On-Wall-Street/Peter-Lynch/9780743200400)). Chapter 7, the six categories: slow growers grow "slightly faster than GNP" and pay a generous dividend, stalwarts 10% to 12% a year, bought for 30% to 50% and then swapped for another, fast growers 20% to 25%, cyclicals whose sales and profits rise and fall, turnarounds, and asset plays sitting on something the market has missed. Chapter 10: a P/E at half the growth rate is very positive and at twice it very negative, and the dividend-adjusted ratio (growth + yield) ÷ P/E is poor below 1, okay at 1.5 and what you want at 2. Chapter 13: debt-to-equity, where a normal balance sheet is 75% equity and 25% debt, and net cash. Chapter 17: when to sell each category, including cyclicals at a low P/E near peak earnings and turnarounds once they have turned and everybody knows it. **(recalled)**
- Lynch with Rothchild (1993), *Beating the Street* ([Simon & Schuster](https://www.simonandschuster.com/books/Beating-the-Street/Peter-Lynch/9780671891633)): Magellan's growth from 41 stocks in 1977 to about 900 by 1983 and up to 1,400 in 1989 ([summary, secondary](https://www.shortform.com/pdf/beating-the-street-pdf-peter-lynch)); rotating among stalwarts; adding to a good company when its price falls. **(recalled)**
- AAII Journal, [Blending Conservative, Growth and Cyclical Stocks Into a Winning Strategy](https://www.aaii.com/journal/article/10415-blending-conservative-growth-and-cyclical-stocks-into-a-winning-strategy) (interview with Lynch, September 1985): about 30% in "conservative" stocks (stalwarts), sold after a 30% to 35% gain and replaced; 25% to 40% in growth companies; 20% to 25% in cyclicals; no market timing; adding to growth and cyclical names in a decline when the fundamentals hold.
- Thorp, AAII Journal, [Peter Lynch and "Investing in What You Know"](https://www.aaii.com/journal/article/10631-peter-lynch-and-investing-in-what-you-know) (August 2019): AAII's Lynch screen, a dividend-adjusted PEG of at most 0.5 (a PEGY of at least 2 as Lynch writes it), 5-year EPS growth below 50%, financials left out; insider buying and buybacks as pluses; a price drop as a chance to buy more.
- Rabener, CFA Institute Enterprising Investor, [GARP Investing: Golden or Garbage?](https://rpc.cfainstitute.org/blogs/enterprising-investor/2019/garp-investing-golden-or-garbage) (March 2019): PEG below 1 beat the market from 1989, mostly in 1989 to 2001; from 2000 value did better and from 2010 the market did.
- Cohen, Malloy & Pomorski, [Decoding Inside Information](https://www.nber.org/digest/apr11/decoding-inside-information) (NBER w16454): opportunistic insider trades earn about 82 basis points a month value-weighted, routine ones about nothing.

## Rules
- **Universe:** the 3,000 stocks with the highest dollar volume, less financials (SIC 6000 to 6999) and utilities (4900 to 4949) (`buizz_bots.value.operating_companies`).
- **Record:** six consecutive 10-K fiscal years of diluted EPS (`record_years`), each split-adjusted from its filing date, so five years of growth. The trailing EPS is the sum of the latest four quarters (`buizz_bots.fundamentals.trailing_year`), and a year earlier the same sum four quarters back. The P/E is the close over trailing EPS. The record's normal P/E is the median of the split-adjusted close at each fiscal year end over that year's EPS. Market cap is `buizz_bots.caps.market_caps`.
- **Categories,** in this order, on the latest record:
  1. **cyclical:** revenue fell in at least two of the five years (`cyclical_falls`), once by 10% or more (`cyclical_drop`);
  2. **turnaround:** a loss in one of the last two fiscal years (`turnaround_years`);
  3. **asset play:** tangible book (equity less goodwill) at least the market cap;
  4. by the 5-year EPS CAGR, **fast grower** at 20% or more (`fast_growth`), **stalwart** at 10% to 20% (`stalwart_growth`), **slow grower** at 0% to 10%. A shrinking record is no category.
- **Buy, by category:**
  - growers: a PEGY = (EPS CAGR % + dividend yield %) ÷ P/E of at least 1.5 (`min_pegy`, Lynch's "okay"), CAGR below 50% (`max_growth`); slow growers must have paid a dividend in every year of the record;
  - cyclicals: trailing EPS above its level a year ago and at most half the record's peak EPS (`cyclical_trough`), so bought in the trough on the way up;
  - turnarounds: trailing EPS positive again;
  - asset plays: market cap at most 75% of tangible book (`asset_entry`; Lynch gives no number).
- **Balance sheet:** every buy needs total debt at most a third of equity (`max_debt_equity`, the 75/25 normal balance sheet) and no 8-K Item 4.02 restatement in five years (`restatement_years`).
- **Ranking:** within a category, names with a plus come first, then the category's score (PEGY; peak EPS over price for cyclicals and turnarounds; tangible book over market cap for asset plays). The pluses are opportunistic open-market buying by an officer or director in the last 365 days (`insider_days`), Cohen–Malloy–Pomorski's classification over three years (`routine_years`, `buizz_bots.insiders.clusters`), and net cash. The categories then take turns, so the book spreads across them as Magellan's did.
- **Size:** up to 100 names (`max_names`), each opened at 1% of equity (`start`), with 1 point more (`add_step`) when it closes 20% below its average cost (`add_on_drop`) and still qualifies in its category, up to 2% (`cap`). The buys and the full scan happen in the first session of each month; held names are rechecked on every new 10-K or 10-Q and every 8-K 4.02. BIL (`cash_symbol`) holds only what empty slots leave.
- **Sell,** in full, by the category a name was bought in, each reason led by its trigger:
  - `price:` stalwarts and slow growers at +50% on cost (`exit_gain`), checked every session, or at +30% (`stretched_gain`) once the P/E is above the record's normal P/E;
  - `valuation:` stalwarts and fast growers at a PEGY of 0.5 or less (`sell_pegy`, P/E twice the growth rate); cyclicals with trailing EPS at or above the record's peak and the P/E at or below its normal (low P/E at peak earnings); turnarounds once both of the last two fiscal years are profitable (turned); asset plays once the market cap reaches tangible book;
  - `thesis broke:` fast growers when trailing EPS grows less than 10% on a year ago (growth slowed); stalwarts when it shrinks; slow growers on a dividend cut of 10% or more (`dividend_cut`); turnarounds when trailing EPS is negative again; any name whose debt exceeds its equity (`break_debt_equity`) or that files an 8-K 4.02.
- **Risk:** no drawdown stop and no market timing.

## What can and cannot be copied honestly
- **Copied:** the six categories, the dividend-adjusted P/E-to-growth test, the balance-sheet preference, insider buying as a plus, many small positions spread across categories, adding on a drop, and a sell rule per category.
- **Proxies, named as such:** the growth rate is the trailing 5-year EPS CAGR, not Lynch's forward estimate; no analyst estimates exist here. Categories come from the record (revenue falls for cyclicals, losses for turnarounds, tangible book for asset plays), not from knowing the industry. Asset plays see only tangible book, never hidden real estate, tax-loss carryforwards or a subsidiary worth more than the parent.
- **Left out:** "know the story", the two-minute drill, store visits, scuttlebutt, talking to companies, and the cyclical signs a reader of the trade press sees (inventories and prices in the industry). Also left out: S&Ls and other financials, which Lynch owned in bulk; the 8% bond and 15% foreign sleeves of the 1985 interview; spin-offs and other special situations; and Magellan's 1,000-plus positions, which a $100,000 account cannot hold in whole shares.
- **History:** XBRL starts in 2009, so a six-year EPS record first exists for fiscal 2014 and the bot can trade from 2015. The trailing CAGR from a low base can rate a cyclical recovery as a fast grower; the category order puts revenue falls and recent losses first for that reason.

## Expected fingerprint
| Metric | Target |
|---|---|
| Positions | 60 to 100 (Magellan held hundreds; capped for whole shares) |
| Top-5 share | under 15% of the stock book |
| Turnover | 50% to 150% a year, well above Buffett's |
| Holding period | median 9 to 24 months |
| Stock share | above 90% once the book fills: no timing, BIL only for empty slots |
| Idle months | under 20% |
| Buys into weakness | 25% to 50% (cyclicals in the trough, turnarounds, adds on drops) |
| Sells on price | 15% to 40%, all from stalwarts and slow growers |
| Drawdown response | a small net buyer in March 2020 and 2022 (adds on drops), never a seller on price alone |
