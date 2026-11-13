# Buizz: where an edge could come from

A brainstorm on four questions: better bots, a better data spine, an AI agent inside a bot, and where to take the project. It rests on a read of the repo and on three web research passes done on 2026-11-13 (surviving edges, free data sources, LLM evidence). Claims carry a link where one was checked; the last section lists what could not be verified.

## The short version

1. **The bots lack edge because of what they bet on, not how well they are coded.** 30 of the 47 bots trade only SPY or a fixed handful of ETFs, nearly all on a published timing or rotation rule. That is the most crowded bet there is, and it makes too few independent decisions to ever be told apart from luck.
2. **Stop trying to replace SPY. Hold SPY and stack a market-neutral stock overlay on top.** This is the only shape where the arithmetic works: combined Sharpe = √(SPY Sharpe² + overlay Sharpe²). With SPY at 0.74 on the 10-year preset, an overlay at 0.6 gives 0.95.
3. **The overlay should be many small bets in small and mid caps, driven by events and text**: what a company just said, who just bought, who is short. That is where evidence of surviving edge is, and where big funds cannot follow.
4. **The data spine already holds most of what is needed, unused.** No bot reads news or earnings-release times, the borrow file is recorded but not readable, and only one 8-K item is kept. Add free short-interest feeds and filing text. One paid source (Sharadar, $69 a month) would fix the biggest structural gap: delisted companies and history before 2016.
5. **Yes to an AI agent, but as a reader in the spine, never as a trader in the bot.** It turns filings and news into stored, timestamped facts. Every rigorous test of LLMs that pick trades directly is null or negative, and any backtest of a current model on dates before its training cutoff is contaminated.
6. **Be honest about the prize.** A realistic overlay adds 0.1 to 0.2 Sharpe. The valuable thing Buizz can become is a machine that tells a real small edge from luck, and then stacks several.

## Why the current bots feel edgeless

**Low breadth.** Skill shows up as information ratio ≈ skill per bet × √(independent bets per year) ([Clarke, de Silva & Thorley 2002](https://ideas.repec.org/a/taf/ufajxx/v58y2002i5p48-66.html)). A rule that switches SPY on and off makes a few bets a year. An event signal across 2,000 stocks makes hundreds. The timing bots could be right and Buizz still could not prove it.

**The measurement cannot see small edges at index level.** Over ten years the standard error of a Sharpe difference against SPY, at 0.9 correlation, is about 0.19, and the best of 45 zero-skill trials is expected to show about +0.4 by luck (research pass, computed from the Bailey–López de Prado formulas). The +0.1 of `breadth_regime` is inside that noise. Telling a true +0.3 from luck this way takes about 19 years.

**Everyone has this information, processed this way.** Prices and reported numbers, sorted monthly, in large caps. The record since 2016 is clear:

| Rule family (Buizz has bots for each) | What the evidence says now |
|---|---|
| Turn of month | SPY earned 5.3 bps a day in the window against 6.3 outside it in 2016–2025 (computed). Gone |
| Pre-FOMC drift | Essentially gone after 2015 ([Kurov, Wolfe & Gilbert 2021](https://ideas.repec.org/a/eee/finlet/v40y2021ics1544612320315956.html)) |
| Overnight drift | 12.0% overnight against 4.7% intraday a year in 2016–2021, then 5.4% against 5.4% in 2022–2025 (computed); the overnight ETFs [closed in 2023](https://www.bloomberg.com/news/articles/2023-07-19/-night-effect-funds-to-shut-down-with-overnight-returns-elusive) |
| Volatility targeting | Real-time versions are worse for 72 of 103 strategies ([Cederburg et al. 2020](https://econpapers.repec.org/RePEc:eee:jfinec:v:138:y:2020:i:1:p:95-117)) |
| Large-cap factor tilts | Of 94 characteristics only 2 still predict outside microcaps after 2003 ([Green, Hand & Zhang 2017](https://econpapers.repec.org/RePEc:oup:rfinst:v:30:y:2017:i:12:p:4389-4436.)). MTUM, QUAL, USMV, VLUE and SIZE all had a lower Sharpe than SPY in 2016–2025 (computed) |
| Earnings drift on reported numbers (SUE) | Gone outside microcaps since about 2006 ([Martineau 2022](https://nowpublishers.com/article/Details/CFR-0122)) |

**One signal per bot, equal weight, no hedge.** Published single anomalies net about 4 bps a month after costs and decay; combinations net about 20 ([Chen & Velikov 2023](https://www.cambridge.org/core/journals/journal-of-financial-and-quantitative-analysis/article/zeroing-in-on-the-expected-returns-of-anomalies/945133D5A3ECEEAF466AEE91551FD225)). The combination is the product, and Buizz has no bot that combines.

**The unused inventory.** `ctx.news` and `ctx.earnings` are read by no bot, `ctx.insider_trades` by one. LightGBM is installed and unused. The borrow history is recorded and not exposed.

## 1. How I would vastly improve the bots

### Four structural changes, before any new signal

| Change | What it means | Why |
|---|---|---|
| **SPY core plus overlay, as new bots** | The existing bots stay as they are, so each approach keeps its own test. A stock signal gets a new overlay bot beside it, sharing the signal code: it holds 100% SPY and adds a dollar-neutral long/short sleeve on top, as `core_overlay` already does with its hedge leg, and is scored on the sleeve's hedged return. `insider_overlay` is the first, beside `insider_cluster` | Market beta is free, so no research should be spent on it. An overlay at Sharpe 0.4 lifts the book from 0.74 to 0.84, at 0.6 to 0.95, at 0.8 to 1.09. Two independent overlays at 0.4 each give 0.93 |
| **Signals separate from portfolios** | A shared signal library returns scores; one portfolio builder turns any mix of scores into a size- and sector-hedged, liquidity-capped book with a no-trade band | Signals can then be combined, and turnover control is the most effective cost reducer ([Novy-Marx & Velikov 2016](https://nber.org/papers/w20721)) |
| **Move down in size** | Universe ranks about 300 to 3,000 by dollar volume, the tiniest left out, hedged with IWM rather than SPY | The edge sits where institutions cannot trade size ([Hou, Xue & Zhang 2020](https://www.nber.org/papers/w23394)). Unhedged small caps were a −6% a year headwind in 2016–2025 (IWM, computed), so the hedge is not optional |
| **Event study before bot** | A signal first has to show an abnormal return across thousands of dated events. Only then does it get a bot | An event test has statistical power in years, not decades. Illustration: a 3% drift over 60 days against a 25% spread per event reaches t = 3 at about 600 independent events |

### New bots, ranked by evidence

Each one leads with what data it needs and when that data became public, since that decides whether the result can be trusted.

| # | Bot | Idea | Data and point-in-time status | Evidence | Odds |
|---|---|---|---|---|---|
| 1 | **Text earnings drift** | The numbers surprise is priced within minutes; the words are not. Score what the release says (guidance, tone against last quarter) and hold the drift for one to four quarters | 8-K Item 2.02 with acceptance time to the second: in the store. The press release text (exhibit 99.1): free on EDGAR, not ingested | Text surprise earned 8.0% over a year against 4.6% for the numeric surprise, 2010–2019 ([Meursault et al. 2023](https://www.philadelphiafed.org/-/media/FRBP/Assets/working-papers/2021/wp21-07.pdf)). That study used call transcripts; using the release is our own hypothesis | Best single idea |
| 2 | **Insider clusters, refined** | Keep `insider_cluster`'s routine filter; drop purchases made right after an acquisition, use the 10b5-1 plan flag, add planned sales (Form 144) as a negative, run it in small caps and hedged | Form 4 in the store. The 10b5-1 flag is a field in the same XML since April 2023. Form 144 is structured since April 2023 | Purchases earned 3.4% over 60 days in 2003–2022, but 0.3% after an acquisition ([summary](https://clsbluesky.law.columbia.edu/2026/09/22/when-are-insider-purchases-credible-signals-of-private-information/)); opportunistic trades carry the signal ([Cohen, Malloy & Pomorski 2012](https://www.nber.org/papers/w16454)) | Good |
| 3 | **Avoid list** | Losers are more predictable than winners. Never be long a stock that is expensive to short, heavily shorted, issuing shares, restating (8-K Item 4.02) or that rewrote its 10-K. Used as a filter on every long book and tested alone as "small caps without the avoid list, against IWM" | Borrow: recorded daily, not exposed. Short interest: free from FINRA, not ingested. Net issuance: in the store. 8-K items and 10-K text: free, not ingested | Anomaly profits sit in the 12% of stock-dates with high borrow fees ([Muravyev, Pearson & Pollet 2025](https://experts.illinois.edu/en/publications/anomalies-and-their-short-sale-costs/)). 10-K changers lag by up to 188 bps a month, 1995–2014 ([Cohen, Malloy & Nguyen 2020](https://www.nber.org/papers/w25084)) | Good, and costs nothing to hold |
| 4 | **Composite ranker** | A slow rank average of 10 to 20 signals, hedged. A linear fit second. Trees only if they beat their linear twin, as `alpha_lab` already demands | Everything above plus what the store has | Combinations net about 20 bps a month. Heavy machine learning on standard characteristics fades outside microcaps ([Avramov, Cheng & Metzker 2023](https://papers.ssrn.com/abstract=3450322)) | Good as the house overlay |
| 5 | **Linked-company momentum** | News about a company's customers and close peers reaches its own price with a lag. Buy the companies whose customers and peers just rose | Major customers are named in the 10-K; peers come from the similarity of business descriptions. Both need filing text | Customer momentum above 150 bps a month in sample ([Cohen & Frazzini 2008](https://econpapers.repec.org/RePEc:bla:jfinan:v:63:y:2008:i:4:p:1977-2011)); text peers beat industry codes ([Hoberg & Phillips 2018](https://econpapers.repec.org/RePEc:cup:jfinqa:v:53:y:2018:i:06:p:2355-2388_00)). No check after publication found | Maybe |
| 6 | **Trend sleeve, stacked** | `trend_following` exists. Stack it on SPY instead of using trend to leave SPY | In the store | Positive in 8 of the 10 worst 60/40 drawdowns since 1880 ([Hurst, Ooi & Pedersen 2017](https://www.aqr.com/Insights/Research/Journal-Article/A-Century-of-Evidence-on-Trend-Following-Investing)); adds about 0.02 to 0.13 Sharpe | Small but real diversifier |
| 7 | **Pension rebalancing** | Large funds rebalance stocks against bonds on known dates and thresholds; prices move and revert within two weeks | In the store (SPY, bond ETFs, calendar) | [Harvey, Mazzoleni & Melone 2025](https://www.nber.org/papers/w33554). New, credible, small, in sample | One event study |

### What I would stop

Retire the SPY and ETF timing bots as research targets: no more sweeps on them. Keep `breadth_regime` running live because it is the standing test of whether its backtest holds, and keep `sixty_forty` as a baseline. Also not worth building, on the evidence:

| Idea | Why not |
|---|---|
| Index additions and deletions | The S&P 500 addition effect fell from 7.4% in the 1990s to 0.3% ([Greenwood & Sammon 2025](https://www.nber.org/papers/w30748)) |
| Spin-off and buyback baskets, 13F cloning | The live funds (CSD, PKW, GURU) had CAPM alphas of −6.3%, −2.4% and −3.9% a year in 2016–2025 (computed) |
| Short books in hard-to-borrow names | 0.15% a month before borrow fees, −0.02% after ([Muravyev et al.](https://experts.illinois.edu/en/publications/anomalies-and-their-short-sale-costs/)). The overlay's short leg should be IWM and liquid names |
| Intraday momentum, leveraged-ETF and option-flow effects | Cost-sensitive, shrinking, and the headline intraday result has no independent replication |
| Merger arbitrage | The funds returned 3 to 4% a year |

## 2. How I would improve the data spine

The spine's point-in-time discipline is the best part of Buizz. The gaps are in what it holds and in the research tools around it.

### Open what is already inside

| Item | Today | Change |
|---|---|---|
| 8-K item codes | The submissions API gives every item with its acceptance time; only 2.02 is kept | Keep all: 4.02 (restatement), 5.02 (officer leaves), 1.01, 2.05, 3.01 are each a dated event stream |
| Filing text | Not stored (gap G20) | The 99.1 exhibit of each earnings 8-K first, then 10-K and 10-Q sections, keyed by acceptance time |
| Borrow history | Recorded daily, no query | A bot query, so the avoid list can use it |
| Form 4 plan flag, Form 144, Schedule 13D/G | Not read | Parser fields and two more form types from the feed already in use; 13D/G is structured XML since December 2024 |

### Free feeds to add

| Feed | Content | History | When it was public | Catch |
|---|---|---|---|---|
| [FINRA short interest](https://www.finra.org/finra-data/browse-catalog/equity-short-interest/files) | Short position and days to cover per symbol | Files from settlement 2017-12-29 | Twice a month on a published schedule, about 7 business days after settlement | Files carry a revision flag, so snapshot them; old publication dates must be rebuilt from the schedule |
| [FINRA daily short volume](https://www.finra.org/finra-data/browse-catalog/short-sale-volume-data/daily-short-sale-volume-files) | Off-exchange short volume per symbol | From 2009-08-03 per facility | By 18:00 ET the same day | Mostly market-maker flow; a different thing from short interest |
| [SEC fails-to-deliver](https://www.sec.gov/data-research/sec-markets-data/fails-deliver-data) | Outstanding settlement fails per symbol | From 2004 | Twice a month, no stated date | Record the download date; give history a conservative lag |
| [CBOE index family and put/call](https://www.cboe.com/us/options/market_statistics/historical_data/) | SKEW, VVIX, VIX9D, implied correlation, put/call ratios | Mostly 2006 on | Daily close | Early rows of some indices were calculated after the fact; start each at its launch, as the store does for VIX |
| Wikipedia page views | Daily views per company article | From 2015-07-01 | Next morning | Needs a dated ticker-to-article map. It replaces the planned Google Trends recorder (G22): real history, stable values, no gated API |

### Recorders to start now

Some data has no free history at all, so it only gains value by being recorded, as the borrow file is today: daily option chain snapshots (Alpaca's free indicative feed), analyst consensus from any free API (no cheap vendor keeps as-of dates, so recording is the only way to true vintages), and ETF holdings and shares outstanding. Each day not recorded is lost.

### The one paid source worth it

[Sharadar's full-history bundle](https://sharadar.com/subscribe), $69 a month or $499 a year: prices and fundamentals for active and delisted US companies back to the 1990s, dated S&P 500 changes, 13F holdings by ticker. It would come in the way Tiingo does, as a second source trusted only where it agrees with the first.

- **Why:** the small-cap overlay needs delisted names, and today's history starts in 2016 inside one bull market. Twenty-five years adds 2000–2002 and 2008, and it eases the statistical power problem more than any signal can.
- **Against:** it breaks "free sources first", it is a vendor's standardised numbers rather than the filing, and the personal licence reportedly requires deleting the data after cancelling (not confirmed).
- **Free half-measure:** [Open Source Asset Pricing](https://www.openassetpricing.com/data/) publishes 212 predictors through December 2025. It cannot drive a bot, but it can pre-screen which signal families held up before 2016, before any Buizz trial is spent.

### Research tools the spine lacks

| Tool | What it does |
|---|---|
| Event study | Give it dated events; it returns abnormal returns against a size-matched benchmark as of each date, after costs, counted as a trial. Seconds instead of a ten-year backtest |
| Panel query | One call returns a features × symbols × dates panel as of each date. Any fitted model needs it; today a bot asks step by step |
| Hedged scoring | Next to "beats SPY", report the overlay's own Sharpe and its t-statistic after the hedge, and add IWM as a benchmark |
| Liquidity-aware costs | A flat 5 bps is generous for a small cap and harsh for SPY. Scale slippage by liquidity bucket, calibrated on quotes recorded at live decisions |
| Annotation store | The home for model-read facts, described in the next section |

A risk to watch: in May 2026 the SEC proposed optional semiannual reporting. No final rule was found. If adopted, quarterly fundamentals thin out and event, text and positioning data matter more.

## 3. An AI agent inside a bot

**Verdict: yes as a reader, no as a trader, and it belongs in the spine.**

### What the evidence says

| Use | Evidence | Verdict |
|---|---|---|
| LLM decides trades (FinMem, TradingAgents and similar) | Over 2004–2024 on a survivorship-free universe the reported results vanish: Sharpe −0.25 to 0.24 against 0.32 to 0.70 for buy and hold ([FINSABER](https://arxiv.org/abs/2505.07078)). Agent Sharpe ratios fall about half past the model's cutoff ([Profit Mirage](https://arxiv.org/abs/2510.07920)) | Discredited |
| LLM as fundamental analyst | The best-known paper ([Kim, Muhn & Nikolaev](https://arxiv.org/abs/2407.17866)) was withdrawn in February 2025 over data inconsistencies | Do not build on it |
| Headline sentiment, traded daily | Real after the cutoff but shrinking: Sharpe 6.5 in late 2021, 1.2 in early 2024, before costs; unprofitable at 20 bps a round trip; mostly small-cap shorts ([Lopez-Lira & Tang 2026](https://arxiv.org/abs/2304.07619)) | Too late, too costly |
| LLM extracts stated facts from filings | 119 event types across 293,000 8-Ks with quoted evidence, precision up to 96% ([Dolphin et al. 2026](https://arxiv.org/abs/2607.08346)) | Well supported |
| Text embeddings into a fitted model | Works in 16 markets; small dated models match large ones ([Chen, Kelly & Xiu](https://ssrn.com/abstract=4416687), [He et al. 2025](https://arxiv.org/abs/2502.21206)) | Well supported |

### The two problems that decide the design

**The model knows the future.** A current model recalls S&P 500 levels before its cutoff with 0.6% error and names the company in masked earnings calls; telling it to ignore the future does not work ([Lopez-Lira, Tang & Zhu 2025](https://arxiv.org/abs/2504.14765), [Sarkar & Vafa 2024](https://ssrn.com/abstract=4754678)). So a Haiku backtest over 2016–2025 proves nothing. `alpha_lab`'s brief already has the right rule: no language model whose training overlaps the test period.

**The model does not repeat itself.** Haiku 5.5 rejects sampling settings, so there is no temperature 0, and model versions retire. The only reproducible LLM output is a stored one.

### The design that fits Buizz

- **An ingest job, not a bot feature.** When a filing or article arrives, the spine sends it to the model once and stores the answer append-only with the model ID, prompt hash, document hash and time. Bots read it through a query as of the clock, like any other data. Bots keep no network access and no keys, and a backtest stays deterministic.
- **Visible when it existed.** An annotation made live is visible from the later of the document's time and its own. One made afterwards over old documents is flagged, exactly as `backfilled` news is today.
- **Two kinds of fields.** *Extraction* is what the document states, with the quoted sentence: guidance raised or cut, buyback size, why the CFO left, a named major customer. Hindsight barely changes it and the quote can be checked. *Judgment* is tone or expected direction: contaminated in any backtest, so its only evidence is forward.
- **Three honest ways to test a text signal.** In history: deterministic methods (word lists, similarity to the previous filing), or dated open models trained only up to each year ([ChronoBERT and ChronoGPT](https://huggingface.co/manelalab), yearly 1999–2024, public weights) run offline as embeddings. Forward: the live league, which is already how Buizz decides. A live variant pins its model; a new model is a new variant.
- **Untrusted input.** Filings and news can contain text aimed at the model. It gets no tools and may only fill a fixed schema; an answer that does not fit is stored as missing, never guessed. Company names are masked for judgment fields, which also removes a known distraction ([Glasserman & Lin 2023](https://arxiv.org/abs/2309.17322)).

### What the agent would do, in order of value

1. **Read earnings releases** for the text earnings drift bot: guidance direction, what changed against last quarter, one-off items.
2. **Tag every 8-K** by event type with its quote, which turns one filing stream into dozens of testable event streams.
3. **Read the 10-K** for named customers and for what changed since last year, feeding the linked-company bot and the avoid list.
4. **Answer "is there news behind this move?"** so a bot can treat a 10% jump on a restatement differently from one on nothing.
5. **Write the decision journal**: a plain account of why the book changed today. No effect on trades.
6. **Outside the bots, a larger model as research assistant**: it proposes hypotheses, writes the brief before the test and reviews results. LLMs can write a convincing theory for any mined pattern ([Novy-Marx & Velikov 2025](https://www.nber.org/papers/w33363)), so its stories count for nothing; the trial count does.

### Cost and what it needs

Haiku 5.5 is listed at $0.10 per million input tokens and $0.50 per million output (Anthropic price list, cached 2026-11-03), half that through the batch API. An earnings release is 3,000 to 10,000 tokens. Reading every release, 8-K and headline for the whole US market comes to tens of dollars a year. Cost is not the constraint; contamination and reproducibility are.

It needs one thing that is yours to approve: an API key in the spine's secret. It needs no new pod and costs nothing while idle.

## 4. Taking the project further

**Change the unit of research from the bot to the signal.** Signal, then event study, then composite, then one house book: SPY plus the risk-weighted overlays that are live. The league then ranks each sleeve by what it adds to the house book, which is the question that matters, rather than by its own Sharpe.

**Buy statistical power before buying ideas.** Longer history with delisted names, event-level tests and hedged scoring each shorten the time to tell real from luck. Without them a new idea is another coin flip on a ten-year window.

**Make the research loop a machine.** The gate, the trial count, the holdout and the league already exist. Add: the hypothesis and expected effect written down before the test (some `bot.toml` descriptions already do this), the trial ledger and deflated Sharpe from gap G23, a sealed period nobody tests on, and a standing 50% haircut on any backtest edge, the average decay after publication ([McLean & Pontiff 2016](https://ivey.uwo.ca/media/3775549/pontiff.pdf)). Then a nightly agent can propose and test within those rules without fooling anyone.

**Close the gap to real execution.** Paper fills are a model. Mirror the house book's orders to Alpaca's paper broker and compare fills, then run a small real account. That is the only out-of-sample test that includes true costs, and it is outside v1 today.

**The order I would work in:**

1. Rescore `insider_cluster` as a hedged overlay in small caps. It needs no new data and shows whether the overlay shape pays.
2. Build the event-study tool and the hedged scoring.
3. Ingest all 8-K items, the earnings release text, FINRA short interest, and open the borrow query.
4. Build the avoid list and the text earnings drift bot, text scored with deterministic methods first.
5. Add the annotation job and run the LLM variants forward beside their deterministic twins.
6. Combine what survives into the composite overlay and run it live as the house book.
7. Decide on Sharadar once step 1 shows whether small caps are where the edge is.

**What to expect.** Net long/short equity run by professionals delivers a Sharpe of about 0.3 to 0.6 (AQR's market-neutral fund: 0.53 in 2016–2025, with three losing years in a row; computed). An overlay in that range adds 0.1 to 0.2 to SPY's Sharpe. A backtest that shows much more is a bug or a leak until proven otherwise.

## What could not be verified

- No study after 2015 was found for: insider-signal decay, 10-K change effects, linked-company momentum, activist filings. Their odds above rest on older samples.
- That an LLM reading press releases improves earnings drift is untested in the literature; a 2025 workshop paper reports only 57.6% classification accuracy.
- Sharadar's licence terms and field definitions, and whether its fundamentals are as first reported, were read from its site and third parties, not from the contract.
- The Alpaca option feed's history (from February 2024) and whether it serves implied volatility; the stability of CBOE's undocumented daily statistics endpoint.
- Figures marked "computed" came from one research pass on public price data, not from a paper, and were not rerun in Buizz. Its SPY Sharpe (0.86, monthly data through 2025-12) differs from Buizz's own 0.74 for the 10-year preset; the arithmetic above uses Buizz's.
- The withdrawal of Kim, Muhn & Nikolaev and the Journal of Financial Economics publication of Lopez-Lira & Tang were read from author and journal pages by the research pass, not checked a second time.

## GAP from research in other session - use if needed

A second session's audit found these gaps. They were measured on a production copy on 2026-11-13. Most are data problems that would weaken the steps above.

**Gaps that would hit the first steps**
1. **insider_cluster is not "no new data needed".** Form 4 data covers about 70 companies so far (3,000 documents per 6-hour run). The ingest also reads only companies listed today. The last backtest (run 29) made 0 trades. Before step 1, finish the Form 4 backfill and read past members.
2. **Survivorship in our own SEC ingest.** `companies_due` keeps only `instrument.active && Stock` (`crates/ingest/src/facts.rs:93-104`). The same path feeds fundamentals, 8-Ks and Form 4. EDGAR has delisted companies' filings for free, so a dated ticker→CIK map for past index members and ended listings fixes this without Sharadar. Do it before deciding on Sharadar.
3. **Holes in the concept map.** Each metric maps to one XBRL concept: PP&E has only `PropertyPlantAndEquipmentNet` (`crates/sources/src/edgar/concepts.rs:73`), and capex and operating income have no fallbacks. On 2022-11-01 only 165 of 441 eligible names had a complete 8-year record (gross margin missing 177, capex 105, PP&E 74). Meta's PP&E and Apple's 2008–2012 capex/PP&E are missing. The composite ranker and the avoid list inherit these holes.
4. **No point-in-time SIC or exchange.** Only today's values are stored. A size- and sector-hedged portfolio builder (and NYSE breakpoints, industry exclusions) would use today's classification.
5. **Little free small-cap history before 2016.** Tiingo fills about 3 symbols an hour, 82 of 5,280 done, roughly 70 days to go, with small caps last. On free data, "move down in size" means a 2016-onward window. This number belongs in the Sharadar decision.
6. **Corporate actions Alpaca lacks.**
   - Spin-offs: UTX→CARR/OTIS, PNR→NVT, MRK→OGN, LB→VSCO, XPO→GXO.
   - UA's class C distribution.
   - True splits for RRD 2016, MFCB 2017 and MDRR 2024.

   A missing spin-off looks like a crash and becomes a fake short profit in a long/short overlay. Small caps have many more of these. The overlay needs a second corporate-action source, or no trading over unexplained jumps. There were about 50 unexplained >30% opening jumps among S&P members in 2016–2025; most are real events.

**Gaps in mechanics the plan assumes**

7. **Multi-class share counts tie arbitrarily.** Per-class cover-page counts from one filing tie on (available_at, filed, accession) in `crates/market/src/read.rs:880`. This gives wrong caps for GOOG/GOOGL, BRK, FOX and NWS, so wrong size buckets and hedges. The finding comes from reading the code and has not been confirmed on production data.
8. **No stable company identity for bots.**
   - Bots key their state by ticker: graham, singleton, munger, insider_cluster, bollinger_reversion.
   - A rename or stock merger re-books the position at that day's open, so the cost basis resets (`crates/core/src/ledger.rs:214-238`).
   - An event overlay holding many small names for months will hit renames often.
9. **Borrow history starts 2026-11-11.** The avoid list's "expensive to short" can only be backtested through FINRA short interest (from 2017-12). Borrow cost is forward-only.
10. **The ingest scheduler runs every job in one sequential loop** (`crates/ingest/src/lib.rs` `tick`). On 2026-11-13 the insider and SEC backfills held up macro and price jobs for over an hour. The plan adds about eight feeds (all 8-K items, filing text, FINRA, CBOE, Wikipedia page views, option snapshots, the annotation job). Jobs from different sources need to run concurrently, or new feeds will delay live prices.
11. **The gate, luck line and League judge "beats SPY".** For an overlay bot they must judge the hedged sleeve: its own Sharpe, a t-statistic after the hedge, IWM as a benchmark where relevant. Otherwise overlays are screened on the wrong question. Hedged scoring has to change the gate, not just add a report.

**Smaller items from the same audit**
- **Bot bugs (if these bots are kept):**
  - graham and buffett subtract minority interest twice in the liabilities fallback.
  - cannibal, value_quality and singleton skip the shared fundamentals helpers (no form, unit or staleness filter, no class dedupe).
  - graham and singleton risk the 60 s step timeout.
  - intraday_momentum_spy has no guard on `close[-1]`.
  - opening_range never cancels its entry limit.
  - core_overlay at exposure 0 runs at gross 3.0, exactly its limit.
- **Macro:**
  - No seasonally adjusted CPI, so `real_sales` deflates seasonally adjusted `rsafs` by unadjusted CPI.
  - Months missing from the 2025 shutdown break the consecutive-month assumptions in `sahm_gap` and `labour` (`signals.py`).
  - ccsa, permit, baa10y and t10y3m are stored but unused.
- **The minute-bar gate is stricter than the index gate.** It demands full coverage for every start-universe name, held or not.
- **SUE in quality_momentum** has no fiscal Q4 and sits at exactly the 6-quarter minimum.

**On the Munger/BYD question.** The plan rightly drops large-cap factor tilts. "A durable high-ROIC business, cheap against its own 10-year range on normalised owner earnings" is a different, event-like long idea. Today's bots miss it: value_quality held META in Q4 2022 only by ranking on 2021's peak earnings, next to MRNA, ZIM and steel names. munger missed both Apple 2016 and Meta 2022 because of the concept-map holes. Once gap 3 is fixed it can be one signal in the library, tested by event study like the rest.

**Already done on 2026-11-13** (skip or adjust in the plan):
- **Trial ledger (part of G23):** trials count once per parameter set and version of the bot's own code. A "behaviour hash" covers the bot folder plus the `buizz_bots` modules it imports, so a shared edit no longer resets other bots' screens or kills their queued runs.
- **Corporate actions:** spin-offs, stock dividends, cash mergers and worthless removals re-backfilled. Alpaca splits that the prices refute are no longer applied (63, recorded with evidence).
- **Other data fixes:**
  - FRED dgs10, t10y3m and baa10y were never stored before; they are now read in windows.
  - Ticker identity through renames and reused tickers; merger continuity.
  - Zero-volume filler bars become no-trade sessions.
  - Point-in-time SEC share-count scale correction.
- **Gate:** screening presets start no earlier than 2016-01-04. Index members are judged within their membership. A member's gaps or jumps fail the gate only if the bot touched them.
