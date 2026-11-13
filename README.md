# Buizz

Buizz tests trading strategies against historic and live US stock and ETF data, paper only. A strategy is a small Python program (a bot) that the Rust spine drives bar by bar. The spine owns the clock, order matching, fills and the portfolio ledger, so a bot only decides. A backtest and a live paper run go through the same code path.

## Quick start (macOS)

Requires Rust (version pinned in `rust-toolchain.toml`), `just`, `uv` and Node. Bots using LightGBM also need OpenMP on macOS (`brew install libomp`). The spine reads FactSet's PDF reports with poppler's `pdftotext` and `pdfinfo` (`brew install poppler`; the image installs `poppler-utils`).

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
| `just setup` | Install web and end-to-end browser dependencies (`npm ci`) and the Python SDK and bots environments (`uv sync --frozen`) |
| `just dev` | Run the spine and the Vite dev server with the fixture data |
| `just check` | Everything a merge needs: `lint`, `test`, `build`, `drift`, `e2e` (the default recipe) |
| `just e2e` | The end-to-end scenarios against the real spine binary and fake providers; the browser steps need Google Chrome installed |
| `just lint` | rustfmt, clippy, ruff, pyright, web typecheck and the no-comments lint |
| `just test` | Rust workspace tests and the SDK tests |
| `just build` | Build the workspace and the web bundle |
| `just fmt` | Format Rust and Python |
| `just gen` | Regenerate the golden protocol frames, `openapi.json` and the web API types |
| `just drift` | Fail if the generated files differ from the code |
| `buizz bots` | List bots with stage, timeframe and live variants |
| `buizz backtest bots/<name> --from YYYY-MM-DD --to YYYY-MM-DD [--param key=value]...` | Backtest a bot, wait, print metrics and an order summary |
| `buizz sweep bots/<name> [--preset 10y] --grid key=v1,v2,...` | Queue a backtest for every combination of param values; variants already run are skipped |
| `buizz event-study <events.csv\|.jsonl> [--label name] [--benchmark SYM] [--cost field=value]...` | Measure net and gross abnormal returns 1, 5, 21 and 63 sessions after dated events (`symbol`, `at`), with clustering-robust t-statistics and the luck line over every study run (see [Event studies](docs/writing-a-bot.md#event-studies)) |

`buizz` is the SDK's console script; run it with `uv run --frozen buizz` inside `sdk/python`. It talks to a running spine: `BUIZZ_URL` (default `http://127.0.0.1:8080`) `BUIZZ_ADMIN_USERNAME` (default `admin`) and `BUIZZ_ADMIN_PASSWORD` (the spine's admin login). `--param` values are parsed as JSON and fall back to strings. A `--param` key that is not in the bot's `bot.toml` params, or a `--cost` field that is not in the cost model, is refused before the run is queued. The spine runs the bot from its own `bots/` directory, so the argument only selects the folder name.

## Configuration

The spine (`cargo run -p buizz-spine`) reads these environment variables. Empty values count as unset.

| Variable | Default | Meaning |
|---|---|---|
| `BUIZZ_ADMIN_USERNAME` | `admin` | The username of the single admin login, so password managers can fill the form |
| `BUIZZ_ADMIN_PASSWORD` | required | The single admin password for the web UI and API, at least 12 characters |
| `BUIZZ_BIND` | `0.0.0.0:8080` | Listen address, `host:port`; with port 0 the spine picks a free port and logs it as `spine listening addr=<host:port>` |
| `BUIZZ_DATA_DIR` | `data` | Where `market.duckdb`, `state.db` and `backups/` live |
| `BUIZZ_BOTS_DIR` | `bots` | Folder scanned for bots |
| `BUIZZ_WEB_DIR` | unset | A built web bundle (`web/dist`) to serve from the spine; unset serves the API only |
| `BUIZZ_FIXTURE_DIR` | unset | Load the CSV fixture data set in this folder into the market store at start and advance the data watermarks to it |
| `BUIZZ_SECURE_COOKIES` | `true` | Mark the session cookie `Secure`; set `false` for plain HTTP |
| `BUIZZ_INGEST` | `true` | Run the scheduled data jobs; `false` for fixture-only work |
| `BUIZZ_INGEST_SYMBOLS` | unset | A comma-separated list that limits prices, corporate actions, fundamentals, filings and Tiingo history to these symbols, for a quick real-data spine on a laptop; calendar, instruments, membership, rates, macro and borrow stay complete |
| `BUIZZ_CLOCK_OFFSET_SECONDS` | `0` | Shifts the spine's clock by this many seconds, positive or negative; the end-to-end scenarios use it to run at noon New York time whatever the hour |
| `ALPACA_KEY_ID`, `ALPACA_SECRET_KEY` | unset | Alpaca market data credentials; both are needed to enable Alpaca, the live feed, minute-bar backtests and the `option_chains` recorder (the free indicative options feed) |
| `ALPACA_DATA_URL` | `https://data.alpaca.markets` | Alpaca data endpoint |
| `ALPACA_TRADING_URL` | `https://paper-api.alpaca.markets` | Alpaca endpoint for the calendar and assets |
| `ALPACA_STREAM_KEYS` | unset | Alpaca accounts for the real-time bar stream, one `key_id:secret` per line; each account streams 30 symbols on its own connection. Unset, the `ALPACA_KEY_ID` account is the one stream account; set, only the listed accounts stream, while `ALPACA_KEY_ID` stays the key for REST and the news stream |
| `ALPACA_STREAM_URL` | `wss://stream.data.alpaca.markets/v2/iex` | Alpaca real-time stream |
| `ALPACA_NEWS_URL` | `wss://stream.data.alpaca.markets/v1beta1/news` | Alpaca real-time news stream, recorded while ingest is on |
| `TIINGO_API_KEY` | unset | Enables Tiingo daily history from 2000 to Alpaca's 2016 start |
| `TIINGO_URL` | `https://api.tiingo.com` | Tiingo endpoint |
| `SEC_USER_AGENT` | unset | Enables SEC EDGAR fundamentals, SIC codes and earnings releases, and Wikipedia's S&P 500 membership; the SEC requires a contact in the User-Agent, for example `name email@example.com`, and the same contact goes into the Wikipedia and FactSet User-Agents, which it also enables |
| `SEC_DATA_URL`, `SEC_WWW_URL` | `https://data.sec.gov`, `https://www.sec.gov` | EDGAR endpoints |
| `TREASURY_URL` | `https://home.treasury.gov` | US Treasury rates endpoint |
| `WIKIPEDIA_URL` | `https://en.wikipedia.org` | MediaWiki endpoint for S&P 500 membership |
| `NASDAQ_TRADER_URL` | `https://www.nasdaqtrader.com` | NASDAQ Trader's symbol directory, read daily to tell ETFs from stocks, and its trade halt feed, read every 5 minutes by the `trading_halts` job |
| `SSGA_URL` | `https://www.ssga.com` | State Street's SPDR fund files, read daily by the `etf_holdings` job for holdings and shares outstanding; free, no key |
| `IBKR_FTP_ADDRESS` | `ftp2.interactivebrokers.com:21` | Interactive Brokers' public short stock FTP server, read daily by the `borrow` job |
| `FRED_API_KEY` | unset | Enables the FRED macro series for bots (`ctx.macro`); a free key from fred.stlouisfed.org. Without it the `macro` job reports that FRED is not configured |
| `FRED_URL` | `https://api.stlouisfed.org` | FRED endpoint |
| `CBOE_URL` | `https://cdn.cboe.com` | CBOE endpoint for the daily VIX and VIX3M history files |
| `FED_URL` | `https://www.federalreserve.gov` | Federal Reserve Board site, read for the FOMC meeting calendar |
| `ANTHROPIC_API_KEY` | unset | Enables Claude Haiku 5.5 for reading facts out of 8-K text; a key from the Claude Console (a Claude subscription is not one). Never logged. Without it the client is unconfigured |
| `ANTHROPIC_MONTHLY_CAP_USD` | `5` | Dollars the `recipe_facts` job may commit to the Anthropic API per calendar month (UTC): what the store's ledger says was spent this month plus the worst case of every open batch and of each request before it is sent. Raise it for the 8-K history fill (about $235 through the Batch API) |
| `ANTHROPIC_URL` | `https://api.anthropic.com` | Anthropic API endpoint |
| `FINRA_URL`, `FINRA_API_URL`, `FINRA_CDN_URL` | `https://www.finra.org`, `https://api.finra.org`, `https://cdn.finra.org` | FINRA's site (the short interest publication schedule), its public Query API (short interest) and its file server (daily short sale volume); free, no key or account, so the `short_interest` and `short_volume` jobs always run |
| `BUIZZ_DUCKDB_MEMORY` | `1GB` | Memory the market store may use for queries and writes before it spills to `market.duckdb.tmp/` in the data directory |
| `BUIZZ_DUCKDB_THREADS` | `2` | Threads the market store uses |
| `BUIZZ_TEXT_DIRS` | unset | Comma-separated directories, usually one per disk, where filings are kept as filed and as text (see `docs/architecture.md`, Where state lives); unset, documents are not harvested |
| `BUIZZ_LIVE_SLOTS` | `2` | Live bot processes that may start and take their first step of a session at once; backtests run on one fewer than the CPUs |
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

- [Writing a bot](docs/writing-a-bot.md): folder layout, `bot.toml`, the SDK, fills, costs, risk and holdout rules, event studies, the headless loop
- [Architecture](docs/architecture.md): components, the one event path, guarantees, data sources, on-disk state
- [Bot protocol](docs/protocol.md): the WebSocket wire format between the spine and a bot
