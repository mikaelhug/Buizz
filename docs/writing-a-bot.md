# Writing a bot

A bot is a folder under `bots/` with a `bot.toml` and a program. The spine starts the program, sends it one `step` per bar and collects its orders. The bot never sees the future: every answer is as of the run clock.

## Layout

```
bots/red_green/
  bot.toml
  bot.py
```

The spine rescans `bots/` on every request, so a new or edited bot shows up without a restart. A folder is a bot when it holds a `bot.toml`; folders starting with `.` are ignored. Each run records a content hash of the folder and never continues under different code: when a backtest is (re)started after a restart, or a live run is started or resumed, with edited bot code, the backtest fails and the live run pauses with the reason. Stop it and start a new run.

## Python environment and shared code

Bots run in one shared uv project, `bots/pyproject.toml` (package `buizz-bots`), selected by `--project ..` in each `command`. It depends on the SDK and on `hmmlearn`, `lightgbm`, `scikit-learn` and `scipy`, all pinned in `bots/uv.lock`; the SDK project itself stays lean (`msgspec`, `numpy`, `websockets`). To add a dependency, put it in `bots/pyproject.toml` and run `cd bots && uv lock`; `just setup` installs the environment with `uv sync --frozen`. The Docker image builds the same locked environment at image build time (non-editable, bytecode compiled, `libgomp1` for LightGBM), so a bot starts offline from a read-only filesystem. On macOS LightGBM needs OpenMP: `brew install libomp`.

Code that more than one bot needs lives in the package `bots/buizz_bots/` and is imported as `from buizz_bots.<module> import ...`, for example a bot reusing the signal functions of another. Move the functions into a module there, import them from both bots, and do not copy them. The folder has no `bot.toml`, so it is not a bot. It is installed into the bots environment: editable locally, copied into the image. Shared modules follow the same lint and strict type checks as bots (`just lint`) and must not import a bot's `bot.py`.

The content hash of a run covers the bot folder plus the shared package `bots/buizz_bots/`, `bots/pyproject.toml` and `bots/uv.lock`, so editing shared code or dependencies changes the hash of every bot, and a live or resumed run whose code changed is stopped instead of continuing under the old hash.

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
| `params` | no | Default parameters, available as `ctx.params` |
| `benchmark` | no, default `"SPY"` | Symbol the equity curve is compared with |
| `[risk]` | no | Risk limits for live runs and the default for backtests, see [Risk limits](#risk-limits) |

The spine starts `command` with `BUIZZ_SPINE_URL`, `BUIZZ_RUN_ID` and `BUIZZ_RUN_TOKEN` set. Of the spine's own environment the bot only inherits `PATH`, `HOME`, `LANG`, `TZ`, `TMPDIR` and the `UV_*` and `PYTHON*` variables, so API keys and the admin password never reach it. Output on stdout and stderr is captured to `logs/<run_id>.log` under the spine's data directory (`BUIZZ_DATA_DIR`, default `data`).

**Universe.** `symbols` is a fixed list. `top_dollar_volume = N` selects the N symbols with the highest average dollar volume over the last 20 sessions, recomputed as of each session; the step carries `universe` whenever it changed and `ctx.universe` follows. With `asset_class = "stock"` only stocks enter that ranking, so cross-sections are not crowded by SPY, IVV, VOO or SHV; a fund is any ticker on the SEC's mutual fund and ETF list, and without SEC EDGAR configured every instrument counts as a stock. The filter only shapes the universe: a bot may still trade and query any instrument, so an ETF hedge stays tradable.

`index = "sp500"` is the S&P 500 as of each session, rebuilt from Wikipedia's current constituents and its change log (see [Index membership](#index-membership)), limited to members with daily bars in the last 20 sessions.

**Wake.** Every bar reaches the bot, but it is only stepped when its wake condition holds; bars that became visible between wakes arrive together at the next step (`ctx.bars` keeps the last bar per symbol).

- `bar`: every bar. The only option for `1d` bots.
- `every:30m` or `every:2h`: when the time since the session open is a positive multiple of the interval, up to the close.
- `at:15:55`: when the exchange-local (New York) time is 15:55.

**Params.** The values in `bot.toml` are defaults. A backtest request, or `buizz backtest --param weight=0.5`, overrides single keys; the merged object is stored with the run.

## SDK

```python
from buizz import Bot, Context, run
```

A bot subclasses `Bot` and implements `on_step(ctx)`. `on_start(ctx)` runs once after the spine's `ready` and `on_end(ctx, reason)` once when the run ends for this process. `reason` is an `EndReason`: `completed`, `paused`, `stopped`, `risk_limit` (the drawdown stop, see [Risk limits](#risk-limits)) or `failed` as the spine sent it, or `disconnected` when the connection went away without one. A pause or stop can arrive in the middle of `on_step`, while it waits on a query or as the SIGTERM that stops the process: the step is abandoned (its orders and state changes are not sent) and `on_end` still runs, with about 10 seconds before the process is killed.

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
| `ctx.portfolio` | `cash`, `equity`, `positions` (`qty`, `avg_cost`) and `working` orders (`WorkingOrder`: `client_id`, `symbol`, `side`, `qty`, `filled_qty` and `order_type`, a `Market()` or `Limit(limit_price)`) as of the start of the step. Orders you place in this step are not reflected |
| `ctx.position(symbol)` | The `Position`, with `qty == 0` when flat |
| `ctx.history(symbols, tf="1d", bars=1, adjust="raw")` | The last `bars` (1 to 10000) bars per symbol visible at the clock, as numpy arrays (`start` in ns, `open`, `high`, `low`, `close`, `volume`). Each call is a round trip to the spine |
| `ctx.fundamentals(symbols, metrics)` | Every fundamental fact visible at the clock for the given metrics (`Metric` names, below) |
| `ctx.instruments(symbols)` | `dict[symbol, InstrumentProfile]` (`name`, `exchange`, `asset_class`, `sic`, `sic_description`) for the known symbols. Current values, see below |
| `ctx.earnings(symbols)` | `list[EarningsRelease]` (`symbol`, `cik`, `accession`, `form`, `filed`, `report_date`, `accepted_at`): every earnings release accepted by EDGAR at or before the clock |
| `ctx.universe_at(on, top_dollar_volume, *, asset_class=None, index=None)` | The symbols a `top_dollar_volume` universe of that size (optionally of one `asset_class`), or the `index` universe, held on the date `on`, see below |
| `ctx.macro(series, start, end)` | The rate and macro observations dated `start` to `end` as known at the clock (`MacroPoint`: `series`, `date`, `value`, `published`, `available_at`), see below |
| `ctx.calendar(start, end)` | The exchange sessions (`Session(date, open, close)`) dated `start` to `end`, as the calendar was published at the clock, see below |
| `ctx.state` | A dict that survives restarts, see below |

`tf`, `bars` and `adjust` are checked before the query is sent: a wrong value raises `ValueError` or `TypeError` in the step. `adjust` is `raw`, `split` or `all` (splits and dividends). Adjustment only uses corporate actions whose ex-date session has opened by the clock, so history never reflects a split that has not happened yet. Fills always use raw prices.

Fundamentals are snake_case metrics, typed as `Metric` (an unknown name raises `ValueError`): `revenue`, `cost_of_revenue`, `gross_profit`, `research_development`, `sga_expense`, `operating_expenses`, `operating_income`, `interest_expense`, `pretax_income`, `income_tax`, `net_income`, `eps_diluted`, `total_assets`, `current_assets`, `cash_and_equivalents`, `inventory`, `receivables`, `ppe_net`, `goodwill`, `current_liabilities`, `long_term_debt`, `total_liabilities`, `equity`, `operating_cash_flow`, `depreciation_amortization`, `stock_based_comp`, `capex`, `investing_cash_flow`, `financing_cash_flow`, `dividends_paid`, `share_repurchases`, `acquisitions`, `shares_outstanding`, `short_term_debt`, `short_term_borrowings`, `current_long_term_debt`, `long_term_debt_noncurrent`, `preferred_stock`, `minority_interest`, `retained_earnings`, `income_taxes_payable`, `short_term_investments`, `convertible_debt`, `secured_debt`, `land`, `buildings`, `ppe_gross`, `advertising`. A `FundamentalPoint` carries `period_type` (`FY` or `Q`), `period_start`, `period_end`, `value`, `form`, `filed` and `available_at`; the latest filing per period wins. A fact becomes visible at the open of the first session after its `filed` date, never earlier. Share counts such as `shares_outstanding` are as reported, before any later split: `split_ratio(raw, split_adjusted, day)` takes a raw and a split-adjusted `ctx.history` of the same symbol and returns how many shares today one share on `day` has become (`None` when the history does not reach back to `day`). `bots/value_quality` is a complete example.

Each metric is one reported XBRL concept or a list of concepts with the same meaning, tried in order; a value is never computed from other concepts, so a filer that tags none of them has no point for that metric. `long_term_debt` takes the noncurrent part when the filer tags it and otherwise the total that may include current maturities; `long_term_debt_noncurrent` is strictly the noncurrent part. `short_term_debt` is the filer's total of short-term borrowings and current maturities (`DebtCurrent`); filers that tag only the parts report them as `short_term_borrowings` and `current_long_term_debt`. `preferred_stock` is the carrying value in equity, `minority_interest` the noncontrolling interest in equity, `income_taxes_payable` current income taxes only, `short_term_investments` the current marketable securities line. `convertible_debt`, `secured_debt`, `land` and `buildings` are often tagged only inside a breakdown table, which SEC company facts leave out, so they are missing for many filers: a missing value means not reported, never zero. The number of employees is not offered: hardly any filer tags it.

`ctx.instruments` answers with today's values, not the values as of the clock. `exchange` is Alpaca's current listing exchange (`NYSE`, `NASDAQ`, `ARCA`, `AMEX`, `BATS`); `sic` is the four-digit Standard Industrial Classification code from the company's EDGAR submissions, `None` for funds and for tickers without a CIK. Neither source keeps a history: EDGAR shows only a company's current SIC code and Alpaca only the current exchange, so a company that moved from NASDAQ to NYSE, or changed industry, carries its current value back in time. Both change rarely; a bot sorting on them should say so in its results.

An earnings release is an 8-K (or 8-K/A) whose items include 2.02, *Results of Operations and Financial Condition*, the filing companies make when they publish quarterly results. It is visible from its EDGAR acceptance time, to the second, which is when it became public on EDGAR; a release accepted after the close is therefore visible on the next step. `report_date` is the event date the filer gave, usually the day of the announcement. Funds and tickers without a CIK have none.

**Calendar.** The exchange publishes its holidays and early closes years ahead, so `ctx.calendar` answers future sessions too, up to 366 days after the clock's date; a later `end` raises `SpineError` with code `bad_query`. A closure decided at short notice (the national days of mourning on 2018-12-05 and 2025-01-09, the first Juneteenth closure on 2022-06-20) stays the regular session it was scheduled as until the open of the first session after its announcement, so a backtest never knows about it earlier than the market did. A daily bot finds the last session of the month or week from the next session:

```python
from datetime import timedelta

today = ctx.session.date
following = ctx.calendar(today, today + timedelta(days=10))[1].date
month_end = following.month != today.month
week_end = following.isocalendar().week != today.isocalendar().week
```

**Past universes.** `ctx.universe_at(on, n)` applies the `top_dollar_volume = n` rule as of the date `on`: the `n` symbols with the highest dollar volume over the 20 sessions before it, without those delisted before it, highest first. `asset_class="stock"` ranks stocks only, and `ctx.universe_at(on, index="sp500")` gives the S&P 500 members on `on` instead; both resolve exactly as a run's `universe` of the same form does on that date. It answers once the last session before `on` has closed, so any date up to the next session works and a later one raises `SpineError` with code `bad_query`. A model that trains on years of cross-sections asks for each formation date in its first step:

```python
from datetime import date

months = [date(year, month, 1) for year in range(2017, 2024) for month in range(1, 13)]
training = {month: ctx.universe_at(month, 1000) for month in months}
```

**Rates and macro series.** `ctx.macro(series, start, end)` returns the observations of `MacroSeries` dated `start` to `end`, each as the latest vintage published before the current session, so revisions arrive when they were published and never earlier. `available_at` is the open of the first session after `published`. Values are `Decimal` percent, except the CPI index.

| Series | Meaning | Frequency |
|---|---|---|
| `treasury_3m` | US Treasury 3-month par yield, the rate the metrics use; published the evening of its date | daily |
| `tb3ms` | 3-month T-bill, secondary market (FRED `TB3MS`), Welch–Goyal `tbl` | monthly average, dated the 1st |
| `gs10` | 10-year Treasury, constant maturity (FRED `GS10`) | monthly average |
| `aaa`, `baa` | Moody's seasoned Aaa and Baa corporate bond yields (FRED `AAA`, `BAA`), Graham's AAA rule and Welch–Goyal `dfy = baa − aaa` | monthly average |
| `cpiaucns` | CPI-U, all items, not seasonally adjusted, 1982–84 = 100 (FRED `CPIAUCNS`) | monthly |

The FRED series need `FRED_API_KEY` on the spine; without it they are empty. Graham's CPI-scaled size floor:

```python
from datetime import date

year = ctx.session.date.year - 1
cpi = ctx.macro(["cpiaucns"], date(year, 1, 1), date(year, 12, 1))
if len(cpi) == 12:
    floor = 100_000_000 * float(sum(point.value for point in cpi) / 12) / 41.8
```

`to_ns(moment)` turns a timezone-aware `datetime` into the nanoseconds of `History.start`, for example `history.start >= to_ns(ctx.session.open)`.

### Acting

```python
ctx.order(symbol, qty, *, reason, limit_price=None, client_id=None) -> str
ctx.order_target_weight(symbol, weight, *, reason, limit_price=None, client_id=None) -> str
ctx.rebalance(weights, *, exit_reason, reason=None)
ctx.cancel(client_id)
ctx.log(message, level="info")
```

- `order` takes a signed share count as an `int` (numpy integers too): positive buys, negative sells. A float raises `TypeError`; round it yourself.
- `order_target_weight` targets a fraction of equity: the target is `trunc(weight * equity / last close)` shares and the order is the difference to the position plus the session's working orders. A negative weight shorts. A target that needs no change is recorded as rejected with `zero_quantity`. Weights and limit prices must be finite numbers; NaN or infinity raises `ValueError`.
- `rebalance` makes the book these weights: it cancels every working order, targets each weight and sends every other held symbol to zero with `exit_reason`. `reason` is one string, or a mapping with a reason per symbol of `weights`. A zero weight for a symbol you do not hold places nothing, so `ctx.rebalance({}, exit_reason="flat before the close")` flattens the book.
- Quantities are whole shares. Splits pay cash in lieu of fractions.
- Without `limit_price` the order is a market order. A limit price is rounded to 4 decimals away from the market (down for a buy, up for a sell). All orders are day orders.
- `reason` is required free text. It is stored with the order and shown in the order list: write the decision, not the action.
- `client_id` is returned. Leave it out to get `<symbol>-<session date>` (daily) or `<symbol>-<YYYY-MM-DDTHH:MM>` (1m), with `-1`, `-2` for repeats in the same step. A client id is unique per run; a repeated one is rejected with `duplicate_client_id` (in the step events and run events; the run's order list keeps only the first) and leaves the first order alone, which makes replaying an ack after a crash safe.
- `cancel` cancels a working order by client id.
- `log` writes a line of the given level (`debug`, `info`, `warn`, `error`) into the run's event log; the bot's stdout and stderr go to the log file.

Orders are rejected, with the reason on the order and in the run events, for: `unknown_symbol`, `duplicate_client_id`, `zero_quantity`, `invalid_limit_price`, `no_price` (no close seen yet), `not_shortable`, `max_position_weight`, `max_gross_exposure`, `invalid_size` (a quantity or target weight too large to compute) and, for live `1m` runs, `no_stream_slot` (the symbol is outside the universe and holdings and the 30-symbol stream has no free slot for it).

### State

`ctx.state` is a dict saved with every step and handed back when the run resumes, so a live bot that restarts continues where it was. It holds only `dict` with `str` keys, `list`, `str`, `int`, `float` (finite), `bool` and `None`, so that it reads back exactly as it was written; anything else (a tuple, a date, a numpy number, an int key) raises `TypeError` or `ValueError` when the step ends. Encoded it may not exceed 64 KiB; more raises `StateTooLarge`. Keep only what cannot be recomputed from `ctx.history`.

## Time and fills

- A bar is visible when it is complete: a 1m bar one minute after its start, a daily bar at the session close, a fundamental fact at the next session open after `filed`, a corporate action at the open of its ex-date session.
- The bot is stepped when a bar is visible. An order decided on bar N fills no earlier than bar N+1's open; there is no way to trade on the bar you are looking at.
- A market order fills at the open of the next bar, moved against you by the slippage. A limit buy fills at the open moved against you by the slippage, but never above the limit, if the open is at or below the limit; otherwise at the limit if the bar's low touches it. A limit sell mirrors that. Otherwise it stays working until its session closes and then expires.
- A day order belongs to the session of the first bar that starts at or after the decision, so an order placed at a daily close fills in the next session, and so does one placed after the last minute of a session.
- Splits and cash dividends hit the position at the ex-date open.
- A delisted symbol's position is liquidated at its last close and its orders are cancelled.

## Costs

The spine starts every run with 100,000 start cash and this cost model:

| Cost | Default |
|---|---|
| Commission | 0.005 per share, at least 1.00 per fill |
| Slippage | 5 bps against you on market fills and on limit orders already marketable at the open |
| Volume cap | 5% of the average daily volume of the last 20 sessions, per symbol and session; the rest of the order stays working and expires with its session. A symbol with no average daily volume fills uncapped and the run gets one data warning event for it |
| Borrow | 2% per year on short positions, charged at each session close |

Shorting is allowed for instruments flagged shortable.

## Risk limits

A bot declares its limits in a `[risk]` section of `bot.toml`, each a positive fraction and each optional:

```toml
[risk]
max_position_weight = 0.3
max_gross_exposure = 1.0
max_drawdown = 0.2
```

They apply to every live run and are the default for backtests; the `risk` field of a backtest request replaces them for that run. A run keeps the limits it started with: a live run captures them when it goes live, so after changing them stop the live run and go live again. An order that would grow a position past `max_position_weight`, or the book past `max_gross_exposure`, is rejected; while equity is at or below zero every order that grows a position breaks a limit that is set. Without limits nothing is rejected for exposure. `max_drawdown` is a stop-loss, the same in backtests and live runs: when equity falls by that fraction from its peak, the run records a risk breach, the bot gets `end` with reason `risk_limit` and no further orders are taken, working orders are cancelled and every position is closed with a market order at the next open through the normal fills (slippage, commission and volume cap; what the cap leaves open is closed at the following opens). Once flat the run ends as `failed` with the message "stopped by drawdown limit"; a backtest ends there instead of running to its end date, and going live again starts a fresh run with a fresh peak. Its metrics, equity and ledger include the closing fills. To test a strategy without the stop, remove `max_drawdown`. Without a `[risk]` section a run has no limits.

## Holdout and honest data

- The last 12 months before today are a locked holdout. A backtest whose end date falls inside them is refused with a 409 unless the request sets `allow_holdout` (`buizz backtest --allow-holdout`) or uses the `holdout` preset. Holdout runs carry a `holdout` badge and are counted per bot.
- The presets `1y`, `3y`, `5y` and `10y` end the day before the holdout starts.
- The data gate refuses a backtest when a symbol has gaps in its daily coverage. An overnight move above 40% with no corporate action does not block the run; it is recorded as a data warning and the run gets the `data_warnings` badge.
- Runs with a fixed `symbols` universe always carry the `survivorship_biased` badge, because the list was picked with hindsight. A `top_dollar_volume` universe carries it until the store holds delisted instruments. An `index` universe carries it when a member has no daily bars during its membership in the run period, and each such member is listed as a data warning.

### Index membership

`index = "sp500"` rebuilds S&P 500 membership from two Wikipedia articles, *List of S&P 500 companies* (current constituents) and *Historical components of the S&P 500* (the dated table of additions and removals), fetched daily through the MediaWiki API. Starting from today's members, each change is undone going back in time, matched by the company's article link and falling back to the ticker. The change log is incomplete before about 2011, so a run that starts before the first year from which every year lists at least 15 changes carries the `thin_index_history` badge: its early membership has stale members and misses some. Delisted members keep their prices when Alpaca still serves bars under their old ticker; a member it does not is reported as above. Wikipedia text is licensed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/); the membership is derived from the articles' contributors' work.
- Each run stores the bot content hash, engine version, seed, data watermark and, when completed or ended by the drawdown stop, a `result_hash` over everything the engine did. The same inputs give the same hash.

## Headless loop

With a spine running (`just dev` for the fixture data, or one with real data):

```sh
cd sdk/python
export BUIZZ_ADMIN_PASSWORD=buizz-dev-admin
uv run --frozen buizz backtest ../../bots/red_green --from 2024-11-20 --to 2024-12-04
uv run --frozen buizz backtest ../../bots/sma_cross --from 2020-01-01 --to 2024-12-31 --param fast=20 --param slow=100
```

It queues the run, prints the clock and equity while it runs, then the metrics (`total_return`, `cagr`, `volatility`, `sharpe`, `sortino`, `max_drawdown`, `calmar`, `win_rate`, `profit_factor`, `exposure`, `turnover`, `alpha`, `beta`, `trades`, `sessions`; `-` when a metric is undefined for the run, e.g. `cagr` and `calmar` for runs shorter than 252 sessions, because annualizing a shorter return mostly magnifies noise) and the orders by status with the net shares held per symbol after splits. The exit code is 1 when the run fails or is cancelled; a run ended by the drawdown stop still prints its metrics and orders, then `run <id> failed: stopped by drawdown limit`. The same run is in the UI with the equity curve against the benchmark, the orders with their reasons, positions and the bot log. Bot output and tracebacks are in `logs/<run_id>.log` under the data directory.
