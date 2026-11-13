# Writing a bot

A bot is a folder under `bots/` with a `bot.toml` and a program. The spine starts the program, sends it one `step` per bar and collects its orders. The bot never sees the future: every answer is as of the run clock.

## Layout

```
bots/red_green/
  bot.toml
  bot.py
```

The spine rescans `bots/` on every request, so a new or edited bot shows up without a restart. A folder is a bot when it holds a `bot.toml`; folders starting with `.` are ignored. Each run records a content hash of the folder and never continues under different code: when a backtest is (re)started after a restart, or a live run is started, resumed, starts its bot for a new session or restarts it after a crash, with edited bot code, the backtest fails and the live run pauses with the reason. Stop it and start a new run.

## Python environment and shared code

Bots run in one shared uv project, `bots/pyproject.toml` (package `buizz-bots`), selected by `--project ..` in each `command`. It depends on the SDK and on `hmmlearn`, `lightgbm`, `scikit-learn` and `scipy`, all pinned in `bots/uv.lock`; the SDK project itself stays lean (`msgspec`, `numpy`, `websockets`). To add a dependency, put it in `bots/pyproject.toml` and run `cd bots && uv lock`; `just setup` installs the environment with `uv sync --frozen`, the SDK editable, so a change to `sdk/python` reaches the bots without reinstalling. The Docker image builds the same locked environment at image build time (non-editable, bytecode compiled, `libgomp1` for LightGBM), so a bot starts offline from a read-only filesystem. On macOS LightGBM needs OpenMP: `brew install libomp`. A dependency without type information gets a minimal stub of the parts the bots use in `bots/typings/` (hmmlearn's `GaussianHMM` has one), so the strict type check still covers its calls.

Code that more than one bot needs lives in the package `bots/buizz_bots/` and is imported as `from buizz_bots.<module> import ...`, for example a bot reusing the signal functions of another. Move the functions into a module there, import them from both bots, and do not copy them. The folder has no `bot.toml`, so it is not a bot. It is installed into the bots environment: editable locally, copied into the image. Shared modules follow the same lint and strict type checks as bots (`just lint`) and must not import a bot's `bot.py`.

The content hash of a run covers the bot folder plus the shared package `bots/buizz_bots/`, `bots/pyproject.toml`, `bots/uv.lock` and the SDK source `sdk/python/src/` beside the bots folder (where the repository and the image both keep it), so editing shared code, the SDK or dependencies changes the hash of every bot, and a live or resumed run whose code changed is stopped instead of continuing under the old hash.

## Walkthrough: `red_green`

`bots/red_green/bot.toml`:

```toml
description = "Buys after a red day, exits after a green day."
command = ["uv", "run", "--frozen", "--project", "..", "python", "bot.py"]
timeframe = "1d"
wake = "bar"
universe = { symbols = ["AAA", "BBB"] }
params = { weight = 0.25 }
```

`bots/red_green/bot.py`:

```python
from buizz import Bot, Context, run


class RedGreen(Bot):
    def on_step(self, ctx: Context) -> None:
        weight: float = ctx.params["weight"]
        for symbol, bar in ctx.bars.items():
            if bar.close < bar.open and ctx.position(symbol).qty == 0:
                ctx.order_target_weight(symbol, weight, reason="red day")
            elif bar.close > bar.open and ctx.position(symbol).qty != 0:
                ctx.order_target_weight(symbol, 0, reason="green day")


if __name__ == "__main__":
    run(RedGreen)
```

`run(RedGreen)` reads `BUIZZ_SPINE_URL` and `BUIZZ_RUN_TOKEN` from the environment, connects, and calls `on_step` once per step until the spine ends the run. After a red day without a position the bot asks for a 25% position; after a green day with one it asks for zero. Both orders fill at the next day's open.

## `bot.toml`

| Field | Required | Meaning |
|---|---|---|
| `description` | no | One line shown in the UI |
| `command` | yes | Argv of the program, not empty. Run with the bot folder as working directory |
| `timeframe` | yes | `"1d"` or `"1m"`: the bar size the bot is stepped on |
| `wake` | no, default `"bar"` | When the bot is called, see below |
| `universe` | yes | `{ symbols = ["SPY", "QQQ"] }` (not empty), `{ top_dollar_volume = 500 }` (positive, optionally with `asset_class = "stock"` or `"etf"`) or `{ index = "sp500" }` |
| `params` | no | Default parameters, available as `ctx.params`. A backtest request may override them but not add others: an unknown name is refused, as are unknown request and cost fields |
| `benchmark` | no, default `"SPY"` | Symbol the equity curve is compared with, as a total-return buy and hold: dividends reinvested at the ex-date open, splits followed |
| `[risk]` | no | Risk limits for live runs and the default for backtests, see [Risk limits](#risk-limits) |
| `step_timeout` | no, default `60` | Seconds the spine waits for the bot to connect and for each of its frames, up to `3600`: raise it for a bot that trains a model inside one step. A bot that stays silent longer fails its backtest or pauses its live run |

The spine starts `command` with `BUIZZ_SPINE_URL`, `BUIZZ_RUN_ID` and `BUIZZ_RUN_TOKEN` set. Of the spine's own environment the bot only inherits `PATH`, `HOME`, `LANG`, `TZ`, `TMPDIR` and the `UV_*` and `PYTHON*` variables, so API keys and the admin password never reach it. Output on stdout and stderr is captured to `logs/<run_id>.log` under the spine's data directory (`BUIZZ_DATA_DIR`, default `data`).

**Universe.** `symbols` is a fixed list. `top_dollar_volume = N` selects the N symbols with the highest average dollar volume over the last 20 sessions, recomputed as of each session; the step carries `universe` whenever it changed and `ctx.universe` follows. With `asset_class = "stock"` only stocks enter that ranking, so cross-sections are not crowded by SPY, IVV, VOO or SHV; a fund is any ticker on the SEC's mutual fund and ETF list, and without SEC EDGAR configured every instrument counts as a stock. The filter only shapes the universe: a bot may still trade and query any instrument, so an ETF hedge stays tradable.

Any listed instrument is tradable, whatever the universe. An order for a symbol the run does not follow yet is sized at its last price visible at the clock (the last minute bar of the session so far in a `1m` run or at a `before_close` step, else the last daily close, adjusted for the splits visible by then); from then on, while the run holds it or has an order in it, it is filled, marked to market, gets its corporate actions and its bars reach `ctx.bars`, like a member. A `1m` or `before_close` backtest backfills its minute bars first; a live run fetches its minute bars of the session so far before sizing the order, so it is priced as its backtest would be (if that fetch fails, the run logs a `warn` line and uses the last daily close), and a live `1m` run then leases a stream slot for it. Nothing else of the market is loaded, so a run that only orders inside its universe is unchanged.

`index = "sp500"` is the S&P 500 as of each session, rebuilt from Wikipedia's current constituents and its change log (see [Index membership](#index-membership)), limited to members with daily bars in the last 20 sessions.

**Tickers.** A symbol names the company that traded under it at the clock, in the universe, in orders and in every query: `FB` in 2018 is Facebook (whose history Alpaca keeps under META), `META` then is not Facebook, and after June 2022 `FB` is whatever later took the ticker. A held position follows a name change into the new ticker, one share for one.

**Wake.** Every bar reaches the bot, but it is only stepped when its wake condition holds; bars that became visible between wakes arrive together at the next step (`ctx.bars` keeps the last bar per symbol).

- `bar`: every bar.
- `every:30m` or `every:2h`: when the time since the session open is a positive multiple of the interval, up to the close. `1m` bots only.
- `at:15:55`: when the exchange-local (New York) time is 15:55. `1m` bots only.
- `before_close:10m`: that many minutes before each session's close, so 15:50 on a normal day and 12:50 on a half day; it must be under 210 minutes, the length of a half day. A `1d` bot then wakes only there, ten minutes into the last part of the session, instead of at the close: see [Deciding before the close](#deciding-before-the-close).

**Params.** The values in `bot.toml` are defaults. A backtest request, or `buizz backtest --param weight=0.5`, overrides single keys; the merged object is stored with the run.

## SDK

```python
from buizz import Bot, Context, run
```

A bot subclasses `Bot` and implements `on_step(ctx)`. `on_start(ctx)` runs once after the spine's `ready` and `on_end(ctx, reason)` once when the run ends for this process. `reason` is an `EndReason`: `completed`, `paused`, `stopped`, `risk_limit` (the drawdown stop, see [Risk limits](#risk-limits)), `failed` or `session_end` as the spine sent it, or `disconnected` when the connection went away without one. A pause or stop can arrive in the middle of `on_step`, while it waits on a query or as the SIGTERM that stops the process: the step is abandoned (its orders and state changes are not sent) and `on_end` still runs, with about 10 seconds before the process is killed.

A backtest is one process from start to end. A live run is one process per session: it starts at the session's first wake and gets `session_end` after the step of its last wake, so a daily bot lives a few seconds a day and a 1m bot from the open to the close. `on_start` therefore runs once per live session, and its work must fit in `step_timeout` together with the connection; keep it to validating params. Anything kept on `self` (a cache of histories, a fitted model) is gone at the next session: rebuild it from `ctx.history` when it is missing, and put what cannot be rebuilt in `ctx.state`, which carries over with the holdings and the working orders.

Outside `on_step` only `params`, `universe`, `state`, `portfolio` and `position` are usable. The step members (`clock`, `session`, `bars`, `events`), queries, orders, cancels and logs raise `UsageError` there; `on_end` can still read the state and print to stdout, which goes to the log file.

### Reading

| Member | Meaning |
|---|---|
| `ctx.params` | The merged params dict |
| `ctx.clock` | The step time as a UTC `datetime`: the session close for a daily bar, one minute after the start for a 1m bar |
| `ctx.session` | `Session(date, open, close)` of the current session |
| `ctx.universe` | Current symbols |
| `ctx.bars` | `dict[symbol, Bar]` (`open`, `high`, `low`, `close`, `volume` floats): the latest bar of each symbol that became visible since the previous step |
| `ctx.events` | `list[Event]`: the `Fill`, `OrderUpdate` and `CorporateAction` events since the previous step |
| `ctx.portfolio` | `cash`, `equity`, `positions` (`qty`, `avg_cost`) and `working` orders (`WorkingOrder`: `client_id`, `symbol`, `side`, `qty`, `filled_qty` and `order_type`: `Market()`, `Limit(limit_price)`, `Stop(stop_price)`, `TrailingStop(trail, stop_price)` with the stop as it trails now, `MarketOnOpen()` or `MarketOnClose()`) as of the start of the step. Orders you place in this step are not reflected |
| `ctx.position(symbol)` | The `Position`, with `qty == 0` when flat |
| `ctx.history(symbols, tf="1d", bars=1, adjust="raw")` | The last `bars` (1 to 10000) bars per symbol visible at the clock, as numpy arrays (`start` in ns, `open`, `high`, `low`, `close`, `volume`). Each call is a round trip to the spine |
| `ctx.fundamentals(symbols, metrics)` | Every fundamental fact visible at the clock for the given metrics (`Metric` names, below) |
| `ctx.instruments(symbols)` | `dict[symbol, InstrumentProfile]` (`name`, `exchange`, `asset_class`, `sic`, `sic_description`) for the known symbols. Current values, see below |
| `ctx.earnings(symbols)` | `list[EarningsRelease]` (`symbol`, `cik`, `accession`, `form`, `filed`, `report_date`, `accepted_at`): every earnings release accepted by EDGAR at or before the clock |
| `ctx.insider_trades(symbols, since)` | `list[InsiderTrade]`: every insider transaction from a Form 4 or 4/A filed on or after `since` and visible at the clock, see below |
| `ctx.news(symbols, since)` | Experimental. `list[NewsArticle]`: every recorded version of the news articles about any of `symbols` created on or after `since` and known at the clock, see below |
| `ctx.universe_at(on, top_dollar_volume, *, asset_class=None, index=None)` | The symbols a `top_dollar_volume` universe of that size (optionally of one `asset_class`), or the `index` universe, held on the date `on`, see below |
| `ctx.macro(series, start, end)` | The rate, macro and S&P 500 valuation observations dated `start` to `end` as known at the clock (`MacroPoint`: `series`, `date`, `value`, `published`, `available_at`), see below |
| `ctx.fomc(start, end)` | The FOMC announcements dated `start` to `end` known at the clock (`FomcAnnouncement`: `date`, `scheduled`, `statement_at`, `available_at`), see below |
| `ctx.calendar(start, end)` | The exchange sessions (`Session(date, open, close)`) dated `start` to `end`, as the calendar was published at the clock, see below |
| `ctx.state` | A dict that survives restarts, see below |

Symbols, metrics and series are lists of `str` (`["SPY"]`, never a bare `"SPY"`); they, `tf`, `bars` and `adjust` are checked before the query is sent: a wrong value raises `ValueError` or `TypeError` in the step. `adjust` is `raw`, `split` or `all` (splits and dividends). Adjustment only uses corporate actions whose ex-date session has opened by the clock, so history never reflects a split that has not happened yet. Fills always use raw prices.

Fundamentals are snake_case metrics, typed as `Metric` (an unknown name raises `ValueError`): `revenue`, `cost_of_revenue`, `gross_profit`, `research_development`, `sga_expense`, `operating_expenses`, `operating_income`, `interest_expense`, `pretax_income`, `income_tax`, `net_income`, `eps_diluted`, `total_assets`, `current_assets`, `cash_and_equivalents`, `inventory`, `receivables`, `ppe_net`, `goodwill`, `current_liabilities`, `long_term_debt`, `total_liabilities`, `equity`, `operating_cash_flow`, `depreciation_amortization`, `stock_based_comp`, `capex`, `investing_cash_flow`, `financing_cash_flow`, `dividends_paid`, `share_repurchases`, `acquisitions`, `shares_outstanding`, `short_term_debt`, `short_term_borrowings`, `current_long_term_debt`, `long_term_debt_noncurrent`, `preferred_stock`, `minority_interest`, `retained_earnings`, `income_taxes_payable`, `short_term_investments`, `convertible_debt`, `secured_debt`, `land`, `buildings`, `ppe_gross`, `advertising`. A `FundamentalPoint` carries `period_type` (`FY` or `Q`), `period_start`, `period_end`, `value`, `unit`, `form`, `filed` and `available_at`; the latest filing per period wins. `unit` is the XBRL unit the filer reported in, never converted: `USD` for amounts, `shares` for share counts and `USD/shares` for `eps_diluted` from US-dollar filers, but a foreign filer reporting in its home currency gives amounts in `JPY`, `EUR`, `TWD` and so on (and `JPY/shares` per share), so compare `unit` before mixing companies. A fact becomes visible at the open of the first session after its `filed` date, never earlier. Share counts such as `shares_outstanding` are as reported, before any later split: `split_ratio(raw, split_adjusted, day)` takes a raw and a split-adjusted `ctx.history` of the same symbol and returns how many shares today one share on `day` has become (`None` when the history does not reach back to `day`). `bots/value_quality` is a complete example. `buizz_bots.fundamentals` holds what the research fundamentals bots share (`bots/singleton`, `bots/munger`, `bots/buffett`, `bots/graham`): annual facts from domestic 10-K filings in US dollars only (`in_dollars`: amounts in `USD`, share counts in `shares`, per-share amounts in `USD/shares`), so 20-F and 40-F filers and 10-K filers reporting in another currency drop out; the latest run of consecutive fiscal years; one definition of total debt (noncurrent long-term debt plus current debt, or a reported total long-term debt plus short-term borrowings, so current maturities are never counted twice) and of gross profit (reported, else revenue minus cost of revenue); the share count of a balance sheet, the first count dated on or up to 120 days after it, so a filer that tags only its cover-page count still has one; the fourth quarter as the fiscal year minus its three reported quarters; and share counts split-adjusted from each fact's `filed` date, since a filing restates its share counts for splits before it was filed (SEC SAB Topic 4C). `buizz_bots.drift.rebalance_within_band` rebalances with a drift `band`, leaving a held leg alone while it is within `band` of its target; when the untouched legs together hold more than their targets, it trims the most overweight of them back to target first, so entries get their full weight and the book never needs more than its targets add up to. The cap-weighted S&P 500 bots (`bots/spy_ex_junk`, `bots/sp500_quality_momentum`, `bots/sp500_momentum_cap`) share `buizz_bots.caps`: `market_caps` (last close times the latest 10-K or 10-Q share count from `buizz_bots.fundamentals.latest_shares`, split-adjusted from its filing date, one line per company when share classes report the same count), `capped_weights` (proportional weights with per-name caps, the excess spread over the uncapped names) and `parked` (what the weights leave over, in a cash symbol such as SPY); `buizz_bots.fundamentals.refreshed` keeps a symbol's points between rebalances and fetches them again only when its latest filing changed. `buizz_bots.ranks` holds the cross-sectional rank z-score, percentiles and a highest-first order whose ties, frequent with discrete ranks, break by symbol rather than by floating-point noise; `buizz_bots.monthly.momentum` is the 12-1 month return. `buizz_bots.beta` holds the Frazzini–Pedersen beta (one-year daily volatilities, five-year overlapping three-day correlations, shrunk 0.6 to 1) and the rank-weighted BAB stock weights. `buizz_bots.volatility.ex_ante_volatility` is the annualised EWMA volatility of daily returns for a centre of mass in days, over the `ewma_sessions` returns whose weights are at least 1e-4 (`bots/trend_following`, `bots/vol_target_spy`). The market-timing bots (`bots/growth_trend`, `bots/credit_trend`, `bots/curve_recession`, `bots/vix_trend`, `bots/breadth_regime`, `bots/forward_pe_tilt`, `bots/timing_vote`, `bots/index_momentum`) take their conditions from `buizz_bots.signals`: one function per condition at its published parameters, returning a `Signal` (`bullish` and a `why` for the order reason) or `None` while its data is not visible at the clock: `trend` (total-return close above its 200-day SMA; `monthly_trend` for the 10-month form), `breadth` (at least 50% of the point-in-time S&P 500 members above their own 200-day SMA, over an `SmaCache` that refetches only histories whose last close changed), `credit` (monthly Baa minus 10-year at or below its 6-month mean), `vix_term` (VIX/VIX3M at most 1), `labour` (unemployment at or below its 12-month mean), `curve` (not both a negative 10y-3m within 730 days and a Sahm gap of 0.5, built from `curve_inversions` and `sahm`) and `valuation` (forward earnings yield minus the 10-year yield not in the bottom 20% of its expanding history of at least 3 years). They rest on `buizz_bots.trend` (`sma_trend` and `month_end_sma_trend` on a history, `daily_trend` and `month_end_trend` fetching it: the last total-return close and its simple average over that many sessions or month-end closes, `None` without today's close or enough history) and `buizz_bots.macro.latest_monthly` (the last `count` observations of a monthly series as known at the clock, allowing up to four months of release lag or missing months). `buizz_bots.switch.switch` holds the risk asset at the bot's `risk_on_weight` param (above 1 borrows the rest) or the safe asset at 1, through `hold_only`, which trades only when another symbol is held or the held weight drifted more than 0.10 from its target (`drifted`, also used by `bots/timing_vote`, `bots/trend_leverage_spy` and `bots/index_momentum` with their own band).

Each metric is one reported XBRL concept or a list of concepts with the same meaning, tried in order; a value is never computed from other concepts, so a filer that tags none of them has no point for that metric. `eps_diluted` falls back to the single basic-and-diluted figure of filers without dilutive securities; `equity` and `net_income` fall back to the totals including noncontrolling interests when the filer tags only those. `long_term_debt` takes the noncurrent part when the filer tags it and otherwise the total that may include current maturities; `long_term_debt_noncurrent` is strictly the noncurrent part. `short_term_debt` is the filer's total of short-term borrowings and current maturities (`DebtCurrent`); filers that tag only the parts report them as `short_term_borrowings` and `current_long_term_debt`. `preferred_stock` is the carrying value in equity, `minority_interest` the noncontrolling interest in equity, `income_taxes_payable` current income taxes only, `short_term_investments` the current marketable securities line. `convertible_debt`, `secured_debt`, `land` and `buildings` are often tagged only inside a breakdown table, which SEC company facts leave out, so they are missing for many filers: a missing value means not reported, never zero. The number of employees is not offered: hardly any filer tags it.

`ctx.instruments` answers for the company each symbol names at the clock, with that company's values today, not as of the clock (FB in 2018 gets Meta Platforms' name and class, not the fund that holds FB now). `exchange` is Alpaca's current listing exchange (`NYSE`, `NASDAQ`, `ARCA`, `AMEX`, `BATS`); `sic` is the four-digit Standard Industrial Classification code from the company's EDGAR submissions, `None` for funds and for tickers without a CIK. Neither source keeps a history: EDGAR shows only a company's current SIC code and Alpaca only the current exchange, so a company that moved from NASDAQ to NYSE, or changed industry, carries its current value back in time. Both change rarely; a bot sorting on them should say so in its results.

An earnings release is an 8-K (or 8-K/A) whose items include 2.02, *Results of Operations and Financial Condition*, the filing companies make when they publish quarterly results. It is visible from its EDGAR acceptance time, to the second, which is when it became public on EDGAR; a release accepted after the close is therefore visible on the next step. `report_date` is the event date the filer gave, usually the day of the announcement. An 8-K/A that amends a release (same `report_date`, accepted later) is not returned, so each announcement is one release; an 8-K/A is returned only when it is the first filing carrying 2.02 for its event. Funds and tickers without a CIK have none.

**Insider transactions.** `ctx.insider_trades(symbols, since)` returns the open-market purchases, sales, grants, option exercises and other non-derivative transactions insiders reported on SEC Form 4, as filed. An `InsiderTrade` carries the filing (`symbol`, `accession`, `form` of `"4"` or `"4/A"`, `filed`, `accepted_at`, `available_at`), its `owners` (`InsiderOwner`: `cik`, `name`, `director`, `officer`, `ten_percent_owner`, `other`, `officer_title`) and one transaction: `security`, `date`, `code` (`P` open-market purchase, `S` open-market sale, `A` grant, `M` option exercise, `F` tax withholding, `G` gift and the other SEC codes), `acquired` (`True` for A, `False` for D), `shares`, `price` and `shares_after` (`Decimal`, `None` where the filer left them out) and `direct`. A trade becomes visible at the open of the first session after the filing's EDGAR acceptance time, so a Form 4 accepted at 18:30 is seen at the next open and one accepted at 11:00 at the open after that session. An amendment is a separate, later filing: a 4/A never changes the trades of the 4 it amends, and a bot that wants the corrected figures matches them itself. A joint filing lists every reporting owner on each of its trades, so sum shares per filing, not per owner. History reaches back to mid-2003, when Forms 4 became XML. `bots/insider_cluster` is a complete example, including one way to match a 4/A to the filing it amends.

```python
buys = [t for t in ctx.insider_trades(ctx.universe, ctx.session.date - timedelta(days=30))
        if t.code == "P" and any(o.officer or o.director for o in t.owners)]
```

**News (experimental).** `ctx.news(symbols, since)` reads the Benzinga news archive recorded from Alpaca. A `NewsArticle` is one version of an article (`id`, `symbols`, `headline`, `summary`, `source`, `created_at`, `updated_at`, `received_at`, `backfilled`); an edited article appears once per version. A version the spine recorded live is visible from its `received_at`. One fetched later from Alpaca's history (`backfilled`) is only the latest edit as Alpaca kept it, so it cannot show what was known before; it is visible from its `updated_at`, and an honest test keeps only versions that were recorded live or whose `updated_at` is close to `created_at`. The archive only grows on a deployed spine, so a backtest over the past sees backfilled history only. The query may change.

**Calendar.** The exchange publishes its holidays and early closes years ahead, so `ctx.calendar` answers future sessions too, up to 366 days after the clock's date; a later `end` raises `SpineError` with code `bad_query`. A closure decided at short notice (the national days of mourning on 2018-12-05 and 2025-01-09, the first Juneteenth closure on 2022-06-20) stays the regular session it was scheduled as until the open of the first session after its announcement, so a backtest never knows about it earlier than the market did. A daily bot finds the last session of the month or week from the next session:

```python
from datetime import timedelta

today = ctx.session.date
following = ctx.calendar(today, today + timedelta(days=10))[1].date
month_end = following.month != today.month
week_end = following.isocalendar().week != today.isocalendar().week
```

**Past universes.** `ctx.universe_at(on, n)` applies the `top_dollar_volume = n` rule as of the date `on`: the `n` symbols with the highest average dollar volume over the 20 sessions before it, not counting those delisted before it, highest first. `asset_class="stock"` ranks stocks only, and `ctx.universe_at(on, index="sp500")` gives the S&P 500 members on `on` instead; both resolve exactly as a run's `universe` of the same form does on that date. It answers once the last session before `on` has closed, so any date up to the next session works and a later one raises `SpineError` with code `bad_query`. A model that trains on years of cross-sections asks for each formation date in its first step:

```python
from datetime import date

months = [date(year, month, 1) for year in range(2017, 2024) for month in range(1, 13)]
training = {month: ctx.universe_at(month, 1000) for month in months}
```

**Rates, macro and valuation series.** `ctx.macro(series, start, end)` returns the observations of `MacroSeries` dated `start` to `end`, each as the latest vintage published before the current session, so revisions arrive when they were published and never earlier. `available_at` is the open of the first session after `published`. Values are `Decimal` percent, except the CPI index, the unemployment rate's percentage points, the VIX index points, the activity series in the units their rows give and the valuation series below.

| Series | Meaning | Frequency |
|---|---|---|
| `treasury_3m` | US Treasury 3-month par yield, the rate the metrics use; published the evening of its date | daily |
| `tb3ms` | 3-month T-bill, secondary market (FRED `TB3MS`), Welch–Goyal `tbl` | monthly average, dated the 1st |
| `gs10` | 10-year Treasury, constant maturity (FRED `GS10`) | monthly average |
| `aaa`, `baa` | Moody's seasoned Aaa and Baa corporate bond yields (FRED `AAA`, `BAA`), Graham's AAA rule and Welch–Goyal `dfy = baa − aaa` | monthly average |
| `cpiaucns` | CPI-U, all items, not seasonally adjusted, 1982–84 = 100 (FRED `CPIAUCNS`) | monthly |
| `unrate` | Civilian unemployment rate, seasonally adjusted (FRED `UNRATE`); revised, so the vintages matter | monthly |
| `sahmrealtime` | Real-time Sahm rule recession indicator (FRED `SAHMREALTIME`), percentage points | monthly |
| `dgs10` | 10-year Treasury, constant maturity, H.15 (FRED `DGS10`) | daily |
| `t10y3m` | 10-year minus 3-month Treasury constant maturity (FRED `T10Y3M`), percentage points | daily |
| `baa10y` | Moody's seasoned Baa corporate yield minus the 10-year Treasury (FRED `BAA10Y`), percentage points | daily |
| `icsa` | Initial unemployment insurance claims, seasonally adjusted (FRED `ICSA`), a count of claims | weekly, dated the Saturday that ends the week, released the following Thursday |
| `ccsa` | Continued claims (insured unemployment), seasonally adjusted (FRED `CCSA`), a count of people | weekly, dated the Saturday that ends the week, released a week after `icsa` for it |
| `indpro` | Industrial production, total index, seasonally adjusted (FRED `INDPRO`, Fed G.17), index points | monthly, dated the 1st, released mid-month after |
| `permit` | New private housing units authorized by building permits (FRED `PERMIT`, Census), thousands at a seasonally adjusted annual rate | monthly, dated the 1st, released about 12 business days after |
| `rsafs` | Advance retail and food services sales, Census (FRED `RSAFS`), millions of dollars, not adjusted for inflation | monthly, dated the 1st, released mid-month after |
| `vix` | CBOE Volatility Index close, from 2003-09-22 | daily |
| `vix3m` | CBOE 3-Month Volatility Index close, from 2009-09-18 | daily |
| `sp500_forward_pe` | S&P 500 forward 12-month P/E (FactSet Earnings Insight) | weekly, dated the report's issue date |
| `sp500_forward_pe_5y_avg`, `sp500_forward_pe_10y_avg` | Its 5- and 10-year averages as the report quotes them | weekly |
| `sp500_forward_eps` | The forward 12-month EPS estimate in dollars the P/E was based on, only in issues that state it | weekly 2011–2016, rare since |
| `sp500_trailing_pe` | S&P 500 trailing 12-month P/E | weekly, since mid-2021 |
| `sp500_<sector>_forward_pe` | A sector's forward 12-month P/E, for `energy`, `materials`, `industrials`, `consumer_discretionary`, `consumer_staples`, `health_care`, `financials`, `information_technology`, `communication_services`, `telecom_services` (its name before the 2018 GICS change, a different sector), `utilities` and `real_estate` | weekly, sparse: every sector only in 2011, otherwise the highest and lowest the text names (none in 2012) |

The FRED series need `FRED_API_KEY` on the spine; without it they are empty. Each FRED value is the one ALFRED records as published on its real-time start date, so `published` is the day FRED first carried it, never the day it was computed for. The daily H.15 rates (`dgs10`, and the two spreads FRED derives from H.15 and Moody's) reach FRED the business day after their date at the earliest, so the rate of day D is visible from the open of the session after D + 1, or later when FRED posted it later. ALFRED only keeps vintages from when FRED started recording them: `dgs10` from 2005-06-28, `t10y3m` and `baa10y` from 2014-01-27 and `sahmrealtime` from 2019-09-06 (the series did not exist before). Observations dated earlier carry that first vintage date as `published`, so a backtest before it sees nothing for the series, as nobody could have seen this vintage then; `unrate` has vintages back to the 1960s, `indpro` back to 1927, `permit` from 1999-08-17, `rrsfs` from 2001-06-15, `icsa` from 2009-05-28 and `ccsa` from 2009-09-10. Claims are released on Thursday at 08:30 ET (earlier in some holiday weeks), so a week's `icsa` is first visible at Friday's open; each release also revises the week before, and the seasonal factors are revised every spring, both as later vintages. Monthly averages (`gs10`, `baa`) cover the earlier years.

The `sp500_*` series are the figures FactSet's weekly Earnings Insight states in its text: `date` is the issue date, `published` the day the report became public (a correction uploaded later under the same date with a letter, such as `B`, is a later vintage of that date, published when it was made), so a Friday report is first seen at Monday's open. A figure the report does not state, or states differently in two places, is missing for that week (about 3% of the forward P/Es, where FactSet's summary and body disagree); nothing is computed, so `sp500_forward_eps` is not price over P/E. Ratios are plain numbers (17.7 means 17.7×), EPS is in dollars. They need `SEC_USER_AGENT` on the spine.

The VIX closes at 16:15 ET, a quarter of an hour after the session's last bar, so a close is published on its date and visible from the open of the next session: a daily bot never reads today's VIX at today's close. The VIX before 2003-09-22 in CBOE's file is a later recalculation with the new methodology, not what was published then (the index of those years, renamed VXO, is not offered as a free file), so `vix` starts on 2003-09-22. Values are CBOE's history file as first read by the spine; a close CBOE changes afterwards becomes a new vintage published on the day the spine saw it. Graham's CPI-scaled size floor:

```python
from datetime import date

year = ctx.session.date.year - 1
cpi = ctx.macro(["cpiaucns"], date(year, 1, 1), date(year, 12, 1))
if len(cpi) == 12:
    floor = 100_000_000 * float(sum(point.value for point in cpi) / 12) / 41.8
```

**FOMC.** `ctx.fomc(start, end)` lists the Federal Open Market Committee's announcements from 2000 on, dated by the day the statement was released. A scheduled meeting (`scheduled=True`) is published long in advance, so it is visible from the start of its year, future meetings included; its `statement_at` is the release time the Fed scheduled: 14:00 ET since 2013, 12:30 ET on the 2011–2012 meetings with a press conference, 14:15 ET before. Each scheduled meeting is the one the Fed held: a meeting moved later in its year shows at its final date all year, and the meeting of 2020-03-18, cancelled when the Committee cut rates on Sunday 2020-03-15, stays listed. An unscheduled action (an intermeeting move, a conference call or a notation vote that released a statement) has `scheduled=False` and `statement_at=None`, and is visible only from the open of the first session after its release day, so a bot never knows of an emergency move before it happened. Conference calls without a statement are left out. A pre-announcement drift bot buys the close before a scheduled statement:

```python
from datetime import timedelta

today = ctx.session.date
tomorrow = ctx.calendar(today, today + timedelta(days=10))[1].date
statement_tomorrow = any(meeting.scheduled for meeting in ctx.fomc(tomorrow, tomorrow))
```

`to_ns(moment)` turns a timezone-aware `datetime` into the nanoseconds of `History.start`, for example `history.start >= to_ns(ctx.session.open)`.

### Acting

```python
ctx.order(symbol, qty, *, reason, limit_price=None, stop_price=None, trail_amount=None,
          trail_percent=None, at=None, tif="day", client_id=None) -> str
ctx.order_target_weight(symbol, weight, *, reason, limit_price=None, stop_price=None,
                        trail_amount=None, trail_percent=None, at=None, tif="day", client_id=None) -> str
ctx.rebalance(weights, *, exit_reason, reason=None, at=None, band=0.0)
ctx.cancel(client_id)
ctx.log(message, level="info")
```

- `order` takes a signed share count as an `int` (numpy integers too): positive buys, negative sells. A float raises `TypeError`; round it yourself. `symbol`, `reason`, `client_id` and log messages must be `str`, and `level` one of `debug`, `info`, `warn` or `error`; anything else raises `TypeError` or `ValueError` at the call.
- `order_target_weight` targets a fraction of equity: the target is `trunc(weight * equity / last close)` shares and the order is the difference to the position plus the working orders that execute by this order's session, except stops (a stop may never trigger). A negative weight shorts. A target that needs no change is recorded as rejected with `zero_quantity`. Weights and prices must be finite numbers; NaN or infinity raises `ValueError`.
- `rebalance` makes the book these weights: it cancels every working order, targets each weight and sends every other held symbol to zero with `exit_reason`. At a `before_close` step the cancels only take effect at the close, so a symbol with a working order other than a stop counts as held and is sent to zero too, since the session may still fill that order. It sizes each order like `order_target_weight`, from the step's equity and the last close in `ctx.bars` (or, for a symbol without a bar this step, its last bar in `ctx.history`), and places only the orders that change a position: a symbol already on target places nothing, and the orders that shrink a position go before the ones that grow one, so the book never passes `max_gross_exposure` on the way. A symbol with a non-zero weight and no price yet places nothing and logs a `warn` line instead of sending an order that would be rejected with `no_price`. `reason` is one string, or a mapping with a reason per symbol of `weights`. A zero weight for a symbol you do not hold places nothing, so `ctx.rebalance({}, exit_reason="flat before the close")` flattens the book. `at="close"` or `at="open"` sends every order to that auction. `band` is a drift band, a fraction of each target: a symbol already held on the side of its target is left alone while what it holds is within `band × target` shares of that target, so `band=0.25` resizes a 2% leg only once it drifts below 1.5% or above 2.5% and spares the commissions of small trims. Entries and exits are never held back by the band.
- Quantities are whole shares. Splits pay cash in lieu of fractions and rescale working orders' prices.
- The order type is set by at most one of these keywords; more than one raises `ValueError`:

| Keyword | Order | Fills |
|---|---|---|
| none | market | at the next bar's open |
| `limit_price` | limit, rounded to 4 decimals away from the market (down for a buy, up for a sell) | at the open if it is at or better than the limit, else at the limit when the bar touches it |
| `stop_price` | stop: a sell stop below the market or a buy stop above it | when a bar trades through the stop: at the open if it gaps through, else at the stop, both moved against you by the slippage |
| `trail_amount` or `trail_percent` | trailing stop, `trail_percent` as a fraction (`0.05` for 5%) | like a stop, whose stop starts that far from the last close at the decision and follows the best price since: after each bar a sell stop rises to the bar's high minus the trail, a buy stop falls to its low plus the trail, never back |
| `at="open"` | market-on-open | at the official open of the next session |
| `at="close"` | market-on-close | at the official close of the session; rejected with `auction_closed` after the exchange cutoff, 10 minutes before the close |

- `tif` is `"day"` (expires at the close of its session) or `"gtc"` (works until it fills or you cancel it).
- `reason` is required free text. It is stored with the order and shown in the order list: write the decision, not the action.
- `client_id` is returned. Leave it out to get `<symbol>-<session date>` (daily) or `<symbol>-<YYYY-MM-DDTHH:MM>` (1m), with `-1`, `-2` for repeats in the same step. A client id is unique per run; a repeated one is rejected with `duplicate_client_id` (in the step events and run events; the run's order list keeps only the first) and leaves the first order alone, which makes replaying an ack after a crash safe.
- `cancel` cancels a working order by client id.
- `log` writes a line of the given level (`debug`, `info`, `warn`, `error`) into the run's event log; the bot's stdout and stderr go to the log file.

Orders are rejected, with the reason on the order and in the run events, for: `unknown_symbol`, `duplicate_client_id`, `zero_quantity`, `invalid_limit_price`, `invalid_stop_price` (a stop at or below zero, or a trail that is not positive or a percentage of 1 or more), `no_price` (the symbol has no visible bar yet, or its last session comes before the one the order would fill in), `not_shortable`, `max_position_weight`, `max_gross_exposure`, `invalid_size` (a quantity or target weight too large to compute), `auction_closed` (a market-on-close after the cutoff), `auction_unavailable` (a market-on-open in a `1m` run, see below) and, for live runs that stream minute bars, `no_stream_slot` (the symbol is outside the universe and holdings and the 30-symbol stream has no free slot for it).

```python
ctx.order("SPY", 100, at="close", reason="RSI2 below 10 in an uptrend")
ctx.order_target_weight("SPY", 0, at="open", reason="flat for the session")
ctx.order("GLD", -40, stop_price=181.2, tif="gtc", reason="2N stop")
ctx.order("QQQ", 25, stop_price=505.0, reason="55-day breakout")
ctx.order("TLT", -60, trail_percent=0.05, tif="gtc", reason="trail 5% below the high")
```

### State

`ctx.state` is a dict saved with every step and handed back when the run resumes, so a live bot that restarts continues where it was. It holds only `dict` with `str` keys, `list`, `str`, `int`, `float` (finite), `bool` and `None`, so that it reads back exactly as it was written; anything else (a tuple, a date, a numpy number, an int key) raises `TypeError` or `ValueError` when the step ends. Encoded it may not exceed 64 KiB; more raises `StateTooLarge`. Keep only what cannot be recomputed from `ctx.history`.

## Time and fills

- A bar is visible when it is complete: a 1m bar one minute after its start, a daily bar at the session close, a fundamental fact at the next session open after `filed`, a corporate action at the open of its ex-date session.
- The bot is stepped when a bar is visible. An order decided on bar N fills no earlier than bar N+1's open; there is no way to trade on the bar you are looking at.
- A market order fills at the open of the next bar, moved against you by the slippage. A limit buy fills at the open moved against you by the slippage, but never above the limit, if the open is at or below the limit; otherwise at the limit if the bar's low touches it. A limit sell mirrors that. A stop is matched the same way against the high (buy) or low (sell). A day order that has not filled expires when its session closes; a `gtc` order keeps working.
- An order belongs to the session of the first bar that starts at or after the decision, so an order placed at a daily close fills in the next session, and so does one placed after the last minute of a session. A market-on-open order belongs to the next session that has not opened yet, a market-on-close order to the first session that has not closed yet.
- The auctions fill at the official open and close, the session's daily bar, never at a minute bar's price: a market-on-close in a `1m` run fills at the daily close, not at the last minute's close. Auction fills pay the slippage and commission of a market order and share its volume cap. A market-on-open needs the official open before the session's minute bars can be matched, which a live `1m` run only learns after the close, so `1m` runs reject it with `auction_unavailable`; a daily bot that decides before the close can use it. A live `1m` run with a market-on-close working takes its closing step once the official daily bar has arrived.
- Like the exchange, a market-on-close can't be cancelled after the cutoff: the cancel is ignored and the order fills.
- Splits and cash dividends hit the position at the ex-date open.
- A delisted symbol's position is liquidated at its last close and its orders are cancelled.

### Deciding before the close

Papers that trade "at the close" compute their signal on the close they trade. A `1d` bot gets the same decision point with `wake = "before_close:10m"`: it is stepped ten minutes before every close, and its `market-on-close` orders join that session's closing auction.

- `ctx.clock` is the decision time, 15:50 or 12:50 on a half day. `ctx.bars` holds the session so far for each symbol: the open of the first minute, the high and low of the minutes visible at the clock, the last of their closes as `close`, and their volume. The daily bar of the session is not visible yet, so `ctx.history(tf="1d")` ends at the previous session.
- The session is matched against its daily bar after the close, as in any daily run, so the step's portfolio reflects the session's corporate actions, priced at the last minute, but not its fills yet: market, limit and stop orders that fill during the session, and market-on-open orders, appear at the next step. A cancel for one of them takes effect at the close, after the session was matched, so you can't use the minutes you saw to undo a fill that already happened.
- Market, limit and stop orders placed at the step belong to the next session; a market-on-open fills at the next official open.
- The minute bars of the universe and holdings must be in the store: a backtest backfills them for its period like a `1m` backtest, and a live run leases stream slots for them like a `1m` run.
- `buizz_bots.preclose` holds what such bots share: `session_so_far(ctx, symbol)` returns the bar only when it covers today's session, `with_session_so_far(history, bar)` appends it to a daily history so the trend helpers treat it as today's close (`bots/trend_leverage_spy`), and `ProxyLog` remembers each pre-close decision in the state and logs it at the next step when the official close would have decided otherwise, so the cost of deciding at 15:50 can be counted (`bots/rsi2_pullback`, `bots/ibs_reversion`).

```toml
timeframe = "1d"
wake = "before_close:10m"
```

```python
class Rsi2(Bot):
    def on_step(self, ctx: Context) -> None:
        closes = ctx.history(["SPY"], "1d", 200, adjust="all")["SPY"].close
        today = ctx.bars["SPY"].close
        if rsi2(closes, today) < 10 and ctx.position("SPY").qty == 0:
            ctx.order_target_weight("SPY", 1.0, at="close", reason="RSI2 below 10")
        elif today > closes[-4:].mean() and ctx.position("SPY").qty != 0:
            ctx.order_target_weight("SPY", 0, at="close", reason="close above SMA5")
```

To hold from one close to the next open, buy on the close and sell the next open in the same step; sizing the sell counts the buy, because it executes first:

```python
ctx.order_target_weight("SPY", 1.0, at="close", reason="hold overnight")
ctx.order_target_weight("SPY", 0, at="open", reason="flat for the session")
```


## Costs

The spine starts every run with 100,000 start cash and this cost model, whose fields the `cost` field of a backtest request or `buizz backtest --cost commission_per_share=0.0035 --cost 'commission_tiers=[{"above": 300000, "per_share": "0.002"}]'` set for that run; the fields left out keep these defaults:

| Field | Default | Meaning |
|---|---|---|
| `commission_per_share`, `commission_min` | `0.005`, `1.00` | Commission per share, at least the minimum per fill |
| `commission_tiers` | none | Volume tiers, e.g. `[{"above": 300000, "per_share": "0.002"}]`: a fill pays the rate of the highest tier whose `above` the run's shares traded so far this calendar month have reached, like IBKR's tiered pricing |
| `slippage_bps` | `5` | Basis points against you on market, stop and auction fills and on limit orders already marketable at the open |
| `slippage_per_share` | `0` | A further amount per share against you on the same fills, e.g. `0.001` |
| `adv_participation`, `adv_sessions` | `0.05`, `20` | Volume cap: that fraction of the average daily volume of the last `adv_sessions` sessions, per symbol and session (universes always rank over 20 sessions, whatever this is); the rest of the order stays working. A symbol with no average daily volume fills uncapped and the run gets one data warning event for it |
| `borrow_rate` | `0.02` | Per year on short positions, charged at each session close |
| `financing_spread` | `0.015` | Added to the T-bill rate for the financing of negative cash: the US Treasury 3-month rate last published before the session (within 31 days). Without one the spread alone is charged and the run gets a data warning |

A cost model with a negative cost or tier price, an `adv_participation` outside 0 (excluded) to 1, or `adv_sessions` below 1 is refused, and slippage never moves a fill price below 0.0001. Shorting is allowed for instruments flagged shortable.

**Leverage.** Buying for more than the cash makes the cash negative, and selling short adds the proceeds to it. Each session close charges a `financing` ledger entry of negative cash × (T-bill rate + `financing_spread`) / 252, the same way borrow is charged on shorts, so a book that is flat before the close pays neither. Gross exposure is limited only by `max_gross_exposure` (below); set it to the book the strategy means, for example `4.0` for two dollars long and two short per dollar of equity. There are no margin calls: the drawdown stop is the only forced exit.

```toml
[risk]
max_gross_exposure = 4.0
```

```python
for symbol in cheap:
    ctx.order_target_weight(symbol, 2.0 / len(cheap), reason="long leg, 2x")
for symbol in rich:
    ctx.order_target_weight(symbol, -2.0 / len(rich), reason="short leg, 2x")
```

## Risk limits

A bot declares its limits in a `[risk]` section of `bot.toml`, each a positive fraction of equity (`max_gross_exposure` above 1 allows leverage) and each optional:

```toml
[risk]
max_position_weight = 0.3
max_gross_exposure = 1.0
max_drawdown = 0.2
```

They apply to every live run and are the default for backtests; the `risk` field of a backtest request replaces them for that run, and so do the limits in the UI's backtest dialog (in percent, empty for none) and `buizz backtest --risk max_drawdown=0.3`, which changes one limit and keeps the bot's others (`--risk max_drawdown=none` removes it).

 A run keeps the limits it started with: a live run captures them when it goes live, so after changing them stop the live run and go live again. An order that would grow a position past `max_position_weight`, or the book past `max_gross_exposure`, is rejected; while equity is at or below zero every order that grows a position breaks a limit that is set. Without limits nothing is rejected for exposure. `max_drawdown` is a stop-loss, the same in backtests and live runs: when equity falls by that fraction from its peak, the run records a risk breach, the bot gets `end` with reason `risk_limit` and no further orders are taken, working orders are cancelled (except a market-on-close already past its 10-minute cutoff, which still fills at that close) and every position is closed with a market order at the next open through the normal fills (slippage, commission and volume cap; what the cap leaves open is closed at the following opens). Once flat the run ends as `failed` with the message "stopped by drawdown limit"; a backtest ends there instead of running to its end date, and going live again starts a fresh run with a fresh peak. Its metrics, equity and ledger include the closing fills. To test a strategy without the stop, remove `max_drawdown`. Without a `[risk]` section a run has no limits.

## Holdout and honest data

- The last 12 months before today are a locked holdout. A backtest whose end date falls inside them is refused with a 409 unless the request sets `allow_holdout` (`buizz backtest --allow-holdout`) or uses the `holdout` preset. Holdout runs carry a `holdout` badge and are counted per bot.
- The presets `1y`, `3y`, `5y` and `10y` end the day before the holdout starts.
- The data gate refuses a backtest when a symbol has gaps in its daily coverage. An overnight move above 40% with no corporate action does not block the run; it is recorded as a data warning and the run gets the `data_warnings` badge.
- Runs with a fixed `symbols` universe always carry the `survivorship_biased` badge, because the list was picked with hindsight. A `top_dollar_volume` universe carries it until the store holds delisted instruments. An `index` universe carries it when a member's listing lacks a daily bar on a session of its membership in the run period, before that listing's last session (a company that stopped trading before the index removed it, as Staples did in September 2017, is not missing afterwards), and each such stretch is listed as a data warning. A stretch before 2016 stays missing until Tiingo's history reaches it.

### Index membership

`index = "sp500"` rebuilds S&P 500 membership from two Wikipedia articles, *List of S&P 500 companies* (current constituents) and *Historical components of the S&P 500* (the dated table of additions and removals), fetched daily through the MediaWiki API. Starting from today's members, each change is undone going back in time, matched by the company's article link and falling back to the ticker; within one date additions are undone before removals, so the order of a day's rows does not matter. A parse is refused, and the stored membership kept, when it gives fewer than 495 or more than 510 members today or on any day from the first reliable year on, or when its change log is more than a tenth shorter than the stored one; rows the parser cannot read are counted in the ingest job's message. The change log is incomplete before about 2011, so a run that starts before the first year from which every year lists at least 15 changes carries the `thin_index_history` badge: its early membership has stale members and misses some. Each member is listed under the ticker the change log names for it: an addition is matched to the member it opened by article and ticker, and a removal keeps the ticker in its row, so Ceridian is CDAY from its 2021 addition until it became DAY, and MasterBrand, whose row links its parent's article, does not take Fortune Brands' years. A member is priced from the listing of the company that traded under that ticker on the date, and through a name change during its membership into the new ticker (FBHS became FBIN four days before its removal); a member whose ticker a later company took is fetched as Alpaca mapped the ticker during the membership, and a member still without bars is reported as above. Wikipedia text is licensed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/); the membership is derived from the articles' contributors' work.
- Each run stores the bot content hash, engine version, seed, data watermark and, when completed or ended by the drawdown stop, a `result_hash` over everything the engine did. The same inputs give the same hash.

## Variants and sweeps

A variant is the bot's params with your overrides merged in, each coerced to the type of its default in `bot.toml` (a whole-number default takes `20` or `20.0` but refuses `20.5`). Sweep a grid to backtest every combination on one preset period before the holdout, with default costs and one shared seed:

```sh
buizz sweep bots/sma_cross --preset 10y --grid fast=20,50,100 --grid slow=100,200
```

The Sweep dialog under the bot's Research tab does the same. Variants already run on the same code and period are skipped, so repeating a sweep queues nothing. Research shows each preset's spread of Sharpe across variants against total-return SPY and the luck line (what the best of that many trials reaches with no edge at all), and suggests the plateau variant: the one passing the screening gate whose neighbours on the grid do best. The gate asks for a completed run with default costs, no data warnings or drawdown stop, at least 10 round trips and Sharpe above 0, and an edge over total-return SPY at the same risk: scaled to SPY's realised volatility (excess returns over T-bills multiplied up or down, leverage charged T-bill plus the financing spread), the variant must beat SPY's CAGR, draw down no deeper, and be ahead of SPY in at least 2 of 3 equal thirds of the period. Holding more or less market than SPY is not an edge. Go live with a variant from its backtest; a bot can run up to three live variants side by side. Every variant ever run counts as a trial, so a wide sweep raises the bar its best result has to clear.

## Headless loop

With a spine running (`just dev` for the fixture data, or one with real data):

```sh
cd sdk/python
export BUIZZ_ADMIN_PASSWORD=buizz-dev-admin
uv run --frozen buizz backtest ../../bots/red_green --from 2024-11-20 --to 2024-12-04
uv run --frozen buizz backtest ../../bots/sma_cross --from 2020-01-01 --to 2024-12-31 --param fast=20 --param slow=100 --risk max_drawdown=0.25
```

It queues the run, prints the clock and equity while it runs, then the metrics (`total_return`, `cagr`, `volatility`, `sharpe`, `sortino`, `max_drawdown`, `calmar`, `win_rate`, `profit_factor`, `exposure`, `turnover`, `alpha`, `beta`, `trades`, `sessions`; `-` when a metric is undefined for the run, e.g. `cagr` and `calmar` for runs shorter than 252 sessions, because annualizing a shorter return mostly magnifies noise, and `sharpe`, `sortino` and `calmar` for a run whose daily returns vary by less than 0.001%, such as one left in cash, whose ratio would only divide by rounding) and the orders by status with the net shares held per symbol after splits. The exit code is 1 when the run fails or is cancelled; a run ended by the drawdown stop still prints its metrics and orders, then `run <id> failed: stopped by drawdown limit`. The same run is in the UI with the equity curve and its drawdowns against the benchmark, the metrics beside the benchmark's, monthly returns, the closed trades with the reasons of the orders that opened and closed them, the orders, positions and the bot log. A trade is a round trip: it opens when a position leaves zero and closes when it returns to zero or flips side, and its P&L includes commissions, dividends and borrow. Bot output and tracebacks are in `logs/<run_id>.log` under the data directory.
