# Buizz — implementation plan

## Context

Buizz is a platform for testing trading strategies of any kind (technical, value, momentum, fundamental, Singleton-style, "buy red days") against historic and live data. The repo is empty apart from `CLAUDE.md`. The sibling project `../HUG` supplies proven data-layer patterns but is end-of-day only and has no backtester, order simulation or streaming, so the engine, protocol and live path are new.

Decisions already made with you: US stocks & ETFs; daily + 1-minute bars + real-time stream; paper trading only (no broker); develop on the Mac, deploy to the `akrell` Kubernetes cluster.

Finished slices are deleted from this file (CLAUDE.md directive 6). Wave 0 (contracts) is merged: `crates/core` is complete, every other crate is a compiling stub with frozen signatures, and `docs/protocol.md`, `fixtures/` and both `schema.sql` files are the contracts.

## Core design: one engine, one event path

- The **spine** (one Rust binary) owns the clock, order matching, fill simulation and the portfolio ledger for every run. Bots are thin external processes that dial in.
- A **run** = bot + mode (`backtest` | `live`) + params + universe + timeframe + start cash + cost model.
- **Live is a backtest whose data has not arrived yet.** The engine only ever reads the market store. Ingest and the live feed write bars and bump a watermark; a live run is the same replay loop blocked at the watermark. This gives backtest/live parity by construction and makes restart recovery a cursor resume.
- **Lookahead is structurally impossible.** Every bot query is answered as-of the run clock, server-side. Rules frozen in `crates/core`:
  - `visible_at`: 1m bar = bar start + 60s; daily bar = session close; fundamentals = next session open after the SEC `filed` date.
  - Per-bar cycle (`core::visibility::CyclePhase`): at bar start, corporate actions → order expiry → fills at open; at the bar's `visible_at`, mark → risk limits → wake bot → accept orders. Bar N's cycle completes before bar N+1's, so an order decided on bar N fills at bar N+1 open.
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
| Tooling | `just`, GitHub Actions, sccache, clippy/rustfmt, ruff + pyright, zero-comment lint | One command surface; CI enforces what HUG lost when its CI was removed |

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
            elif bar.close > bar.open:
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

Tables. DuckDB: `instrument`, `calendar`, `bar_1d`, `bar_1m`, `corporate_action`, `fundamental_fact`, `rate_daily`, `coverage`, `ingest_batch`. SQLite: `run`, `run_order`, `ledger_entry`, `equity_point`, `run_event`, `job_run`, `source_fetch`. No `bot` table (scan `bots/`), no `position` table (fold the ledger). `bar_1m` and `fundamental_fact` have no primary key (DuckDB index memory): writers replace bars per symbol-session and dedupe facts on their identity fields. `equity_point` is written at session close only; intraday marks go out over SSE.

Known parity gap, stated honestly: live minute bars are IEX (partial volume), backtests use SIP history. Live fills are tagged `feed=iex`; the volume cap is ADV-based so it is identical in both modes. Daily live runs use the official daily bar and no stream slots.

## UI

- Bot grid: one card per bot with status, equity sparkline, Live/Pause; hover shows key stats.
- Bot detail: equity vs SPY, metrics, orders with reasons, positions, logs, run history, "Backtest" with period presets.
- Leaderboard: bots compared on the same preset period and cost model only.
- Data health: job runs, staleness, coverage, feed slots (e.g. 22/30).
- Single admin login (password → HttpOnly session cookie).

## Added beyond the request

In v1:
1. Benchmark (SPY buy & hold) and metrics per run: CAGR, volatility, Sharpe, Sortino, max drawdown, Calmar, win rate, profit factor, exposure, turnover, alpha/beta.
2. Cost model: commission, slippage bps, ADV participation cap, borrow rate.
3. Risk limits per run: max position weight, max gross exposure, drawdown kill switch (auto-pause).
4. Overfitting guard: the last 12 months are a locked holdout; running on it is explicit and counted per bot.
5. Bad-data gate: refuse runs with coverage gaps; flag >40% overnight moves with no corporate action.
6. Survivorship honesty: delisted positions liquidate at last close; runs carry a "survivorship-biased universe" badge.
7. Reproducibility record: bot content hash, engine version, config, seed, data watermark.
8. Headless loop: `buizz backtest bots/x --from --to` prints metrics (the feedback loop for writing bots).
9. Crash recovery: checkpoint with each step; live runs catch up after a restart.

Out of v1: brokers and real money; options, crypto, FX; tick/quote simulation; extended hours; stop/bracket orders; margin model; walk-forward, sweeps, optimisers; index-membership history; non-Python SDKs; multi-user; notifications; live-vs-backtest shadow parity report (v1.1).

## Slices

Each slice owns its paths exclusively. Agents run in git worktrees at medium effort.

Contract notes for slice owners: stub parameters are `_`-prefixed and stub structs empty, so rename and add fields freely inside your crate. The SDK must use the websockets sans-I/O `ClientProtocol` over a blocking socket (the only transport that met the 20k steps/s gate, ~24k measured). TypeScript stays on 5.9 while openapi-typescript requires it.

Wave 1 is merged. Handoff notes for Wave 2 (the code is the reference):
- Runner: `commit_step` needs `step_seq` exactly last+1; drain the engine Outbox and `checkpoint()` in that same commit. Screen unknown and unshortable symbols before `engine.accept` (the engine has no instrument data). Repeated `client_id`s are dropped silently. Bots get `BUIZZ_RUN_TOKEN`; logs land in `data/logs/<run_id>.log` relative to the spine's working directory.
- Runner/feed: market watermarks are in memory only; re-advance them on boot. Use `bars_between` for chunk preloads and apply `bar_visible_at` yourself.
- Ingest: load the calendar before bars and facts (rows outside it are dropped); widen corporate-action ranges (Alpaca filters on process date); budget Tiingo's 1000/day and 500 symbols/month; back off on `RateLimited{retry_after}` via `record_fetch`; snapshots need a fresh dated path.
- Unverified until Alpaca keys exist: calendar times assumed America/New_York; SDK throughput (~19.5k steps/s measured against a Python fake under heavy load) is re-measured against the real runner.

**Wave 2 — parallel (6 agents)**

| Slice | Owns | Done when | Model |
|---|---|---|---|
| Runner | `crates/runner/` | Backtest lockstep, live tail-follow, coalescing, checkpoint/recovery, risk limits; `red_green` passes through the real binary | Opus |
| Feed manager | `crates/feed/` | Ref-counted subscriptions, cap refusal naming holders, gap backfill, watermark notifications | Opus |
| Web UI | `web/` (not `schema.d.ts`) | Grid, hover stats, detail, Live/Pause, leaderboard, data health against the stub API | Opus |
| Ingest | `crates/ingest/` | Tick scheduler, jobs, lazy 1m coverage, nightly `state.db` snapshot; second tick is a noop | Sonnet |
| API | `crates/api/` | Real handlers + SSE + login replace every stub fixture; serves the UI | Sonnet |
| Example bots | `bots/` | `red_green`, `sma_cross`, `momentum_12_1`, `value_quality` (Buffett), `cannibal` (Singleton buybacks), `opening_range` (1m) | Sonnet |

**Wave 3 — integration (3 agents)**

| Slice | Owns | Done when | Model |
|---|---|---|---|
| End-to-end + fake Alpaca | `e2e/` | The seven scenarios below are green; slice-level checks they supersede are deleted | Opus |
| Deploy | `Dockerfile`, `.github/workflows/release.yaml` | amd64 image on `ghcr.io/mikaelhug/buizz`, `tini` PID 1, bots' uv env synced `--frozen` at build | Sonnet |
| Docs | `README.md`, `docs/` (not `protocol.md`) | Small and current: run, write a bot, architecture | Sonnet |

**Merge protocol**
- One slice = one squash PR, rebased on main, `just check` green.
- Slices never edit `crates/core`, schemas, root manifests, lockfiles, generated files or `PLAN.md`.
- A contract or dependency change is its own PR touching only those files, merged first; open worktrees rebase.
- Wave 2 merges runner → feed → ingest → API → web → bots. A wave starts only from a green main.
- The orchestrating session spawns the agents, merges, and prunes `PLAN.md`.

## After Wave 3

1. Walk the whole website end to end in a browser and fix what is broken or unclear.
2. Confirm every worktree branch is merged into `main` and pruned.
3. Code review each module of Buizz (one review per crate, the SDK and the web app); fix confirmed findings.
4. Implement the bot backlog in `bots/BRIEFS.md`, one Opus 5.5 agent per bot, each followed by a critical review of that implementation against its brief. Contract gaps (G1–G13) land as their own PRs before the bots they block.

## Deployment (after Wave 3, separate repo, with your go-ahead)

Follows `~/akrell/akrell-apps` conventions (pattern: `apps/base/bande-logs/`, guide: `~/Private/Kubit/docs/apps-repository.md`, workflow: `~/Private/mappr/.github/workflows/release.yaml`):
- `apps/base/buizz/`: namespace, `buizz-data` PVC (Longhorn, RWO), single-replica `Recreate` Deployment (non-root, read-only root, emptyDir `/tmp`, 60s grace), Service, HTTPRoute `buizz.mikaelhug.se`, ImageRepository.
- `apps/akrell/buizz/`: `ghcr.sops.yaml`, `buizz.sops.yaml` (`ALPACA_KEY_ID`, `ALPACA_SECRET_KEY`, `TIINGO_API_KEY`, `SEC_USER_AGENT`, `BUIZZ_ADMIN_PASSWORD`), image policy.
- Default chosen: public route + app login, as `bro`/`engly`/`vabblarna` do. Say so if you prefer a LAN-only MetalLB IP.
- Backups: nightly `state.db` snapshots on the PVC (keep 14). The cluster has one Longhorn replica and no off-cluster backup target, so disk loss still loses run history; market data is re-downloadable.

## Verification

All scenarios use the real spine binary, real bot processes and fixture data (CLAUDE.md directive 2):
1. **Hand-computed backtest** — `red_green` over 10 sessions: ledger, equity and metrics match the hand calculation; a rerun gives the same `result_hash`.
2. **Lookahead boundary** — a probe bot asks for everything every step: no bar past the clock, filing invisible until next open, no fill on the decision bar.
3. **Intraday sessions** — 1m bot with `every:30m`: correct step count, limit fills on touch, day orders expire at 13:00 on the half day.
4. **Live, pause, resume (Playwright)** — fake Alpaca replays a session; click Live and see fills arrive; Pause cancels orders and stops the bot; Resume skips the paused interval; live ledger equals the backtest ledger on the same bars.
5. **Crash recovery** — `kill -9` the bot, then the spine: no duplicate orders; final ledger equals the uninterrupted run.
6. **Ingest** — empty spine backfills from fake providers; 429 is a recorded backoff; second tick is a noop; mismatched Tiingo tail is rejected and shown in data health.
7. **Limits** — a run exceeding 30 stream symbols is refused naming the holders; a drawdown breach auto-pauses; an oversized order is rejected.

Final manual check: `just dev`, run every example bot over the standard period from the UI, start one live during market hours.

## Needed from you

- A free Alpaca account (paper API key + secret).
- A private GitHub repo `mikaelhug/buizz` (CI and image builds).
- Confirm the Tiingo key and SEC user-agent from HUG may be reused.
