# templeton

Complex · Gaps: none (the global search and the lockup-expiry short are stated infidelities)

Sir John Templeton's "maximum pessimism", US buy side only: buy solvent companies after the crowd has given up on them, at a low price on normal earnings, hold many of them for about four years, and swap one only for a much better bargain. New money goes in when the market is cold; otherwise it waits in T-bills. The behaviour is `buizz_bots.persona`. Implemented in `bot.py`.

## Sources
- Templeton, *16 Rules for Investment Success* (1993, reprinted by Franklin Templeton): "buy at the point of maximum pessimism", "search for bargains", diversify ("in stocks and bonds, as in much else, there is safety in numbers"), buy value, not market trends. **(recalled)**
- The Motley Fool, [John Templeton](https://www.fool.com/investing/how-to-invest/famous-investors/john-templeton): in 1939 he borrowed to buy 100 shares of each of 104 companies selling at $1 or less, many of them in bankruptcy, and most were sold at a profit about four years later. **(secondary)**
- L. Templeton & S. Phillips (2008), *Investing the Templeton Way*, as summarised in [Investing the Templeton Way](https://financialpreparedness.substack.com/p/investing-the-templeton-way): buy where prices have fallen furthest, judge cheapness on normal earnings over several years rather than this year's, sell a holding only when a replacement is about 50% better a bargain, an average holding period of about four years. **(secondary)**
- De Bondt & Thaler (1985), "Does the Stock Market Overreact?", *Journal of Finance* 40(3): three-year losers beat three-year winners over the following three to five years. **(recalled)**
- `persona_bots.md` §10 (the research behind this bot) found no persona test of Templeton's rules.

## Rules
- **Universe:** the 3,000 stocks with the highest dollar volume, less financials (SIC 6000 to 6999) and utilities (4900 to 4949) (`buizz_bots.value.operating_companies`), at least $50 million market cap (`min_market_cap`) to buy. EV/EBIT means nothing for a bank, so financials are left out, although Templeton owned them.
- **Pessimism:** the split-adjusted close at least 60% below its highest close of the last three years (`min_fall`, `high_years`), with at least a year of history.
- **Normal earnings:** the mean EBIT of the last five consecutive 10-K fiscal years (`normal_years`; `buizz_bots.value.ebit`, operating income or pretax income plus interest), the latest at most 548 days old. Enterprise value is market cap (`buizz_bots.caps.market_caps`) plus net debt at the latest fiscal year end (`buizz_bots.value.net_debt`). A bargain is EV at most 8 times normal EBIT (`max_ev_ebit`, a proxy: Templeton gave no single number); the lowest multiple is bought first.
- **Solvent:** positive normal EBIT and equity, net debt at most 3 times normal EBIT (`max_debt_ebit`), and no 8-K Item 4.02 restatement in five years (`restatement_years`).
- **When:** the universe is scanned in the first session of each month, and in the first session of every week while the market temperature (`buizz_bots.persona.temperature`: forward P/E against its 10-year average, the Baa-10y spread, VIX and the S&P 500 drawdown, each a 10-year percentile) is 0.30 or colder (`cold`). New names are opened only while it is cold; held names are rechecked on every new 10-K or 10-Q and every 8-K 4.02.
- **Size:** up to 100 names (`max_names`), each opened at 1% of equity, no adds. Nothing forces a slot to be filled: what the stocks do not use sits in BIL (`cash_symbol`).
- **Swap,** at every scan, cold or not: the best bargain not bought replaces the holding with the lowest EBIT/EV when its EBIT/EV is at least 1.5 times that holding's (`swap_edge`, "50% better"), then the next pair, until one fails.
- **Sell,** in full, each reason led by its trigger:
  - `timer:` four years after the position opened (`hold_years`, `Position.opened_at`, which follows renames);
  - `swap:` the swap above;
  - `valuation:` EV at 15 times normal EBIT or more (`full_ev_ebit`, a proxy for "fully valued");
  - `thesis broke:` normal EBIT or equity no longer positive, net debt above 3 times normal EBIT, or an 8-K 4.02.
  - Never on the price alone, so sells on price should be zero.
- **Risk:** no drawdown stop; the stock book is never levered.

## What can and cannot be copied honestly
- **Copied:** buying what has fallen furthest, judging it on normal earnings, insisting on solvency, many small positions, about four-year holds, the 50%-better swap rule and cash that waits for a cold market.
- **Proxies, named as such:** "normal earnings" is a five-year mean of reported EBIT; "point of maximum pessimism" is a 60% fall plus a cold market gauge; the EV/EBIT thresholds are this bot's, not his.
- **Left out:** the global search across countries (Japan in the 1960s, Korea in 1997): US stocks only. The 2000 short of tech stocks before their IPO lockups expired: it needs IPO dates and borrow history, which do not exist here before late 2026. Also left out: his 1939 purchases of bankrupt companies (this bot insists on solvency), bonds and judging management.
- **State:** the filing date seen for each holding, the last 8-K check and the week and month scanned, about 3 KB at 100 names whatever the universe.
- **History:** a five-year EBIT record first exists for fiscal 2013, but prices before 2016 cover only 91 symbols, so the 3-year high is short until 2019; 2016 to 2026 holds three or four cold spells (early 2016, late 2018, March 2020, 2022), and the bot makes most of its buys in them.

## Expected fingerprint
| Metric | Target |
|---|---|
| Positions | median 20 to 60, from 0 before the first cold spell up to 100 |
| Top-5 share | under 20% of the stock book once 30 names are held |
| Turnover | 10% to 40% a year |
| Holding period | median 18 to 48 months, none above 49 |
| Stock share | 20% to 60% on average: near 0 before the first cold spell, rising in each |
| Idle months | 50% to 80%: buys cluster in cold weeks |
| Buys into weakness | above 90% (every open is 60% below its 3-year high) |
| Sells on price | 0% |
| Drawdown response | a clear net buyer in March 2020 and in 2022 |
| Characteristics | high book-to-market, small size, beta above 1, low ROE |
