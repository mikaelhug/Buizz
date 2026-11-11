# Writing a bot

A bot is a folder under `bots/` with a `bot.toml` and a program. The spine starts the program, sends it one `step` per bar and collects its orders. The bot never sees the future: every answer is as of the run clock.

## Layout

```
bots/red_green/
  bot.toml
  bot.py
```

The spine rescans `bots/` on every request, so a new or edited bot shows up without a restart. A folder is a bot when it holds a `bot.toml`; folders starting with `.` are ignored. Each run records a content hash of the folder.

## Walkthrough: `red_green`

`bots/red_green/bot.toml`:

```toml
description = "Buys after a red day, exits after a green day."
command = ["uv", "run", "--frozen", "--project", "../../sdk/python", "python", "bot.py"]
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
            elif bar.close > bar.open:
                ctx.order_target_weight(symbol, 0, reason="green day")


if __name__ == "__main__":
    run(RedGreen)
```

`run(RedGreen)` reads `BUIZZ_SPINE_URL` and `BUIZZ_RUN_TOKEN` from the environment, connects, and calls `on_step` once per step until the spine ends the run. After a red day the bot asks for a 25% position; after a green day it asks for zero. Both orders fill at the next day's open.

## `bot.toml`

| Field | Required | Meaning |
|---|---|---|
| `description` | no | One line shown in the UI |
| `command` | yes | Argv of the program, not empty. Run with the bot folder as working directory |
| `timeframe` | yes | `"1d"` or `"1m"`: the bar size the bot is stepped on |
| `wake` | no, default `"bar"` | When the bot is called, see below |
| `universe` | yes | `{ symbols = ["SPY", "QQQ"] }` (not empty) or `{ top_dollar_volume = 500 }` (positive) |
| `params` | no | Default parameters, available as `ctx.params` |
| `benchmark` | no, default `"SPY"` | Symbol the equity curve is compared with |

The spine starts `command` with `BUIZZ_SPINE_URL`, `BUIZZ_RUN_ID` and `BUIZZ_RUN_TOKEN` set. Output on stdout and stderr is captured to `data/logs/<run_id>.log`.

**Universe.** `symbols` is a fixed list. `top_dollar_volume = N` selects the N symbols with the highest average dollar volume over the last 20 sessions, recomputed as of each session; the step carries `universe` whenever it changed and `ctx.universe` follows.

**Wake.** Every bar reaches the bot, but it is only stepped when its wake condition holds; bars that became visible between wakes arrive together at the next step (`ctx.bars` keeps the last bar per symbol).

- `bar`: every bar. The only option for `1d` bots.
- `every:30m` or `every:2h`: when the time since the session open is a positive multiple of the interval, up to the close.
- `at:15:55`: when the exchange-local (New York) time is 15:55.

**Params.** The values in `bot.toml` are defaults. A backtest request, or `buizz backtest --param weight=0.5`, overrides single keys; the merged object is stored with the run.

## SDK

```python
from buizz import Bot, Context, run
```

A bot subclasses `Bot` and implements `on_step(ctx)`. `on_start(ctx)` runs once after the spine's `ready` (only `params`, `universe`, `state`, `portfolio` and `position` are usable there; bars, `clock` and queries are not) and `on_end(ctx, reason)` runs when the spine ends the run (`completed`, `paused`, `stopped`, `risk_limit` or `failed`).

### Reading

| Member | Meaning |
|---|---|
| `ctx.params` | The merged params dict |
| `ctx.clock` | The step time as a UTC `datetime`: the session close for a daily bar, one minute after the start for a 1m bar |
| `ctx.session` | `Session(date, open, close)` of the current session |
| `ctx.universe` | Current symbols |
| `ctx.bars` | `dict[symbol, Bar]` (`open`, `high`, `low`, `close`, `volume` floats): the latest bar of each symbol that became visible since the previous step |
| `ctx.events` | Fills, order status changes and corporate actions since the previous step |
| `ctx.portfolio` | `cash`, `equity`, `positions` (`qty`, `avg_cost`) and `working` orders as of the start of the step. Orders you place in this step are not reflected |
| `ctx.position(symbol)` | The `Position`, with `qty == 0` when flat |
| `ctx.history(symbols, tf="1d", bars=1, adjust="raw")` | The last `bars` (1 to 10000) bars per symbol visible at the clock, as numpy arrays (`start` in ns, `open`, `high`, `low`, `close`, `volume`). Each call is a round trip to the spine |
| `ctx.fundamentals(symbols, metrics)` | Every fundamental fact visible at the clock for the given metrics |
| `ctx.state` | A dict that survives restarts, see below |

`adjust` is `raw`, `split` or `all` (splits and dividends). Adjustment only uses corporate actions whose ex-date session has opened by the clock, so history never reflects a split that has not happened yet. Fills always use raw prices.

Fundamentals are snake_case metrics: `revenue`, `cost_of_revenue`, `gross_profit`, `research_development`, `sga_expense`, `operating_expenses`, `operating_income`, `interest_expense`, `pretax_income`, `income_tax`, `net_income`, `eps_diluted`, `total_assets`, `current_assets`, `cash_and_equivalents`, `inventory`, `receivables`, `ppe_net`, `goodwill`, `current_liabilities`, `long_term_debt`, `total_liabilities`, `equity`, `operating_cash_flow`, `depreciation_amortization`, `stock_based_comp`, `capex`, `investing_cash_flow`, `financing_cash_flow`, `dividends_paid`, `share_repurchases`, `acquisitions`, `shares_outstanding`. A `FundamentalPoint` carries `period_type` (`FY` or `Q`), `period_start`, `period_end`, `value`, `form`, `filed` and `available_at`; the latest filing per period wins. A fact becomes visible at the open of the first session after its `filed` date, never earlier. `bots/value_quality` is a complete example.

### Acting

```python
ctx.order(symbol, qty, *, reason, limit_price=None, client_id=None) -> str
ctx.order_target_weight(symbol, weight, *, reason, limit_price=None, client_id=None) -> str
ctx.cancel(client_id)
ctx.log(message, level="info")
```

- `order` takes a signed share count: positive buys, negative sells.
- `order_target_weight` targets a fraction of equity: the target is `trunc(weight * equity / last close)` shares and the order is the difference to the position plus the session's working orders. A negative weight shorts. A target that needs no change is recorded as rejected with `zero_quantity`.
- Quantities are whole shares. Splits pay cash in lieu of fractions.
- Without `limit_price` the order is a market order. All orders are day orders.
- `reason` is required free text. It is stored with the order and shown in the order list: write the decision, not the action.
- `client_id` is returned. Leave it out to get `<symbol>-<session date>` (daily) or `<symbol>-<YYYY-MM-DDTHH:MM>` (1m), with `-1`, `-2` for repeats in the same step. A client id is unique per run and a repeated one is ignored, which makes replaying an ack after a crash safe.
- `cancel` cancels a working order by client id.
- `log` writes a line of the given level (`debug`, `info`, `warn`, `error`) into the run's event log; the bot's stdout and stderr go to the log file.

Orders are rejected, with the reason on the order and in the run events, for: `unknown_symbol`, `zero_quantity`, `invalid_limit_price`, `no_price` (no close seen yet), `not_shortable`, `max_position_weight`, `max_gross_exposure` and `run_paused`.

### State

`ctx.state` is a dict that must stay JSON-encodable. It is saved with every step and handed back when the run resumes, so a live bot that restarts continues where it was. Encoded it may not exceed 64 KiB; more raises `StateTooLarge`. Keep only what cannot be recomputed from `ctx.history`.

## Time and fills

- A bar is visible when it is complete: a 1m bar one minute after its start, a daily bar at the session close, a fundamental fact at the next session open after `filed`, a corporate action at the open of its ex-date session.
- The bot is stepped when a bar is visible. An order decided on bar N fills no earlier than bar N+1's open; there is no way to trade on the bar you are looking at.
- A market order fills at the open of the next bar, moved against you by the slippage. A limit buy fills at the open if the open is at or below the limit, otherwise at the limit if the bar's low touches it; a limit sell mirrors that. Otherwise it stays working until its session closes and then expires.
- A day order belongs to the session of the first bar that starts at or after the decision, so an order placed at a daily close fills in the next session, and so does one placed after the last minute of a session.
- Splits and cash dividends hit the position at the ex-date open.
- A delisted symbol's position is liquidated at its last close and its orders are cancelled.

## Costs

The spine starts every run with 100,000 start cash and this cost model:

| Cost | Default |
|---|---|
| Commission | 0.005 per share, at least 1.00 per fill |
| Slippage | 5 bps against you on market fills |
| Volume cap | 5% of the average daily volume of the last 20 sessions, per symbol and session; the rest of the order stays working and expires with its session |
| Borrow | 2% per year on short positions, charged at each session close |

Shorting is allowed for instruments flagged shortable.

## Risk limits

A run can carry `max_position_weight`, `max_gross_exposure` and `max_drawdown` (fractions). None is set by default; they are set per run through the `risk` field of the backtest request of the HTTP API. An order that would grow a position past `max_position_weight`, or the book past `max_gross_exposure`, is rejected. Falling past `max_drawdown` from the peak equity records a risk breach and pauses the run with end reason `risk_limit`.

## Holdout and honest data

- The last 12 months before today are a locked holdout. A backtest whose end date falls inside them is refused with a 409 unless the request sets `allow_holdout` (`buizz backtest --allow-holdout`) or uses the `holdout` preset. Holdout runs carry a `holdout` badge and are counted per bot.
- The presets `1y`, `3y`, `5y` and `10y` end the day before the holdout starts.
- The data gate refuses a backtest when a symbol has gaps in its daily coverage. An overnight move above 40% with no corporate action does not block the run; it is recorded as a data warning and the run gets the `data_warnings` badge.
- Runs with a fixed `symbols` universe always carry the `survivorship_biased` badge, because the list was picked with hindsight. A `top_dollar_volume` universe carries it until the store holds delisted instruments.
- Each run stores the bot content hash, engine version, seed, data watermark and, when completed, a `result_hash` over everything the engine did. The same inputs give the same hash.

## Headless loop

With a spine running (`just dev` for the fixture data, or one with real data):

```sh
cd sdk/python
export BUIZZ_ADMIN_PASSWORD=dev
uv run --frozen buizz backtest ../../bots/red_green --from 2024-11-20 --to 2024-12-04
uv run --frozen buizz backtest ../../bots/sma_cross --from 2020-01-01 --to 2024-12-31 --param fast=20 --param slow=100
```

It queues the run, prints the clock and equity while it runs, then the metrics (`total_return`, `cagr`, `volatility`, `sharpe`, `sortino`, `max_drawdown`, `calmar`, `win_rate`, `profit_factor`, `exposure`, `turnover`, `alpha`, `beta`, `trades`, `sessions`; `-` when a metric is undefined for the run) and the orders by status with the net shares held per symbol after splits. The exit code is 1 when the run fails or is cancelled. The same run is in the UI with the equity curve against the benchmark, the orders with their reasons, positions and the bot log. Bot output and tracebacks are in `data/logs/<run_id>.log`.
