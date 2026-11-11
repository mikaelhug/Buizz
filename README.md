# Buizz

Buizz tests trading strategies against historic and live US stock and ETF data, paper only. A strategy is a small Python program (a bot) that the Rust spine drives bar by bar. The spine owns the clock, order matching, fills and the portfolio ledger, so a bot only decides. A backtest and a live paper run go through the same code path.

## Quick start (macOS)

Requires Rust (version pinned in `rust-toolchain.toml`), `just`, `uv` and Node.

```sh
just setup
just dev
```

`just dev` starts the spine on `127.0.0.1:8080` and the web UI on <http://localhost:5173>. Log in with the password `buizz-dev-admin`. It loads the small fixture data set in `fixtures/market/` (AAA, BBB and SPY, daily bars 2024-11-20 to 2024-12-04, some 1m bars) and does not call any data provider, so only the `red_green` bot has data to run on.

Backtest it from the UI (bot detail, Backtest) or headless from a second terminal:

```sh
cd sdk/python
BUIZZ_ADMIN_PASSWORD=buizz-dev-admin uv run --frozen buizz bots
BUIZZ_ADMIN_PASSWORD=buizz-dev-admin uv run --frozen buizz backtest ../../bots/red_green --from 2024-11-20 --to 2024-12-04
```

The other bots in `bots/` need real data: set the provider keys below and start the spine without the fixture (see Configuration).

## Commands

| Command | What it does |
|---|---|
| `just setup` | Install web and end-to-end browser dependencies (`npm ci`) and the Python SDK environment (`uv sync --frozen`) |
| `just dev` | Run the spine and the Vite dev server with the fixture data |
| `just check` | Everything a merge needs: `lint`, `test`, `build`, `drift`, `e2e` (the default recipe) |
| `just e2e` | The end-to-end scenarios against the real spine binary and fake providers; the browser steps need Google Chrome installed |
| `just lint` | rustfmt, clippy, ruff, pyright, web typecheck and the no-comments lint |
| `just test` | Rust workspace tests and the SDK tests |
| `just build` | Build the workspace and the web bundle |
| `just fmt` | Format Rust and Python |
| `just gen` | Regenerate the golden protocol frames, `openapi.json` and the web API types |
| `just drift` | Fail if the generated files differ from the code |
| `buizz bots` | List bots with status, timeframe and equity |
| `buizz backtest bots/<name> --from YYYY-MM-DD --to YYYY-MM-DD [--param key=value]...` | Backtest a bot, wait, print metrics and an order summary |

`buizz` is the SDK's console script; run it with `uv run --frozen buizz` inside `sdk/python`. It talks to a running spine: `BUIZZ_URL` (default `http://127.0.0.1:8080`) and `BUIZZ_ADMIN_PASSWORD` (the spine's admin password, used to log in). `--param` values are parsed as JSON and fall back to strings. The spine runs the bot from its own `bots/` directory, so the argument only selects the folder name.

## Configuration

The spine (`cargo run -p buizz-spine`) reads these environment variables. Empty values count as unset.

| Variable | Default | Meaning |
|---|---|---|
| `BUIZZ_ADMIN_PASSWORD` | required | The single admin password for the web UI and API, at least 12 characters |
| `BUIZZ_BIND` | `0.0.0.0:8080` | Listen address, `host:port`; with port 0 the spine picks a free port and logs it as `spine listening addr=<host:port>` |
| `BUIZZ_DATA_DIR` | `data` | Where `market.duckdb`, `state.db` and `backups/` live |
| `BUIZZ_BOTS_DIR` | `bots` | Folder scanned for bots |
| `BUIZZ_WEB_DIR` | unset | A built web bundle (`web/dist`) to serve from the spine; unset serves the API only |
| `BUIZZ_FIXTURE_DIR` | unset | Load the CSV fixture data set in this folder into the market store at start and advance the data watermarks to it |
| `BUIZZ_SECURE_COOKIES` | `true` | Mark the session cookie `Secure`; set `false` for plain HTTP |
| `BUIZZ_INGEST` | `true` | Run the scheduled data jobs; `false` for fixture-only work |
| `ALPACA_KEY_ID`, `ALPACA_SECRET_KEY` | unset | Alpaca market data credentials; both are needed to enable Alpaca, the live feed and minute-bar backtests |
| `ALPACA_DATA_URL` | `https://data.alpaca.markets` | Alpaca data endpoint |
| `ALPACA_TRADING_URL` | `https://paper-api.alpaca.markets` | Alpaca endpoint for the calendar and assets |
| `ALPACA_STREAM_URL` | `wss://stream.data.alpaca.markets/v2/iex` | Alpaca real-time stream |
| `TIINGO_API_KEY` | unset | Enables Tiingo history before 2016 |
| `TIINGO_URL` | `https://api.tiingo.com` | Tiingo endpoint |
| `SEC_USER_AGENT` | unset | Enables SEC EDGAR fundamentals; the SEC requires a contact in the User-Agent, for example `name email@example.com` |
| `SEC_DATA_URL`, `SEC_WWW_URL` | `https://data.sec.gov`, `https://www.sec.gov` | EDGAR endpoints |
| `TREASURY_URL` | `https://home.treasury.gov` | US Treasury rates endpoint |
| `RUST_LOG` | `info` | Log filter |

`just dev` sets `BUIZZ_ADMIN_PASSWORD=buizz-dev-admin`, `BUIZZ_SECURE_COOKIES=false`, `BUIZZ_INGEST=false`, `BUIZZ_BIND=127.0.0.1:8080` and `BUIZZ_FIXTURE_DIR=fixtures/market`.

Spine start with real data and the bundled UI:

```sh
(cd web && npm run build)
BUIZZ_ADMIN_PASSWORD=... BUIZZ_SECURE_COOKIES=false BUIZZ_WEB_DIR=web/dist \
ALPACA_KEY_ID=... ALPACA_SECRET_KEY=... SEC_USER_AGENT="name email@example.com" \
cargo run --release -p buizz-spine
```

Bot logs go to `logs/<run_id>.log` under `BUIZZ_DATA_DIR`.

Failed logins are rate-limited per client address. Behind a reverse proxy the spine takes the client from the last `X-Forwarded-For` entry when the direct peer is a private or loopback address, so the deployment must preserve client IPs: the proxy must set `X-Forwarded-For`, and in Kubernetes the proxy's Service needs `externalTrafficPolicy: Local` so the proxy sees the real source address.

## Docs

- [Writing a bot](docs/writing-a-bot.md): folder layout, `bot.toml`, the SDK, fills, costs, risk and holdout rules, the headless loop
- [Architecture](docs/architecture.md): components, the one event path, guarantees, data sources, on-disk state
- [Bot protocol](docs/protocol.md): the WebSocket wire format between the spine and a bot
