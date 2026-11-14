# buffett

Complex · `owner` (default) and `systematic` (long-only) are implemented; the long/short `systematic` book is blocked by G10

Two variants. `owner` behaves like Buffett buying public stocks: few businesses with long records, bought at a sensible owner-earnings yield, added to when the price falls, held until the business changes. `systematic` (`systematic.py`) is Frazzini, Kabiller & Pedersen's factor replication, kept as the control: it holds Berkshire's factor tilts, not its behaviour.

## Sources
- Berkshire Hathaway letters: [1986](https://www.berkshirehathaway.com/letters/1986.html) (owner earnings), [1988](https://www.berkshirehathaway.com/letters/1988.html) and [1989](https://www.berkshirehathaway.com/letters/1989.html) (Coca-Cola bought and added to), [2007](https://www.berkshirehathaway.com/letters/2007ltr.pdf) (See's, capital-light businesses), [2014](https://www.berkshirehathaway.com/letters/2014ltr.pdf) (acquisition criteria; Tesco and the cockroach rule), [2024](https://www.berkshirehathaway.com/letters/2024ltr.pdf) (delaying the correction of mistakes).
- Martin & Puthenpurackal, Berkshire's 13F 1976 to 2006: median 27 stocks, top five 73% of the book, median turnover 20% a year, median holding 18 months.
- Frazzini, Kabiller & Pedersen (2018), "Buffett's Alpha", *FAJ* 74(4); NBER w19681 §7, Appendix C, Tables 2, 4 and 5 (`systematic`).
- Asness, Frazzini & Pedersen (2019), "Quality Minus Junk"; Frazzini & Pedersen (2014), "Betting Against Beta".

## `owner` rules
- **Universe:** the `owner_universe` (1,000) stocks with the most dollar volume, re-read every week. Banks and insurers (SIC 6000 to 6999) are left out: owner earnings do not describe them, and that leaves out Berkshire's largest public holdings in banks, American Express and insurers.
- **Qualify, only when a new 10-K or 10-Q arrives** (`buizz_bots.persona.new_filings`), on the last `record_years` (8) consecutive 10-K years:
  - operating income and owner earnings positive every year. Owner earnings are net income + D&A − all capex, because maintenance capex is not reported;
  - mean return on equity at least `min_roe` (15%), equity positive every year;
  - total debt at most `max_debt_cover` (3) times the latest owner earnings;
  - capex at most `max_capex_share` (50%) of net income plus D&A over the record, the See's test;
  - pre-tax income at least `min_pretax_2014` ($75M) in 2014 dollars, scaled by CPI-U;
  - predictability: the coefficient of variation of operating income ÷ (equity + debt) in the steadier half of the universe's records;
  - capital allocation: owner earnings grew by at least `min_yield` per retained dollar (net income less dividends and buybacks) over the record, so a retained dollar is worth at least a dollar at the price Buffett pays;
  - candour: no 8-K Item 4.02 restatement in five years;
  - alignment: officers and directors sold, net of open-market purchases and outside 10b5-1 plans, at most `max_insider_sale` (20%) of their stake in the last year.
- **Price gate:** owner-earnings yield (latest owner earnings ÷ split-adjusted market cap) at least max(`min_yield` 6%, 10-year Treasury + `yield_spread` 2pt). Candidates rank by yield × ROE percentile among the qualified; a second share class of a held company is skipped.
- **Portfolio:** at most `max_names` (12). A new name opens at `start_weight` (8%) of equity, only with free money. A holding gets `add_weight` (4%) more each time it closes `add_on_drop` (15%) below its average cost while it still qualifies and passes the gate, up to `cap` (25%). Nothing is trimmed when it grows. The rest of equity sits in BIL; nothing forces a slot to be filled.
- **Sell, in full and at once:**
  - thesis: a new filing fails the qualification, or an 8-K 4.02 restatement arrives (the cockroach rule);
  - valuation: owner-earnings yield below `absurd_ratio` (0.5) × the 10-year Treasury, the PetroChina sale at estimated value;
  - swap: when no money or slot is free, a candidate yielding `swap_ratio` (2) times the weakest holding replaces it, at most one a day;
  - never because the price fell. Order reasons start with `thesis broke:`, `valuation:` or `better use:`, so the fingerprint counts no sell on price.

## What it cannot copy honestly
- **Judgement of people.** No data states character. The capital-allocation, candour and alignment tests above are proxies for it and are named as such.
- **Circle of competence.** Steady returns on capital stand in for "a simple business". There is no industry list; whatever qualifies is bought, and much of what Buffett bought (Apple in 2016) or avoided is decided by the record alone.
- **The forward view of a wonderful business.** Only the record is read: eight years, so 2017 is the first year any name qualifies (XBRL starts in 2009).
- **Maintenance capex.** All capex is subtracted, which understates owner earnings of growing companies.
- **Banks and insurers**, Berkshire's largest public holdings for most of 2016 to 2025, are left out until a qualification for them exists. This is the largest infidelity.
- **Private businesses, negotiated preferreds and cheap float leverage.** Left out: the comparison is Berkshire's 13F public portfolio, not BRK stock, and the bot is not levered.
- **Size.** Berkshire's size forced it into mega caps and kept it from small ones; the bot has no such constraint beyond the $75M pre-tax floor.

## Expected fingerprint against Berkshire's 13F, 2017 to 2025
| Metric | Berkshire 13F 2016 to 2025 | Expected `owner` band |
|---|---|---|
| Positions, median | 45.5 (share classes apart) | 8 to 12; fewer in 2017 while few eight-year records exist |
| Top-5 share | 71% | 45% to 65%: equal 8% entries give 42%, adds and winners raise it |
| Turnover | 10% a year | 10% to 25% a year |
| Holding period, median | 30 months | 24 to 48 months, capped by the window |
| Buys into weakness | 22% of buy dollars | 25% to 45%: adds only on 15% drops, entries when yields rise |
| Sells on price | — | 0% |
| 2022 bear market, net buying | +10% of book | +5% to +20% of equity |
| Stock share | — | 60% to 95%, the rest in BIL |

## Done when
- The `owner` backtest's fingerprint lands in the band above, beside Berkshire's.
- Its return is reported against `greenblatt` and `systematic` on the same window: if the persona rules cannot beat the published magic formula, they add nothing.

## Remaining: the `systematic` long/short book
- Params to add: `beta = 0.68` (β_B, Table 2), `idio_vol = 0.224` (σ_I, Table 2), and a `systematic_long_short` variant.
- At each month end, the active book a = Σ loading_f × w_f (`active_book`), scaled by c = σ_I / σ̂(a) from the last 252 days of total returns; targets are β_B on MKT plus c × a, netted per stock, at about 3.7× gross (Table 2), financed at the T-bill rate plus the spread.
- A `realtime` version (Appendix C) re-estimates β_B, the loadings and σ_I from BRK-B on factor returns up to t, with at least 60 months; it needs factor return history before the run (G12, G14 or published factor series).
- G10: every factor leg shorts, including small, high-beta and junk names; per-name borrow is recorded but not exposed to runs.
- Done when a monthly regression on the six factors recovers market beta ≈ 0.68 and the loadings in the target proportions, and the correlation with BRK-B is reported against FKP Table 5's 0.47 (0.73 for the 13F portfolio).
