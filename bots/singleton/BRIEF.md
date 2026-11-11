# singleton

Complex · Gaps: G2, G16; G10 (long/short variant)

## Idea
Henry Singleton's Teledyne playbook: shrink the share count hard when the stock is cheap. Per Thorndike, from 1972 to 1984 Singleton made eight tender offers and retired about 90% of Teledyne's shares, mostly when the P/E was near its lows (often below 10). He funded them with cash flow and, at times, debt. Teledyne paid no cash dividend in those years.

The research version of this is the buyback anomaly. Repurchasing firms earn a multi-year drift, and almost all of it comes from undervalued (high book-to-market) repurchasers. Ikenberry, Lakonishok & Vermaelen (1995) measured +45.3% 4-year abnormal buy-and-hold returns for value-quintile repurchasers against nothing for glamour repurchasers. So the bot goes long the firms with the largest actual share-count reductions among the cheapest by book-to-market, and holds each for 48 months. Teledyne's lesson is retired shares, not announced programs, and the issuance literature (Pontiff & Woodgate; Fama & French; Daniel & Titman) measures exactly that. The research variant also shorts the heaviest issuers.

The drift has shrunk sharply for US buybacks announced after 2001 (Lee, Park & Pearson 2019; Fu & Huang 2016). The bot keeps the published rules regardless, and the PR measures the decay.

This is the faithful version of the `cannibal` example bot, which is a simple SDK demo.

## Sources
- Thorndike (2012), *The Outsiders*, ch. 2 (Singleton): tender offers at low P/Es, about 90% of shares retired from 1972 to 1984.
- Ikenberry, Lakonishok & Vermaelen (1995), "Market Underreaction to Open Market Share Repurchases", *JFE* 39: 1980–1990, 48-month buy-and-hold returns, book-to-market quintiles.
- Peyer & Vermaelen (2009), "The Nature and Persistence of Buyback Anomalies", *RFS* 22: the anomaly persists in 1991–2001 as a response to overreaction to bad news. Their U-index combines prior 6-month return, size and book-to-market.
- Pontiff & Woodgate (2008), "Share Issuance and Cross-Sectional Returns", *JF* 63: annual log change in split-adjusted shares outstanding.
- Fama & French (2008), "Dissecting Anomalies", *JF* 63: net stock issues = log change in split-adjusted shares from fiscal year-end t−2 to t−1, pervasive across size groups.
- Daniel & Titman (2006), "Market Reactions to Tangible and Intangible Information", *JF* 61: 5-year composite issuance.
- Decay: Fu & Huang (2016), *Management Science* 62(4); Lee, Park & Pearson (2019), "Repurchases after Being Well Known as Good News". Outside the US: Manconi, Peyer & Vermaelen (2019), *JFQA*.

## Manifest
```toml
description = "Singleton cannibals: the largest share-count shrinkers among value stocks, held 48 months"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 3000 }
params = { issuance_bottom = 0.10, value_top = 0.20, hold_months = 48, undervaluation = "book_to_market", long_short = false, short_issuance_top = 0.10 }
```

## Logic
- Net share issuance (NSI) = ln(split-adjusted `SharesOutstanding` at the latest fiscal year-end / at the prior fiscal year-end), from annual (FY) facts.
  - Use period-end shares outstanding, never weighted-average or diluted share counts.
  - Split-adjust with the cumulative split factor between the two `period_end` dates and now. The factor is the ratio of `adjust = "split"` to raw closes.
- Book-to-market = book equity / market cap.
  - Book equity = `Equity` − preferred stock (G16).
  - Market cap = split-adjusted `SharesOutstanding` × close, summing every share class.
- On the first session of each month, rank the universe on the latest visible annual data. A name qualifies when:
  - its NSI is negative and in the universe's bottom `issuance_bottom` (the heaviest shrinkers, Pontiff–Woodgate deciles);
  - its book-to-market is in the top `value_top` (Ikenberry–Lakonishok–Vermaelen's value quintile).
- `undervaluation = "u_index"` replaces the book-to-market test with Peyer & Vermaelen's U-index (quintile scores for low prior 6-month return, small size and high book-to-market). The agent must take the exact scoring from the paper.
- Hold each qualifier for `hold_months` (48, the horizon of both event studies). Re-qualifying restarts its clock. There is no other exit, as in the papers.
- Hold every current name at equal weight, rebalanced monthly: the calendar-time portfolio the papers use.
- Reason: "shares {x}%/yr, B/M pct {b}, buyback/FCF {f}". Buyback/FCF = `ShareRepurchases` / (`OperatingCashFlow` − `Capex`) is reported only. Singleton funded tenders with debt too, so it is not a filter.
- Long/short variant: short the top `short_issuance_top` of Daniel–Titman composite issuance ι = ln(ME_t / ME_t−5y) − r(t−5y, t), where r is the log total return from `adjust = "all"` history. Same monthly dates, equal weight, dollar-neutral to the long book.
- Pitfalls:
  - Stock-financed mergers and stock-based compensation raise the count. They count as issuance, as in the papers.
  - The cover-page share date is not `period_end`. Use one consistent date.
  - A missing count (extension tag) disqualifies the name.
  - Annual comparatives start about 2009/2011, so the first valid signal is about 2010 for large filers and 2012 for all filers.
  - Delisted holdings liquidate at their last close.

## Gaps
- G2: stocks only.
- G16: book equity deducts preferred stock, as the papers' Compustat book equity does.
- G10: only the long/short variant needs per-name borrow.

## Done when
- The PR verifies the share-count arithmetic across a split on a known repurchaser, such as AAPL's 4:1 split in August 2020.
- The PR reports the long book's calendar-time alpha on the Fama–French five factors plus momentum. It compares that with Ikenberry–Lakonishok–Vermaelen and Peyer–Vermaelen, and with the post-2001 decay in Lee–Park–Pearson. A small alpha is a faithful result, not a bug.
- The long/short spread is correlated, over overlapping months, with the published Open Source Asset Pricing (Chen & Zimmermann) `ShareIss1Y` and `CompEquIss` portfolios.
- The PR compares results with the simple `cannibal` example bot.
