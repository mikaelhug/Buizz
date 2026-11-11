# Bot protocol v1

The types are `buizz_core::protocol`; golden frames for every message are in `fixtures/protocol/` (`*.msgpack` is normative, `*.json` is a readable rendering with binaries as hex).

## Transport

- The supervisor starts the bot with `BUIZZ_SPINE_URL` (the address the spine is bound to, loopback when it binds all interfaces, e.g. `ws://127.0.0.1:8080`), `BUIZZ_RUN_ID` and `BUIZZ_RUN_TOKEN`.
- The bot opens a WebSocket to `$BUIZZ_SPINE_URL/bot/v1` with `Authorization: Bearer $BUIZZ_RUN_TOKEN`. A bad token is refused at the upgrade with 401, and a connection from anywhere but the spine's own host (loopback or the address the spine is bound to) with 403.
- Every frame is one binary WebSocket message holding one MessagePack map with a `type` key. Structs are maps with named fields; unknown fields must be ignored.
- Encoding: timestamps are `int64` nanoseconds since the Unix epoch, UTC. Dates are `"YYYY-MM-DD"` strings. Money, prices, quantities as weights and rates are decimal strings (`"49.2246"`); share quantities are integers. Enums are snake_case strings.

## Conversation

```
bot   -> hello {protocol: 1, sdk}
spine -> ready {run_id, bot, mode, timeframe, wake, params, universe, start, end, seed, state, cash, equity, holdings}
loop:
  spine -> step {seq, clock, session, universe?, bars, events, cash, equity, holdings?}
  bot   -> query {id, request}      zero or more, each answered before the next
  spine -> data {id, result} | error {id, code, message}
  bot   -> ack {seq, orders, cancels, state?, logs}
spine -> end {reason, message?}
```

- Strict alternation: after a `step` the spine only answers queries until it receives the `ack` with the same `seq`. A second `hello`, an ack with the wrong `seq` or any frame outside this order ends the run with `error {code: protocol_violation}`.
- `error` with `id: null` is fatal and is followed by the socket closing; with an `id` it answers that query and the step continues.
- Identical in backtest and live. In live, bars that become visible while the bot is deciding coalesce into the next step: `bars` then holds several rows per symbol.
- `end` reasons: `completed`, `paused`, `stopped`, `risk_limit`, `failed`. `risk_limit` is the drawdown stop: the spine closes every position without the bot and the run ends as failed. On `paused` the process is stopped; resuming starts a new connection that receives `ready` with the saved `state` and current holdings, never the paused interval.

## Step

- `clock`: the run time of this step. Everything in the frame and every query answer is as-of `clock`.
- `bars`: a columnar buffer of the bars that became visible since the previous step, ordered by `(start, symbol)`:
  - `symbols`: the strings referenced by `symbol_index`.
  - `symbol_index`: little-endian `uint16` per row; `start`: little-endian `int64` ns per row (bar start); `open`, `high`, `low`, `close`, `volume`: little-endian `float64` per row.
  - In numpy: `np.frombuffer(bars["close"], "<f8")`.
- `events`: what happened since the previous step: `fill`, `order` (status changes incl. `rejected` with `reject_reason`, `expired`, `cancelled`) and `corporate_action` (applied at the ex-date open).
- `cash`, `equity`: always present. `holdings` (`positions` with `qty` and `avg_cost`, `working` orders) is sent on the first step and whenever it changed, otherwise `null`; the bot keeps its last copy and prices positions with the bars.
- `universe`: present when the universe changed (`top_dollar_volume` and `index` universes are recomputed as-of each session).

## Queries

- `history {symbols, timeframe, bars, adjust}`: the last `bars` (1..10000) bars per symbol visible at `clock`, as a bar buffer. `adjust` is `raw`, `split` or `all` (splits and dividends), using only corporate actions whose ex-date session has opened by `clock`.
- `fundamentals {symbols, metrics}`: every fact visible at `clock` for those canonical metrics (the latest filing per period wins), as `points`.
- `instruments {symbols}`: `instruments`, one `{symbol, name, exchange, asset_class, sic, sic_description}` per known symbol. These are the instrument's current values, not as of `clock` (see [Writing a bot](writing-a-bot.md#reading)).
- `earnings {symbols}`: `releases`, every 8-K or 8-K/A carrying Item 2.02 (results of operations) whose EDGAR acceptance time is at or before `clock`: `{symbol, cik, accession, form, filed, report_date, accepted_at}` with `accepted_at` in ns.

## Ack

- `orders`: `{client_id, symbol, size, order_type, tif, reason}`. `size` is `{by: "qty", qty}` (signed: negative sells) or `{by: "target_weight", weight}` (sized against equity and the last close at `clock`). `order_type` is `{type: "market"}` or `{type: "limit", limit_price}`; the spine rounds `limit_price` to 4 decimals away from the market (down for a buy, up for a sell), so a limit never fills worse than the bot asked. `tif` is `day`. `client_id` must be unique per run; a repeat is rejected with `duplicate_client_id` and leaves the first order alone, which makes acks safe to replay after a crash.
- `cancels`: client ids of working orders.
- `state`: opaque bytes ≤ 64 KiB persisted with the step checkpoint and returned in `ready`; `null` keeps the previous state.
- `logs`: `{level, message}` lines for the run log.

## Time rules

- Visibility: a 1m bar at `start + 60 s`; a daily bar at the session close; a fundamental fact at the open of the first session after its `filed` date; an earnings release at its EDGAR acceptance time; a corporate action at the open of its ex-date session.
- Each bar runs one cycle: order expiry, corporate actions, fills (at the bar's start), mark, risk limits, wake the bot, accept orders (at the bar's visible time). A bar's cycle finishes before the next bar's begins, so an order accepted on bar N fills no earlier than bar N+1's open.
- A day order belongs to the session of the first bar starting at or after its decision and expires at that session's close.
- `wake`: `bar` wakes on every bar; `every:30m` when `clock − session open` is a positive multiple of 30 minutes, up to the close; `at:15:55` when the exchange-local clock reads 15:55. Bars visible between wakes still arrive, coalesced, at the next step.

## Throughput

The Python SDK drives the socket with the `websockets` sans-I/O `ClientProtocol` over a blocking socket on one thread: it is the transport that clears 20k no-op lockstep steps/s for 50 symbols of 1m bars (asyncio stays near 15k).
