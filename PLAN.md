# Buizz — implementation plan

## Context

Buizz is a platform for testing trading strategies of any kind (technical, value, momentum, fundamental, Singleton-style, "buy red days") against historic and live data. The repo is empty apart from `CLAUDE.md`. The sibling project `../HUG` supplies proven data-layer patterns but is end-of-day only and has no backtester, order simulation or streaming, so the engine, protocol and live path are new.

Decisions already made with you: US stocks & ETFs; daily + 1-minute bars + real-time stream; paper trading only (no broker); develop on the Mac, deploy to the `akrell` Kubernetes cluster.

Waves 0–3 are merged: every crate, the SDK, the web UI, six example bots, the Docker image and release workflow, and the seven end-to-end scenarios in `e2e/` (run by `just e2e`, part of `just check`). Finished work is deleted from this file (CLAUDE.md directive 6).

## Core design: one engine, one event path

- The **spine** (one Rust binary) owns the clock, order matching, fill simulation and the portfolio ledger for every run. Bots are thin external processes that dial in.
- A **run** = bot + mode (`backtest` | `live`) + params + universe + timeframe + start cash + cost model.
- **Live is a backtest whose data has not arrived yet.** The engine only ever reads the market store. Ingest and the live feed write bars and bump a watermark; a live run is the same replay loop blocked at the watermark. This gives backtest/live parity by construction and makes restart recovery a cursor resume.
- **Lookahead is structurally impossible.** Every bot query is answered as-of the run clock, server-side. Rules frozen in `crates/core`:
  - `visible_at`: 1m bar = bar start + 60s; daily bar = session close; fundamentals = next session open after the SEC `filed` date.
  - Per-bar cycle in the engine: at bar start, order expiry → corporate actions → fills (opening auction, then at the open or on touch, then the closing auction); at the bar's `visible_at`, mark → risk limits → wake bot → accept orders. Bar N's cycle completes before bar N+1's, so an order decided on bar N fills at bar N+1 open.
  - Whole shares only (signed); splits pay cash in lieu. A target-weight order resolving to zero shares is recorded as rejected `zero_quantity`.
  - Fills use raw prices; splits, dividends, spin-offs, mergers and worthless removals hit positions on their ex or effective date. Bots may ask for back-adjusted history, computed only from actions known at the clock.
- **Deterministic:** no wall clock in the engine, seeded, ordered maps, `result_hash` per run. Same inputs → same hash.
- **Money:** `rust_decimal` in the ledger, strings on the wire, UTC everywhere. Bars reach bots as f64 columnar buffers; floats never enter the ledger.

## Stack

| Layer | Choice | Why |
|---|---|---|
| Spine | Rust, tokio, axum | Replay loop, matching, fan-out and many parallel runs are the hot path; single static binary |
| Market data | DuckDB embedded (`market.duckdb`) | Columnar scans and as-of joins over millions of bars; re-derivable from sources |
| Run state | SQLite WAL via `rusqlite` (`state.db`) | Small transactional writes, crash-safe ledger and checkpoints; the backup target |
| Bot protocol | WebSocket, binary MessagePack, columnar bar buffers | Language-agnostic, one frame each way per step; Python is not the bottleneck |
| Bot SDK | Python 3.13, uv, msgspec, websockets, numpy | Strategy ecosystem; other languages can implement the protocol later |
| Web UI | React + TypeScript (strict) + Vite + TradingView lightweight-charts; SSE for live updates; types generated from OpenAPI (utoipa) | Lean, typed against the API, proper financial charts |
| Tooling | `just`, clippy/rustfmt, ruff + pyright, zero-comment lint | One command surface; `just check` gates every merge to `main` |

Versions: latest stable at Wave 0, pinned by committed lockfiles.

## Bots

- A bot is a folder: `bots/<name>/{bot.toml, bot.py}`. `bot.toml` declares command, default params, universe (`symbols = [...]` or `top_dollar_volume = N`, recomputed as-of), timeframe and `wake` (`bar`, `every:30m`, `at:15:55`).
- A supervisor in the spine spawns one child process per run (env: spine URL, run id, run token), captures logs, kills by process group, restarts live bots with backoff. Backtests queue behind a core-count semaphore and run in parallel.
- **Live** = start a live paper run of a variant (up to three per bot), or resume a paused one. **Pause** = cancel working orders, stop the bot process, keep marking equity; resume respawns it with its portfolio and saved state, never replaying the paused interval.
- Protocol (`ws://spine/bot/v1`, bearer run token): `hello` → `ready`, then strict `step` → (`query`/`data`)* → `ack` alternation. `ack` carries orders, cancels, state (≤64KB) and logs. Identical in both modes; in live, bars arriving before an ack coalesce into the next step.
- Orders: market, limit, stop, trailing stop, market-on-open and market-on-close (official open and close prints), day or good-till-cancelled, by `qty` or `target_weight`, each with a free-text `reason` (decision journal). Shorts allowed with a flat borrow rate; negative cash is financed at the T-bill rate plus a spread. Daily bots may wake before the close (`before_close:10m`) to trade that close.

```python
from buizz import Bot, Context, run

class RedGreen(Bot):
    def on_step(self, ctx: Context) -> None:
        for symbol, bar in ctx.bars.items():
            if bar.close < bar.open and ctx.position(symbol).qty == 0:
                ctx.order_target_weight(symbol, ctx.params["weight"], reason="red day")
            elif bar.close > bar.open and ctx.position(symbol).qty != 0:
                ctx.order_target_weight(symbol, 0, reason="green day")

if __name__ == "__main__":
    run(RedGreen)
```

Also on `Context`: `history(symbols, tf, bars, adjust)`, `fundamentals(symbols, metrics)`, `calendar(start, end)`, `universe_at(on, n)`, `macro(series, start, end)`, `fomc(start, end)`, `universe`, `portfolio`, `events`, `clock`, `session`, `state`, `order`, `cancel`, `log`.

## Data

| Source | Used for | Notes |
|---|---|---|
| Alpaca (free plan) | Daily + 1m history since 2016, calendar, assets, corporate actions, real-time IEX minute bars | Primary. ~30-symbol stream cap, managed by a feed manager with visible slots |
| Tiingo | Daily history before 2016 | Gated by the overlap check; raw close + split/dividend factors, never `adjClose` |
| Wikipedia | Point-in-time S&P 500 membership, daily page views of each member's article since 2015-07-01 | Membership rebuilt from the constituents and change-log articles (CC BY-SA); views from the Wikimedia REST API, summed over the titles an article was moved from |
| Interactive Brokers | Short availability, recorded daily | Public FTP file; not used by runs yet |
| SEC EDGAR | Point-in-time fundamentals, SIC codes, earnings-release times, 8-K texts and the typed facts recipes read from them (`ctx.facts`) | Raw XBRL facts plus canonical metrics; submissions; one version per recipe serves bots, and each run records the versions it read |
| US Treasury | Risk-free rate for Sharpe and bots | |
| FRED | Macro series for bots (T-bill, 10-year, Moody's Aaa/Baa, CPI-U, unemployment, real-time Sahm rule, daily 10-year and spreads, weekly jobless claims, industrial production, building permits, advance retail sales) with every ALFRED vintage | Free API key |
| CBOE | Daily VIX (from its 2003 methodology), VIX3M, VIX9D, VVIX, SKEW, COR1M and COR3M closes, each from its launch, and the total, equity and index put/call ratios since 2006-11 for bots | Free history files and daily market statistics |
| SEC fails-to-deliver | Outstanding settlement fails per security since 2004 for bots | Twice-monthly files without a stated date: published on the download date or an assumed later date |
| Federal Reserve Board | FOMC meeting calendar for bots | Public pages |

Verified limits: Alpaca Basic 200 calls/min, history since 2016 minus the latest 15 minutes, 30 IEX stream symbols, minute bars include extended hours (filter to the session); Tiingo free 50 req/h, 1000/day, 500 symbols/month; EDGAR 10 req/s with a contact User-Agent. EODHD is dropped (its free tier adds nothing here).

Reuse from HUG (port the logic, cite the file in the slice brief):
- Scheduler semantics — `../HUG/backend/worker/scheduler.py` (due-from-last-attempt, noop retry), `../HUG/backend/db/jobs.py` (job runs, staleness scan)
- Fetch budget/backoff — `../HUG/backend/db/source_coverage.py`
- Second-source trust gate — `../HUG/backend/adapters/overlap.py` (`validate_overlap`)
- Tiingo, Treasury clients — `../HUG/backend/adapters/{tiingo,treasury}/`
- EDGAR companyfacts parsing, FY/Q classification, share-scale fix, concept map — `../HUG/backend/adapters/edgar/{client,adapter,concept_map}.py`
- UI primitives and rules — `../HUG/frontend/src/components/ui/{Dialog,Toaster}.tsx`, `../HUG/CLAUDE.md:66-83`

Tables. DuckDB: `instrument`, `calendar`, `bar_1d`, `bar_1m`, `corporate_action`, `fundamental_fact`, `rate_daily`, `macro_observation`, `fomc_meeting`, `company`, `earnings_release`, `index_member`, `index_history`, `borrow_availability`, `coverage`, `ingest_batch`. SQLite: `run`, `run_order`, `ledger_entry`, `equity_point`, `run_event`, `job_run`, `source_fetch`. No `bot` table (scan `bots/`), no `position` table (fold the ledger). `bar_1m` and `fundamental_fact` have no primary key (DuckDB index memory): writers replace bars per symbol-session and dedupe facts on their identity fields. `equity_point` is written at session close only, plus the moment a drawdown stop leaves an intraday run flat; intraday marks go out over SSE.

Known parity gap, stated honestly: live minute bars are IEX (partial volume), backtests use SIP history. Live fills are tagged `feed=iex`; the volume cap is ADV-based so it is identical in both modes. Daily live runs use the official daily bar and no stream slots.

## UI

- Bots answers four questions in order: is anything beating SPY live, what is worth going live next, what needs attention, and what is running. Under the title one line states live runs, bots ready to go live and bots backtesting. A desk follows. Live paper is a tile per live run: its variant, CAGR vs SPY (live since the run started, else the backtest it went live from, with that backtest's curve in grey), equity against SPY, a track of 20 pieces that fills one per session until the run is judged, and its verdict (Starts Fri 22:00, Paused before its first close, Day 3 of 20, On track, Off track, Beats SPY, No backtest to compare); on phones the tiles scroll sideways. A rail beside it holds Needs attention (off-track or errored live runs, bots whose latest run failed or hit its drawdown stop, failed or late data jobs, with identical alerts grouped into one line such as `breadth_regime ×3 paused`), else All clear, and Ready to go live (screened bots with Go live), else Closest to passing (the drafts failing the fewest gate checks, which checks, and Sweep); below 1120px the rail sits under the tiles. Below, one list grouped by lifecycle (Live paper, Ready to go live, Fails the gate, Not screened yet, the last folded away with a one-click 10y backtest of every idle bot in it), filterable by group with counts. The benchmark is context, not a row datum: its CAGR and max drawdown are stated once per window above the list (SPY 10y, and any other benchmark a bot.toml names), and a row says only `vs AOR` when its benchmark differs. A row is about the bot: CAGR vs SPY as the primary figure with a centred bar (live since the run started, else the screening backtest), its status, its backtest (for a live run the source backtest; gate result and thirds), its own CAGR and max drawdown on wide screens, equity against the benchmark, last run, and its one next step (Go live, Sweep or Backtest) beside the overflow menu. Every column sorts within its group, a search filters, nothing moves on hover, and everything patches in place from the event stream. The screening backtest is the plateau variant among the standard runs on the gate's period, else the default params, else the latest variant; it screens, it never ranks.
- Bot detail: opens on the live run, else the screening backtest, so a bot always shows its charts. The overview leads with one sentence that answers the question for the run (beats or trails total-return SPY by how much a year, drawdown beside SPY's, and the gate result for the screening backtest; before a live run's first close, when it first trades and what happens next, with the backtest it went live from and that backtest's curve in place of the empty chart), then the chart, metrics (on narrow screens only each group's headline until expanded) and monthly returns, with the setup folded away; run badges such as survivorship-biased are quiet pills with their explanation. A run reads Backtest or Live paper, and a screening backtest that passes the gate offers Go live with its params. Research states SPY's CAGR, Sharpe and max drawdown for the period once beside its dates, and leads with the plateau suggestion (its CAGR vs SPY, thirds ahead and curve) and the variant heatmap, coloured by CAGR vs SPY or Sharpe; every variant shows its CAGR vs SPY, its edge over SPY at SPY's volatility in each third of the period, and its curve against SPY. A breadcrumb trail, one header (name, status, timeframe, Live/Pause/Sweep/Backtest, a collapsible description), then a run bar whose run picker switches between recent runs and shows the run's period, clock, equity, costs, badges and reproducibility. Tabs reflected in the URL (`#/bots/<name>[/runs/<id>][/<tab>]`): Overview (equity vs SPY with an underwater pane, metrics grouped into vs benchmark (headed by CAGR vs SPY), returns, risk and trading with a definition and the reason for any blank, monthly returns on a diverging colour key with the benchmark per month, and the run's params, cost model and risk limits), Trades (round trips with entry and exit reasons, P&L and holding time), Orders with reasons, Positions & events, Logs (bot output) and History (every run; select two to four to compare). Drawdowns read negative everywhere. "Backtest" offers period presets or a custom range.
- Run chart: equity against the benchmark's total-return buy and hold from the same starting capital (the close before the first session), with drawdown-from-peak and optional gross-exposure panes. A crosshair legend gives values, return since the start of the visible range and drawdown for both, and toggles each series. Range presets (1M to 5Y, YTD, All), a $, % or log scale, wheel, drag, pinch and keyboard zoom and pan, and markers for drawdown stops, risk breaches, pauses and resumes. It reads as a text summary to screen readers.
- League asks which live runs beat total-return SPY with confidence. One sentence says how many live runs were ever started (M) and that about 5% of them would look significant by luck; SPY's return over the shared live window is stated once above the table, and a run over another window states its own under its excess. Live runs (running, paused and retired) with at least 20 sessions are ranked by the probabilistic Sharpe of daily excess return over SPY, with CAGR vs SPY ± its standard error as the headline, confidence, sessions, return, excess return, drawdown, verdict and the sessions still needed (minimum track record length at 1 − 0.05/M). Younger runs sit in a collapsed "too early" group; with none ranked yet the page says how many are waiting and when the first close is.
- Data opens on the spine: one compact summary with the counts of datasets, sources, symbols priced and markets, a health bar of the datasets by health whose counts filter the page (`?health=`), and the watermarks runs see; a row of source chips (health, how many datasets each feeds); picking one (`?source=`) hides every dataset it does not feed and opens its panel with each of its jobs (schedule, last run, next run, last message); then one dense row per dataset grouped by kind (prices, reference, filings and estimates, macro and rates, news and short selling), each with its name and sources, a timeline per market from 2000 to today (one block per year of twelve monthly columns, each as tall as the symbols covered that month on one log scale shared by every dataset, a faint bed for years without data, lighter full-height months for a dataset that is one series, red for empty months inside its range, counted per symbol across sources so a gap between two sources stays visible, and for a tail where it stops more than two months before today, except minute prices, which are fetched on demand), its symbol count and years, its coverage contract (a bar of done against expected, and complete, filling with the end date its measured pace gives, or stalled with the reason), its health and when it last updated; the summary counts the contracts complete and stalled. A row opens to its coverage per source (symbols, dates), each contract's scope with expected, done, failed and pending, complete-through date, pace and state, and its jobs. A `contracts` event patches the contracts in place. A Text volumes card below the summary shows each volume's use against its 90% fill limit and opens a small browser: folders and files from the catalogue, a file's documents, and a document's text in a dialog with a link to the filing on EDGAR; it refreshes when a document job finishes. Kind comes first and market is a timeline inside each dataset, so a new country adds a timeline beside the US one. Job and feed events patch it in place with a short pulse, and a finished job refetches the spine. Details (`#/data?view=details`) keeps the full view: one sentence on the watermarks runs see and four summary cards (coverage contracts complete, filling and stalled, jobs healthy against stale and failing, stream slots and connections, warnings), then the panels: job outcome, last run and next due per job (a plain notice when ingest is off), staleness, each coverage contract, coverage and its gaps, feed slots (e.g. 22/30) with the runs holding them.
- Notices: a run that finishes, fails or ends at its drawdown stop raises a toast linking to it, derived from the shared event stream.
- Single admin login (password → HttpOnly session cookie).
- Compare (`#/compare/<id,id,…>`): two to four runs' returns since their own start overlaid with the first run's total-return benchmark, and a metric table (CAGR vs SPY first) with each run's difference from the first, the best value per row marked and the params that differ.
- Working fast: a command palette (⌘K) jumps to any page, bot or recent run and starts a backtest, sweep, go live, pause or resume; single keys (`g b`, `g l`, `g d`, `g r`, `b`, `s`, `/`) are listed under `?`. Tables select rows (shift-click for a range) for bulk pause, resume, backtest, go live, cancel or compare. Pausing offers Undo in its toast. Text filters live in the URL query (`#/league?q=…`).
- Look: soft charcoal dark (and a warm paper light) with off-white text, one muted teal accent for the bot's own line and marks, SPY always grey, faded sage, rose, sand and slate for better, worse, warning and info, 3–6px corners, hairline rules instead of nested cards, and every figure in a tabular mono with a slashed zero.
- Shell: sticky topbar with the live-updates status in words, a light/dark/system theme menu and a title per page. Pages use the full window width. A tab left open across a deploy that cannot load a page offers Reload. Definitions open from an `Info` button (a popover), not from `title` alone, so they work on touch and keyboard.

## Next: backtests screen, live paper decides

Approved design. A backtest is a pass/fail screen, never a scoreboard; only live paper results rank bots. Slices ship one at a time; delete each once done.

- Lifecycle is derived from runs, never stored. A version is bot code hash + params; a live run is that version with its own "since live" clock. Stages: draft (no screening backtest at the current hash), screened (a variant passes the gate), live incubating (< 20 sessions), live tracking (inside the backtest's envelope), live falsified (outside it; flagged, retiring is the owner's click), proven (probabilistic Sharpe of daily excess return over SPY ≥ 1 − 0.05/M, M = live runs ever started), retired (live run ended with a reason).
- `before_close` daily bots fetch session minute bars over REST instead of leasing stream slots. Intraday bots: N Alpaca stream accounts pooled by the feed manager (cap 30 × N); a symbol stays on its connection until released.
- Headline metric everywhere: CAGR versus SPY over the same window (bot CAGR − SPY CAGR); plain CAGR and total return stay visible as secondary numbers. Windows under a year are marked as annualized from N sessions.
- Slices: (7) optional replay of the live window.

## Added beyond the request

In v1:
1. Benchmark (SPY buy & hold) and metrics per run: CAGR, volatility, Sharpe, Sortino, max drawdown, Calmar, win rate, profit factor, exposure, turnover, alpha/beta.
2. Cost model: commission (with optional monthly volume tiers), slippage as the larger of a bps floor and each stock's estimated half-spread plus square-root market impact, slippage per share, ADV participation cap, borrow rate, financing spread over the T-bill rate.
3. Risk limits per run: max position weight, max gross exposure, drawdown stop-loss (closes every position and ends the run).
4. Overfitting guard: the last 12 months are a locked holdout; running on it is explicit and counted per bot.
5. Bad-data gate: refuse runs with coverage gaps; flag >40% overnight moves with no corporate action.
6. Survivorship honesty: delisted positions liquidate at last close; runs carry a "survivorship-biased universe" badge.
7. Reproducibility record: bot content hash, engine version, config, seed, data watermark, recipe versions read.
8. Headless loop: `buizz backtest bots/x --from --to` prints metrics (the feedback loop for writing bots).
9. Crash recovery: checkpoint with each step; live runs catch up after a restart.

Out of v1: brokers and real money; options, crypto, FX; tick/quote simulation; extended hours; bracket orders; margin calls; walk-forward and optimisers; non-Python SDKs; multi-user; notifications; live-vs-backtest shadow parity report (v1.1).

## Working agreement for agent work

- Each agent works in its own git worktree and first clones the main checkout's build caches (`cp -c -R <repo>/target ./target`, same for `web/node_modules`) so DuckDB is not recompiled. At most 5 agents at a time.
- An agent owns explicit paths; a contract or dependency change is its own commit touching only those files and is merged first.
- `just check` is green on the merged result before anything is pushed to `main`.

## After Wave 3

Open data-trust work: S&P 500 member sessions without a priced listing remain where no source links a member to its earlier ticker: renames before 2019, which Alpaca's corporate actions do not list (TSO to ANDV, DLPH to APTV, LB to BBWI, BHGE to BKR, DPS to KDP, so KDP before July 2018 still carries Dr Pepper Snapple's mapped history and a DPS holder is closed at its last price instead of converted), Wikipedia members that continue through a merger (Praxair into Linde, Mylan into Viatris, Paramount into Paramount Skydance), a member Alpaca has no bars for (TFCFA) and a member listed before its IPO (Ceridian before April 2018). A listing that ended through a merger Alpaca does not record (SunTrust, TECO) is closed at its last price instead of exchanged. Before 2016 membership is priced only where Tiingo's history has arrived, and Tiingo's own ticker mapping is not resolved by listing yet. Symbols Tiingo extended before the split and dividend cross-check landed are not cross-checked yet.

1. Walk the whole website end to end in a browser and fix what is broken or unclear.
2. Confirm every worktree branch is merged into `main` and pruned.
3. Code review each module of Buizz (one review per crate, the SDK and the web app); fix confirmed findings.
4. Implement the bot backlog in `bots/BRIEFS.md`, one Opus 5.5 agent per bot, each followed by a critical review of that implementation against its brief. Contract gaps (G1–G13) land as their own PRs before the bots they block.
   Landed: G1 calendar lookahead (`ctx.calendar`), G12 past universes (`ctx.universe_at`), G15 rates and FRED series (`ctx.macro`; the Welch–Goyal `dp`, `ep`, `bm` and `ntis` have no free point-in-time source and still block gbm_ranker).
   Gap decisions (free sources first; nothing is ever derived or modelled in place of data):
   - Landed: G2 (`universe.asset_class`), G3 (current SIC codes; EDGAR keeps no history, a documented caveat), G7 (`ctx.earnings`), G11 (`universe.index = "sp500"`, reliable from 2011, `thin_index_history` badge before), G16 (15 new EDGAR metrics; employees refused, only about 50 filers tag it), G17 (current listing exchange; no free history, a documented caveat), and the G10 recorder (the `borrow` job; not exposed to runs, so the G10 bots stay paused).
   - Free and official: G3 SIC codes and G7 earnings-release dates (8-K Item 2.02 filing times) from EDGAR submissions; G15 rates and macro series from FRED (free API key) and the Treasury feed already ingested; G16 extra metrics from EDGAR XBRL concepts.
   - G11 index membership: rebuild point-in-time S&P 500 membership from Wikipedia's current-constituents and changes tables (MediaWiki API, CC BY-SA), joined with Alpaca's inactive assets for delisted prices; results keep a caveat where the change log is thin.
   - G10 borrow: a daily ingest job records Interactive Brokers' public short-availability file (borrow fee and shares available per symbol). It collects wherever ingest runs, which is the cluster's spine (local `just dev` runs with ingest off), so real point-in-time borrow history accumulates there; the bots it blocks stay paused until that history covers what they need.
   - G14 fundamentals before 2009: Yahoo cannot fill it (about four years, unofficial, restated). Free option for later: parse the financial statements of pre-XBRL 10-K/10-Q filings on EDGAR (filed since 1993) for the few metrics the blocked bots need; until then gbm_ranker and graham's defensive variant wait.
   - G8 VIX intraday: no free source reproduces the paper; vol_premium_etf is deferred.
   Gap PRs run as four parallel groups, each additive to the protocol and SDK: orders and costs (G6 auction orders and the pre-close wake, G9 stops, G4 margin, G18 per-share costs); bot queries (G1 calendar, G12 past universes, G15 rates and FRED series); instrument and fundamentals data (G2 asset-class filter, G3 SIC, G17 exchange, G16 new metrics, G7 earnings-release dates, G11 Wikipedia S&P 500 membership, the IB borrow recorder); bot environment (G5 dependencies, G13 shared bot code). Bots start as soon as their gaps land, five at a time, each followed by a critical review against its brief. Real-data backtests run against the cluster's spine, so deployment comes before the bots that need real history.
   Landed: G5 (the locked bots environment in `bots/pyproject.toml`) and G13 (the shared package `bots/buizz_bots`).
   Landed: G6 (market-on-open and market-on-close at the official prints, the `before_close:<n>m` wake), G9 (stop and trailing-stop orders, good-till-cancelled), G4 (leverage, negative cash financed at the T-bill rate plus a spread; no margin calls) and G18 (per-share slippage and monthly commission tiers).

## Deployment (after Wave 3, separate repo, with your go-ahead)

Secrets are encrypted with SOPS using the age key on this laptop; provider keys are read from a local file outside the repo and never printed.

The cluster's spine is the data spine: it runs ingest (including the borrow recorder) and builds the real market history. Once it is reachable on the local network, local development and the bot backlog's 10-year backtests run against it (`BUIZZ_URL`) instead of ingesting on the laptop.

Follows `~/akrell/akrell-apps` conventions (pattern: `apps/base/bande-logs/`, guide: `~/Private/Kubit/docs/apps-repository.md`, workflow: `~/Private/mappr/.github/workflows/release.yaml`):
- `apps/base/buizz/`: namespace, `buizz-data` PVC (Longhorn, RWO), single-replica `Recreate` Deployment (non-root, read-only root, emptyDir `/tmp`, 60s grace), Service, HTTPRoute `buizz.mikaelhug.se`, ImageRepository.
- `apps/akrell/buizz/`: `ghcr.sops.yaml`, `buizz.sops.yaml` (`ALPACA_KEY_ID`, `ALPACA_SECRET_KEY`, optional `ALPACA_STREAM_KEYS`, `TIINGO_API_KEY`, `SEC_USER_AGENT`, `FRED_API_KEY`, `BUIZZ_ADMIN_PASSWORD`), image policy.
- Default chosen: public route + app login, as `bro`/`engly`/`vabblarna` do. Say so if you prefer a LAN-only MetalLB IP.
- The spine runs ingest there, so the `borrow` job records Interactive Brokers' short availability daily from the first deploy; `just dev` runs with ingest off and records nothing.
- `buizz-data` PVC of 50 GiB; pod requests 1 CPU and 2 GiB, limits 4 CPU and 4 GiB, `BUIZZ_DUCKDB_MEMORY=1GB`, `BUIZZ_DUCKDB_THREADS=2` (sizing in `docs/architecture.md`).
- Backups: nightly `state.db` snapshots (keep 14) and an incremental Parquet backup of the market tables no source can give again (borrow history, revisions, index membership) on the PVC. The cluster has one Longhorn replica and no off-cluster backup target, so disk loss still loses run history and the borrow history; the rest of the market data is re-downloadable. Copying `backups/` off the cluster now and then is the cheap protection.

## Needed from you

- A free Alpaca account (paper API key + secret).
- A free FRED API key for the macro series bots ask for.
- Confirm the Tiingo key and SEC user-agent from HUG may be reused.
