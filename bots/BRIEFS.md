# Bot backlog

Thirty-five strategy briefs, one per `bots/<name>/BRIEF.md`, each to be researched and implemented by its own agent. The six example bots (`red_green`, `sma_cross`, `momentum_12_1`, `value_quality`, `cannibal`, `opening_range`) belong to the Wave 2 slice and stay simple SDK demos. `buffett` and `singleton` here are the research-faithful versions of `value_quality` and `cannibal`. Delete this file when the last brief is gone.

**Design rule: the research decides the bot, not the platform.** A brief describes the strategy exactly as its source defines it. Where Buizz cannot express part of it, the brief names a gap below. Nobody ships a reduced version to fit today's contracts.

## Gaps

Each gap is its own contract PR, merged before the bots it blocks.

| Gap | What the research needs | Proposed contract change | Blocks |
|---|---|---|---|
| G1 Calendar lookahead | Knowing a session is the last of its week or month, or that the next day is a holiday | `ctx.calendar(from, to)` query over the exchange calendar | sixty_forty, halloween, turn_of_month, gtaa5, dual_momentum, sector_rotation, low_volatility, short_term_reversal, trend_following, multi_factor_composite, gbm_ranker, betting_against_beta, risk_managed_momentum, meta_allocator, buffett, ml_trend_quarterly |
| G2 Asset-class universe filter | Stock-only cross-sections (otherwise SHV wins "lowest vol" and SPY/IVV/VOO win "closest pair"); hedging ETFs stay tradable | `universe.asset_class` in `bot.toml`, applied to `top_dollar_volume` | bollinger_reversion, low_volatility, short_term_reversal, pead_sue, piotroski_fscore, pairs_distance, multi_factor_composite, stat_arb_pca, gbm_ranker, betting_against_beta, risk_managed_momentum, buffett, munger, graham, singleton, ml_trend_quarterly |
| G3 Industry classification | Industry-neutral ranks, same-industry pairs, industry-ETF residuals, excluding financials and utilities | SIC code from EDGAR submissions on `instrument`, exposed to bots | multi_factor_composite, gbm_ranker, munger, stat_arb_pca (ETF variants), pairs_distance (industry variant), graham (defensive variant) |
| G4 Leverage and margin model | Gross exposure above 1× with a financing cost | Margin model: financing rate on negative cash and a gross cap above 1 | donchian_breakout, trend_following, betting_against_beta, risk_managed_momentum, stat_arb_pca, noise_boundary_spy, meta_allocator, buffett, hrp_allocation (levered variant) |
| G5 Python dependencies | hmmlearn, lightgbm, scipy, torch | One dependency PR to the bots' uv env | regime_hmm, gbm_ranker, hrp_allocation, meta_allocator, ml_trend_quarterly |
| G6 Auction orders | Trading at the official close or open, as the papers do. Daily bots need a pre-close decision point (e.g. 15:45–15:50 with the bar so far) to submit market-on-close orders | `MarketOnClose` and `MarketOnOpen` order types plus a pre-close wake for daily bots | turn_of_month, rsi2_pullback, ibs_reversion, bollinger_reversion, overnight_drift, intraday_momentum_spy, stat_arb_pca, noise_boundary_spy, vol_premium_etf, meta_allocator |
| G7 Earnings announcement dates | PEAD and announcement-return features measure from the earnings release, not the later 10-Q | 8-K Item 2.02 filing timestamps from EDGAR as point-in-time events | pead_sue, gbm_ranker |
| G8 Volatility data | VIX and VIX3M, intraday, since the decision is taken before their 16:15 close | Cboe index series (1m and daily) in the market store | vol_premium_etf |
| G9 Stop and trailing-stop orders | Protective and trailing stops filled intrabar | Stop and trailing-stop order types, matched like limits | donchian_breakout, vwap_reversion |
| G10 Per-name borrow | Realistic cost and availability for shorting small, high-beta or crowded names | Borrow rate per instrument and a hard-to-borrow flag | short_term_reversal, pead_sue, piotroski_fscore, pairs_distance, multi_factor_composite, stat_arb_pca, gbm_ranker, betting_against_beta, risk_managed_momentum, buffett, singleton (long/short variant) |
| G11 Historical index membership | S&P 500, Russell 1000 or CRSP universes as used in the papers | Out of v1 per PLAN.md; results carry the survivorship caveat | low_volatility, multi_factor_composite, stat_arb_pca, gbm_ranker, betting_against_beta, risk_managed_momentum, buffett |
| G12 Point-in-time universe history | Past cross-sections before the run starts, to train on years of history | Query for the universe as-of a past date | gbm_ranker, ml_trend_quarterly |
| G13 Shared code between bots | One bot reusing another bot's signal functions without copying them | A shared bots package or an import convention | meta_allocator, buffett |
| G14 Fundamentals before 2009 | 10- to 30-year accounting records (EDGAR XBRL starts 2009) | A second fundamentals source with pre-XBRL history; none is free, so this needs a decision on a paid source | gbm_ranker, graham (defensive variant) |
| G15 Rates and macro series for bots | T-bill and Treasury yields, Moody's AAA, CPI-U and the Welch–Goyal predictors as point-in-time data | Expose `rate_daily` to bots and add the FRED series | buffett, gbm_ranker, graham (defensive and 1976 variants) |
| G16 Missing fundamentals | Short-term debt, preferred stock, minority interest, retained earnings, income taxes payable, short-term investments, strictly noncurrent long-term debt; employees, convertible and secured debt, buildings and land, advertising | New `FundamentalMetric`s in the EDGAR concept map | multi_factor_composite, gbm_ranker, buffett, munger, graham, singleton |
| G17 Listing exchange | NYSE breakpoints for size and decile sorts | Expose `instrument.exchange` to bots, point-in-time | risk_managed_momentum, buffett |
| G18 Per-share costs | $/share slippage and volume-tiered commissions, as intraday papers assume | Per-share slippage and commission tiers in `CostModel` | noise_boundary_spy |

G11 and G18 are caveats, not blockers (G18's error is under 0.02 bps a side). Every other gap blocks the bots it lists, except where it's marked as a variant.

Execution rule: papers that rebalance monthly or less often trade at the signal's close, but the standard implementation lag of filling at the next open does not change those strategies, so they don't need G6. G6 applies where the edge lives within a few days or within the session.

## Unblock order

| After | Newly buildable |
|---|---|
| today | dip_ladder |
| G1 | sixty_forty, halloween, gtaa5, dual_momentum, sector_rotation |
| G6 | rsi2_pullback, ibs_reversion, overnight_drift, intraday_momentum_spy |
| G5 | regime_hmm, hrp_allocation |
| G1 + G6 | turn_of_month |
| + G2 | bollinger_reversion, low_volatility |
| + G16 | singleton, graham |
| + G3 | munger |
| + G10 | short_term_reversal, piotroski_fscore, pairs_distance, multi_factor_composite |
| + G9 | vwap_reversion |
| + G4 | donchian_breakout, trend_following, noise_boundary_spy, betting_against_beta, stat_arb_pca |
| + G17 | risk_managed_momentum |
| + G13, after its sleeves | meta_allocator |
| + G15 | buffett |
| + G7 | pead_sue |
| + G8 | vol_premium_etf |
| + G12 | ml_trend_quarterly |
| + G14 | gbm_ranker |

## Index

### Simple: one rule, few symbols, no fundamentals

| Bot | Idea | Gaps |
|---|---|---|
| `sixty_forty` | 60/40 SPY/AGG, rebalanced at month end; the baseline | G1 |
| `halloween` | SPY November–April, T-bills May–October | G1 |
| `turn_of_month` | SPY over trading days −1 to +3 of each month | G1, G6 |
| `rsi2_pullback` | Connors RSI(2) pullback above the 200-day SMA | G6 |
| `ibs_reversion` | Internal bar strength mean reversion on index ETFs | G6 |
| `bollinger_reversion` | Lower-band reversion in liquid stocks with a trend filter | G2, G6 |
| `dip_ladder` | Resting limit buys at −2/−4/−6%, take profit or time exit | none |
| `gtaa5` | Faber's 5-asset 10-month SMA timing | G1 |
| `donchian_breakout` | Turtle System 2 with N sizing, pyramiding, 2N stops | G4, G9 |
| `overnight_drift` | Long SPY close-to-open | G6 |

### Medium: ranking, rotation, intraday, light fundamentals

| Bot | Idea | Gaps |
|---|---|---|
| `dual_momentum` | Antonacci Global Equities Momentum | G1 |
| `sector_rotation` | Top-3 sector SPDRs by blended momentum | G1 |
| `low_volatility` | Lowest-vol decile of large caps | G1, G2, G11 |
| `short_term_reversal` | Weekly market-neutral reversal deciles | G1, G2, G10 |
| `trend_following` | Moskowitz–Ooi–Pedersen time-series momentum, vol-scaled | G1, G4 |
| `pead_sue` | Post-earnings drift on standardised unexpected earnings | G2, G7, G10 |
| `piotroski_fscore` | F-score within the cheapest book-to-market quintile | G2, G10 |
| `pairs_distance` | Gatev–Goetzmann–Rouwenhorst distance pairs | G2, G10, G3 (variant) |
| `intraday_momentum_spy` | First half-hour predicts last half-hour | G6 |
| `vwap_reversion` | Intraday fade of deviations from VWAP | G9 |

### Complex: multi-factor, stat-arb, models, optimisation

| Bot | Idea | Gaps |
|---|---|---|
| `multi_factor_composite` | Integrated value, momentum, quality, low-vol score; industry-neutral, dollar-neutral, beta-hedged | G1, G2, G3, G10, G11, G16 |
| `stat_arb_pca` | Avellaneda–Lee PCA residual s-scores, 2+2 leverage, SPY beta hedge | G2, G4, G6, G10, G11, G3 (ETF variants) |
| `regime_hmm` | Nystrup adaptive 2-state HMM tilting a 10-ETF 60/40 between risk-on and risk-off | G5 |
| `gbm_ranker` | Gu–Kelly–Xiu GBRT+H on the full characteristic set, value-weighted decile spread | G1, G2, G3, G5, G7, G10, G11, G12, G14, G15, G16 |
| `betting_against_beta` | Frazzini–Pedersen BAB, beta-neutral, with the Novy-Marx–Velikov value-weighted variant | G1, G2, G4, G10, G11 |
| `risk_managed_momentum` | Barroso–Santa-Clara vol-scaled WML; Daniel–Moskowitz dynamic variant | G1, G2, G4, G10, G11, G17 |
| `hrp_allocation` | López de Prado HRP over 10 cross-asset ETFs, re-estimated every 22 sessions | G5, G4 (levered variant) |
| `noise_boundary_spy` | Zarattini–Aziz–Barbon intraday noise-area momentum with a VWAP stop | G4, G6, G18 (caveat) |
| `vol_premium_etf` | Zarattini–Mele–Aziz VIX-sized short/long vol in SVXY and VIXY | G6, G8 |
| `ml_trend_quarterly` | Gradient-boosted trees on price-trend features, top 25 stocks held each quarter; tests a retail "35% a year" target | G1, G2, G5, G12 |
| `meta_allocator` | Equal-risk mix of the trend, IBS reversion and HRP sleeves, each scaled to 10% vol | G1, G4, G5, G6, G13 |

### Complex: investor styles

| Bot | Idea | Gaps |
|---|---|---|
| `buffett` | Frazzini–Kabiller–Pedersen systematic Buffett: beta 0.68 plus factor tilts in Berkshire's proportions, scaled to its volatility | G1, G2, G4, G10, G11, G13, G15, G16, G17 |
| `munger` | Five Gray–Carlisle franchise-power compounders at median-or-better EBIT/TEV, no rebalancing | G2, G3, G16 |
| `graham` | Net-nets below two-thirds of NCAV (primary); defensive criteria and the 1976 rule as variants | G2, G16; G3, G14, G15 (variants) |
| `singleton` | Value repurchasers: bottom-decile net issuance in the top book-to-market quintile, held 48 months | G2, G16, G10 (variant) |

## Agent prompt

Use this prompt for each bot's Opus 5.5 agent, in its own worktree:

> Implement `bots/<name>` for Buizz. Read `CLAUDE.md`, `PLAN.md`, `docs/protocol.md`, the Python SDK, `bots/BRIEFS.md` and `bots/<name>/BRIEF.md`.
>
> 1. Research the strategy in depth: the original paper, at least one independent replication and known pitfalls (lookahead, survivorship, costs, decay). Refine the brief's rules to match the source exactly.
> 2. Check every gap the brief lists against main. If one hasn't landed, or you find a new one, stop and report what the faithful version needs. Never implement a reduced version.
> 3. Write `bot.toml` and `bot.py` following the example bots. Use `ctx.history` and `ctx.fundamentals` only, keep state small, and give every order a `reason`.
> 4. Tune only on the 10y preset, never on the holdout. Run `buizz backtest bots/<name>` over 10y.
> 5. Put the metrics and how they compare with the source in the PR description, not the repo. Delete `BRIEF.md`.

Agents start after Wave 2 (SDK, runner, ingest) and after the gap PRs their bot needs. Each owns only `bots/<name>/`, so they run in parallel.

## Facts every brief relies on

- One timeframe per bot. Daily bots wake at the session close and fill at the next open. A 1m bot may query daily history.
- Orders are market or limit, day time-in-force, by `qty` or `target_weight`.
- Fundamentals are point-in-time EDGAR metrics (`crates/core/src/market.rs` `FundamentalMetric`), visible from the next open after filing. Market cap = `SharesOutstanding` × close, with shares adjusted for splits after `period_end`.
- `top_dollar_volume = N` is recomputed as-of. A live 1m bot holds at most about 30 stream slots.
- Signals use total-return (`adjust = "all"`) history; fills use raw prices.
- EDGAR XBRL facts start in 2009 for large filers and 2011 for all filers. A fundamental bot's first valid signal is that date plus its lookback.
- Whole shares only; a target weight that rounds to zero shares is rejected as `zero_quantity`.
