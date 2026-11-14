# greenblatt

Medium · Gaps: none (G11 is a caveat)

The magic formula as Greenblatt publishes it, implemented in `bot.py`. It is the control for `munger` and `buffett`: if their persona rules cannot beat the published formula on the same data, they add nothing.

## Sources
- Greenblatt (2006, updated 2010), *The Little Book That Beats the Market*: the step-by-step instructions and the appendix that defines both ratios.
- [magicformulainvesting.com, How it works](https://www.magicformulainvesting.com/Home/HowItWorks): at least 20 stocks, buying in several lots over 12 months, holding about a year, losses sold before one year and gains after it.
- Schwartz & Hanauer, [Formula Investing](https://papers.ssrn.com/sol3/papers.cfm?abstract_id=5043197): weaker after 2000 but still positive in 40-name portfolios for 2000 to 2022, with big relative drawdowns in 2018 to 2020.
- Gray & Carlisle (2012), *Quantitative Value*, and [greenbackd](https://greenbackd.com/2013/03/12/how-to-beat-the-little-book-that-beats-the-market-redux-part-3): their 8-year "Quality and Price" beat the formula for 1964 to 2011. `munger` is built on it.

## Rules
- **Universe:** the 3,000 stocks with the highest dollar volume (the book ranks the 3,500 largest), at least $50 million market cap (`min_market_cap`).
- **Exclusions:** financials (SIC 6000 to 6999) and utilities (SIC 4900 to 4949). Foreign companies drop out because only 10-K and 10-Q filers in US dollars are read. Two share classes of one company count once (`buizz_bots.caps.market_caps`).
- **Return on capital** = EBIT ÷ (net working capital + net fixed assets):
  - EBIT is operating income over the last four reported quarters (the fourth quarter is the fiscal year minus its first three);
  - net working capital is current assets less cash and short-term investments, minus current liabilities less short-term debt;
  - net fixed assets is net PP&E, from the same latest balance sheet, at most 200 days old.
- **Earnings yield** = EBIT ÷ enterprise value. Enterprise value is market cap (split-adjusted latest share count × close) plus total debt and preferred stock, minus cash and short-term investments.
- **Rank:** each company's percentile on both ratios, summed. This orders the same as Greenblatt's sum of ranks.
- **Building the book:** 7 top-ranked names (`tranche_size`) every 3 months (`tranche_months`), until 28 are held after a year. Each buy is 1/28 of equity, funded from cash; the book is never rebalanced.
- **Selling, the book's tax timing:** a lot below its cost is sold once held 358 days (`loser_days`), so the loss stays short-term. Every other lot is sold once held 367 days (`winner_days`), so the gain becomes long-term. Each sale is replaced at once by the best-ranked name not held and not just sold.
- **Corporate actions:** a lot whose position vanished (acquired, delisted) is replaced. A lot's age is its position's `opened_at`, which follows a rename or a stock merger, so a renamed lot keeps its date.

## Departures from the book
- Greenblatt does not define "excess cash". Here all cash and short-term investments count as excess: they leave working capital and are netted against debt.
- EBIT is reported operating income, so non-operating gains and losses are left out.
- The engine does not model taxes. The sell timing is kept for fidelity, but it only pays after tax.

## Caveats
- G11: the universe is a dollar-volume ranking, not a point-in-time list of the largest 3,500 companies.
- EDGAR XBRL starts in 2009 to 2011, so the 10-year preset has full data from its first day.
