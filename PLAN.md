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
  - Per-bar cycle in the engine: at bar start, order expiry → corporate actions → fills at open; at the bar's `visible_at`, mark → risk limits → wake bot → accept orders. Bar N's cycle completes before bar N+1's, so an order decided on bar N fills at bar N+1 open.
  - Whole shares only (signed); splits pay cash in lieu. A target-weight order resolving to zero shares is recorded as rejected `zero_quantity`.
  - Fills use raw prices; splits and dividends hit positions on the ex-date. Bots may ask for back-adjusted history, computed only from actions known at the clock.
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
- **Live** = start or resume the bot's live paper run. **Pause** = cancel working orders, stop the bot process, keep marking equity; resume respawns it with its portfolio and saved state, never replaying the paused interval.
- Protocol (`ws://spine/bot/v1`, bearer run token): `hello` → `ready`, then strict `step` → (`query`/`data`)* → `ack` alternation. `ack` carries orders, cancels, state (≤64KB) and logs. Identical in both modes; in live, bars arriving before an ack coalesce into the next step.
- Orders v1: market and limit, day time-in-force, by `qty` or `target_weight`, each with a free-text `reason` (decision journal). Shorts allowed with a flat borrow rate.

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

Also on `Context`: `history(symbols, tf, bars, adjust)`, `fundamentals(symbols, metrics)`, `universe`, `portfolio`, `events`, `clock`, `session`, `state`, `order`, `cancel`, `log`.

## Data

| Source | Used for | Notes |
|---|---|---|
| Alpaca (free plan) | Daily + 1m history since 2016, calendar, assets, corporate actions, real-time IEX minute bars | Primary. ~30-symbol stream cap, managed by a feed manager with visible slots |
| Tiingo | Daily history before 2016 | Gated by the overlap check; raw close + split/dividend factors, never `adjClose` |
| SEC EDGAR | Point-in-time fundamentals | Raw XBRL facts plus canonical metrics |
| US Treasury | Risk-free rate for Sharpe | |

Verified limits: Alpaca Basic 200 calls/min, history since 2016 minus the latest 15 minutes, 30 IEX stream symbols, minute bars include extended hours (filter to the session); Tiingo free 50 req/h, 1000/day, 500 symbols/month; EDGAR 10 req/s with a contact User-Agent. EODHD is dropped (its free tier adds nothing here).

Reuse from HUG (port the logic, cite the file in the slice brief):
- Scheduler semantics — `../HUG/backend/worker/scheduler.py` (due-from-last-attempt, noop retry), `../HUG/backend/db/jobs.py` (job runs, staleness scan)
- Fetch budget/backoff — `../HUG/backend/db/source_coverage.py`
- Second-source trust gate — `../HUG/backend/adapters/overlap.py` (`validate_overlap`)
- Tiingo, Treasury clients — `../HUG/backend/adapters/{tiingo,treasury}/`
- EDGAR companyfacts parsing, FY/Q classification, share-scale fix, concept map — `../HUG/backend/adapters/edgar/{client,adapter,concept_map}.py`
- UI primitives and rules — `../HUG/frontend/src/components/ui/{Dialog,Toaster}.tsx`, `../HUG/CLAUDE.md:66-83`

Tables. DuckDB: `instrument`, `calendar`, `bar_1d`, `bar_1m`, `corporate_action`, `fundamental_fact`, `rate_daily`, `coverage`, `ingest_batch`. SQLite: `run`, `run_order`, `ledger_entry`, `equity_point`, `run_event`, `job_run`, `source_fetch`. No `bot` table (scan `bots/`), no `position` table (fold the ledger). `bar_1m` and `fundamental_fact` have no primary key (DuckDB index memory): writers replace bars per symbol-session and dedupe facts on their identity fields. `equity_point` is written at session close only, plus the moment a drawdown stop leaves an intraday run flat; intraday marks go out over SSE.

Known parity gap, stated honestly: live minute bars are IEX (partial volume), backtests use SIP history. Live fills are tagged `feed=iex`; the volume cap is ADV-based so it is identical in both modes. Daily live runs use the official daily bar and no stream slots.

## UI

- Bot grid: one card per bot with status, equity sparkline, Live/Pause; hover shows key stats.
- Bot detail: equity vs SPY, metrics, orders with reasons, positions, logs, run history, "Backtest" with period presets or a custom range.
- Leaderboard: bots compared on the same preset period and cost model only.
- Data health: job runs, staleness, coverage, feed slots (e.g. 22/30).
- Single admin login (password → HttpOnly session cookie).

## Added beyond the request

In v1:
1. Benchmark (SPY buy & hold) and metrics per run: CAGR, volatility, Sharpe, Sortino, max drawdown, Calmar, win rate, profit factor, exposure, turnover, alpha/beta.
2. Cost model: commission, slippage bps, ADV participation cap, borrow rate.
3. Risk limits per run: max position weight, max gross exposure, drawdown stop-loss (closes every position and ends the run).
4. Overfitting guard: the last 12 months are a locked holdout; running on it is explicit and counted per bot.
5. Bad-data gate: refuse runs with coverage gaps; flag >40% overnight moves with no corporate action.
6. Survivorship honesty: delisted positions liquidate at last close; runs carry a "survivorship-biased universe" badge.
7. Reproducibility record: bot content hash, engine version, config, seed, data watermark.
8. Headless loop: `buizz backtest bots/x --from --to` prints metrics (the feedback loop for writing bots).
9. Crash recovery: checkpoint with each step; live runs catch up after a restart.

Out of v1: brokers and real money; options, crypto, FX; tick/quote simulation; extended hours; stop/bracket orders; margin model; walk-forward, sweeps, optimisers; index-membership history; non-Python SDKs; multi-user; notifications; live-vs-backtest shadow parity report (v1.1).

## Working agreement for agent work

- Each agent works in its own git worktree and first clones the main checkout's build caches (`cp -c -R <repo>/target ./target`, same for `web/node_modules`) so DuckDB is not recompiled. At most 5 agents at a time.
- An agent owns explicit paths; a contract or dependency change is its own commit touching only those files and is merged first.
- `just check` is green on the merged result before anything is pushed to `main`.

## After Wave 3

1. Walk the whole website end to end in a browser and fix what is broken or unclear.
2. Confirm every worktree branch is merged into `main` and pruned.
3. Code review each module of Buizz (one review per crate, the SDK and the web app); fix confirmed findings.
   Known issues to fold into the reviews:
   - The UI and CLI cannot override a bot's risk limits for one backtest.
   - No time seam: the runner, feed, ingest, API and supervisor call the wall clock directly, so the live e2e scenarios must sleep between 23:28 and 00:11 ET. Add one `now()` in `buizz_core::time` with a test offset.
   - The e2e fake does not serve SEC `company_tickers_mf.json`, so fund classification is untested and the instruments job reports a 404 in e2e.
   - `StepCommit` cannot carry undelivered step events, so a live crash between wakes holds back the checkpoint; add the field so the clock and equity never lag.
   - The "Drawdown stop" badge is inferred in the web UI from the failed status and message; store it as a run badge in the state store instead.
4. Implement the bot backlog in `bots/BRIEFS.md`, one Opus 5.5 agent per bot, each followed by a critical review of that implementation against its brief. Contract gaps (G1–G13) land as their own PRs before the bots they block.
   Gap decisions (free sources first; nothing is ever derived or modelled in place of data):
   - Free and official: G3 SIC codes and G7 earnings-release dates (8-K Item 2.02 filing times) from EDGAR submissions; G15 rates and macro series from FRED (free API key) and the Treasury feed already ingested; G16 extra metrics from EDGAR XBRL concepts.
   - G11 index membership: rebuild point-in-time S&P 500 membership from Wikipedia's current-constituents and changes tables (MediaWiki API, CC BY-SA), joined with Alpaca's inactive assets for delisted prices; results keep a caveat where the change log is thin.
   - G10 borrow: a daily ingest job records Interactive Brokers' public short-availability file (borrow fee and shares available per symbol). It collects wherever ingest runs, which is the cluster's spine (local `just dev` runs with ingest off), so real point-in-time borrow history accumulates there; the bots it blocks stay paused until that history covers what they need.
   - G14 fundamentals before 2009: Yahoo cannot fill it (about four years, unofficial, restated). Free option for later: parse the financial statements of pre-XBRL 10-K/10-Q filings on EDGAR (filed since 1993) for the few metrics the blocked bots need; until then gbm_ranker and graham's defensive variant wait.
   - G8 VIX intraday: no free source reproduces the paper; vol_premium_etf is deferred.
   Gap PRs run as four parallel groups, each additive to the protocol and SDK: orders and costs (G6 auction orders and the pre-close wake, G9 stops, G4 margin, G18 per-share costs); bot queries (G1 calendar, G12 past universes, G15 rates and FRED series); instrument and fundamentals data (G2 asset-class filter, G3 SIC, G17 exchange, G16 new metrics, G7 earnings-release dates, G11 Wikipedia S&P 500 membership, the IB borrow recorder); bot environment (G5 dependencies, G13 shared bot code). Bots start as soon as their gaps land, five at a time, each followed by a critical review against its brief. Real-data backtests run against the cluster's spine, so deployment comes before the bots that need real history.

## Deployment (after Wave 3, separate repo, with your go-ahead)

Order before deploying, so the data spine never has to be wiped and rebuilt:
1. Merge the four gap groups.
2. Review the data spine for durability: versioned schema migrations for both stores (no change may ever require deleting data), point-in-time completeness of every stored dataset, idempotent and resumable backfills, backup and restore of `state.db`, and a short real-data smoke test against the live providers with a handful of symbols.
3. Fix what the review finds, then deploy. Secrets are encrypted with SOPS using the age key on this laptop; provider keys are read from a local file outside the repo and never printed.

The cluster's spine is the data spine: it runs ingest (including the borrow recorder) and builds the real market history. Once it is reachable on the local network, local development and the bot backlog's 10-year backtests run against it (`BUIZZ_URL`) instead of ingesting on the laptop.

Follows `~/akrell/akrell-apps` conventions (pattern: `apps/base/bande-logs/`, guide: `~/Private/Kubit/docs/apps-repository.md`, workflow: `~/Private/mappr/.github/workflows/release.yaml`):
- `apps/base/buizz/`: namespace, `buizz-data` PVC (Longhorn, RWO), single-replica `Recreate` Deployment (non-root, read-only root, emptyDir `/tmp`, 60s grace), Service, HTTPRoute `buizz.mikaelhug.se`, ImageRepository.
- `apps/akrell/buizz/`: `ghcr.sops.yaml`, `buizz.sops.yaml` (`ALPACA_KEY_ID`, `ALPACA_SECRET_KEY`, `TIINGO_API_KEY`, `SEC_USER_AGENT`, `BUIZZ_ADMIN_PASSWORD`), image policy.
- Default chosen: public route + app login, as `bro`/`engly`/`vabblarna` do. Say so if you prefer a LAN-only MetalLB IP.
- Backups: nightly `state.db` snapshots on the PVC (keep 14). The cluster has one Longhorn replica and no off-cluster backup target, so disk loss still loses run history; market data is re-downloadable.

## Needed from you

- A free Alpaca account (paper API key + secret).
- Confirm the Tiingo key and SEC user-agent from HUG may be reused.
