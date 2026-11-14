# munger

Complex · Gaps: G2, G3, G16 (closed); banks left out (a stated infidelity)

Charlie Munger's way of investing as he described and practised it: a few wonderful businesses bought at fair prices, cash kept until the price is fair or the market panics, and then nothing done for years. Munger published no formula, so the selection is Gray & Carlisle's franchise power and EBIT/TEV; the behaviour is the persona layer `buizz_bots.persona`. Implemented in `bot.py`.

## Sources
- Buffett, [1989 letter](https://www.berkshirehathaway.com/letters/1989.html): "a wonderful company at a fair price", which he credits to Munger.
- Munger, [A Lesson on Elementary, Worldly Wisdom](https://fs.blog/great-talks/a-lesson-on-worldly-wisdom/) (1994): the 20-slot punch card, "load up on the very few good insights", "sit back and wait"; a business earning 18% on capital beats one earning 6% bought cheap.
- Buffett, [The Superinvestors of Graham-and-Doddsville](https://www.lapasserelle.com/courses/advanced_finance/documents/TheSuperinvestorsofGraham-and-Doddsville-WarrenE.Buffett.pdf), Table 5: Wheeler, Munger & Co. 19.8% a year 1962 to 1975, −31.9% in 1973 and −31.5% in 1974, "concentrated in very few securities".
- Daily Journal [10-K for fiscal 2009](https://www.sec.gov/Archives/edgar/data/0000783412/000119312509253186/d10k.htm): about $15.5M of T-bills moved into Wells Fargo, BofA, US Bancorp and Posco in the six months to March 2009.
- Gray & Carlisle (2012), *Quantitative Value*, ch. 5 to 7: the 8-year franchise power and EBIT/TEV.

## Rules
- **Universe:** the 1,000 stocks with the highest dollar volume, less financials (SIC 6000 to 6999) and utilities (4900 to 4949) (`buizz_bots.value.operating_companies`).
- **Wonderful:** franchise power in the top 10% (`quality_top`): the average universe percentile of the 8-year geometric-mean ROA and return on capital, 8 years of free cash flow over current assets, and the larger of gross-margin growth and stability. EBIT is operating income, or pretax income plus interest expense for filers that report only pretax lines (`buizz_bots.value.ebit`).
- **Fair price:** EBIT/TEV at or above the universe median (`value_floor`). TEV is market cap plus total debt and preferred stock, minus cash and short-term investments (`buizz_bots.value.net_debt`).
- **Candour:** no 8-K Item 4.02 restatement in 5 years (`restatement_years`).
- **When it looks:** once a month, and every session of a panic, only while it has room: a free slot (at most 4 names, `max_names`) and 20% of equity not in stocks, or a holding the ladder may add to.
- **Market:** no new name while the market temperature (`buizz_bots.persona.temperature`) is at least 0.80 (`hot`). In a panic, the S&P 500 at least 20% below its high (`panic_drawdown`) or the temperature at most 0.20 (`cold`), the price bar drops to the 25th percentile of EBIT/TEV (`panic_value_floor`): pay up for quality when others are scared.
- **Size ladder:** a new name opens at 20% of equity (`start`); 5 points (`add_step`) are added each time it closes 20% below its average cost (`add_on_drop`) while its thesis still holds, up to 40% (`cap`).
- **Selling:** in full and at once, only on
  - a thesis break: on a new 10-K or 10-Q, return on capital below zero in the latest fiscal year or franchise power below the top 30% (`sell_quality_floor`); or an 8-K Item 4.02 restatement;
  - an absurd price: on a new filing, EBIT/TEV in the universe's bottom 10% (`sell_value_floor`).
  Never on a price fall, and nothing forces a free slot to be filled.
- **Cash:** everything not in stocks is held in BIL (`cash_symbol`), sold to fund buys.
- **Risk:** no drawdown stop. Wheeler, Munger lost about 53% in 1973 and 1974; a 50% drawdown is part of the persona.

## What can and cannot be copied honestly
- **Copied:** few names, quality first and price second, cash kept until the price is fair, cash spent in a panic, adding into weakness, holding for years, selling only when the business breaks.
- **Proxies, named as such:** "wonderful" is the 8-year record of returns on capital and margins, never a forecast; Munger's judgement of management becomes candour (no 4.02 restatement); his circle of competence is not modelled.
- **Left out:** judgement of people, private deals, the BYD-style insight that a record cannot show, and banks. His 2009 buys were banks, but return on capital and EBIT/TEV mean nothing for them, so financials stay excluded.
- **History:** XBRL starts in 2009, so an 8-year record first exists for fiscal 2016. The honest window is 2017 to 2026. With 3 to 5 names and about 15 decisions in that window, the return cannot tell skill from luck; the behaviour fingerprint can be checked.

## Expected fingerprint
| Metric | Target |
|---|---|
| Positions | 2 to 5 (Daily Journal 3 to 4) |
| Top-5 share | 100% of the stock book (never more than 5 names) |
| Turnover | under 15% a year |
| Holding period | median at least 36 months |
| Stock share | often well below 100%: four full slots start at 80%, more only as winners grow or the ladder adds |
| Sells on price | 0% |
| Drawdown response | net buyer from cash in March 2020 and in 2022 |
