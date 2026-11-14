# singleton: strong variant

Complex · Gaps: G24

`bot.py` is Singleton v1, built from what the store has: 10-K XBRL, Form 4 and FRED. It buys companies that shrank their share count at least 4% a year over two fiscal years, from repurchases covered by free cash flow, spent at a dollar-weighted earnings yield at least the larger of the 10-year Treasury and 1/15, with no net open-market selling by officers and directors. It holds them for years, adds in lumps on 20% drops, and sells only on the company's own behaviour: issuing shares, buying back below the 10-year yield, restating, or insiders selling 1% of the company.

This brief is what is left: the two things that make "bought cheap" measured instead of inferred.

## Sources
- Cooperman's Teledyne exhibits, reprinted in the [csinvesting case study](https://oldschoolvalue.s3.amazonaws.com/pdf/Singleton-Teledyne-case-study.pdf): eight self-tenders from 1972 to 1984 at 22% to 47% premiums, and "I won't pay 15 times earnings" (Forbes 1978, Exhibit 11).
- Ben-Rephael, Oded & Wohl (2014): firms buy below the average market price, and a portfolio built on actual quarterly repurchases earns about 5.1% a year.
- Bonaime & Ryngaert (2013); Babenko et al. (2012): buybacks with insider net buying beat those with net selling.

## What v1 approximates
- **Price paid.** v1 values each fiscal year's repurchases at that year's average price, since 10-Q cash flows are year-to-date and the spine keeps only 3-month and 12-month durations. A company that bought heavily in a cheap quarter and stopped in a dear one looks the same as one that bought evenly.
- **Self-tenders.** v1 cannot see a tender offer, Singleton's own instrument, at all.

## Logic once G24 lands
- **Timing skill**: from the monthly *Issuer Purchases of Equity Securities* table, the average price paid against the average close of each month, dollar-weighted over the last 8 quarters; qualify only below the market's average price, and value each month's purchases at its own earnings yield.
- **Tender boost**: an SC TO-I in the last 12 months (not amended away) ranks first among qualifiers and opens at the ladder's cap rather than its start.
- **Exit**: buying back at an earnings yield below the 10-year Treasury in the latest two quarters, read monthly rather than per fiscal year.

## Gaps
- G24: the issuer-purchases table needs a `capital_returns` recipe over 10-K Item 5 and 10-Q Part II Item 2; tenders need SC TO-I and SC TO-I/A in the kept forms. Both are new data, so the owner decides.

## Done when
The strong variant runs beside v1 on the same window, and an event study compares quarters of heavy repurchases bought below the month's average price against those bought above it.
