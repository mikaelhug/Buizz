from __future__ import annotations

import argparse
import contextlib
import csv
import json
import os
import sys
import time
from collections import Counter
from datetime import UTC, datetime
from decimal import Decimal, InvalidOperation
from http.cookiejar import CookieJar
from pathlib import Path
from typing import TYPE_CHECKING, Any, cast
from urllib.error import HTTPError
from urllib.request import HTTPCookieProcessor, Request, build_opener

if TYPE_CHECKING:
    from collections.abc import Callable

DEFAULT_URL = "http://127.0.0.1:8080"
FINISHED = {"completed", "failed", "cancelled"}
POLL_SECONDS = 1.0
ORDERS_PAGE = 1000
RISK_LIMITS = ("max_position_weight", "max_gross_exposure", "max_drawdown")
SWEEP_PRESETS = ("1y", "3y", "5y", "10y")


class CliError(Exception):
    pass


class Api:
    def __init__(self, base: str, username: str, password: str | None) -> None:
        self._base = base.rstrip("/")
        self._opener = build_opener(HTTPCookieProcessor(CookieJar()))
        if password is not None:
            self.call("POST", "/api/auth/login", {"username": username, "password": password})

    def call(self, method: str, path: str, body: dict[str, Any] | None = None) -> Any:
        data = None if body is None else json.dumps(body).encode()
        request = Request(f"{self._base}{path}", data=data, method=method, headers={"Content-Type": "application/json"})
        try:
            with self._opener.open(request) as response:
                raw = response.read()
        except HTTPError as exc:
            detail = exc.read().decode(errors="replace")
            with contextlib.suppress(ValueError, KeyError, TypeError):
                detail = json.loads(detail)["error"]
            raise CliError(f"{method} {path}: {exc.code} {detail}") from exc
        except OSError as exc:
            raise CliError(f"cannot reach {self._base}: {exc}") from exc
        return json.loads(raw) if raw else None


def parse_param(text: str) -> tuple[str, Any]:
    key, separator, value = text.partition("=")
    if not separator or not key:
        raise argparse.ArgumentTypeError(f"expected key=value, got {text!r}")
    try:
        return key, json.loads(value)
    except ValueError:
        return key, value


def parse_grid(text: str) -> tuple[str, list[Any]]:
    key, values = parse_param(text)
    if not isinstance(values, str):
        return key, [values]
    return key, [parse_param(f"{key}={value}")[1] for value in values.split(",")]


def parse_risk(text: str) -> tuple[str, str | None]:
    key, separator, value = text.partition("=")
    if not separator or key not in RISK_LIMITS:
        raise argparse.ArgumentTypeError(f"expected one of {', '.join(RISK_LIMITS)}=VALUE, got {text!r}")
    if value == "none":
        return key, None
    try:
        float(value)
    except ValueError:
        raise argparse.ArgumentTypeError(f"{key} must be a number or none, got {value!r}") from None
    return key, value


def decimal_text(value: str) -> str:
    try:
        number = Decimal(value)
    except InvalidOperation:
        number = None
    if number is None or not number.is_finite():
        raise ValueError(f"must be a number, got {value!r}")
    return value


def whole_number(value: str) -> int:
    if not value.isdigit():
        raise ValueError(f"must be a whole number, got {value!r}")
    return int(value)


def commission_tiers(value: str) -> list[Any]:
    try:
        tiers = json.loads(value)
    except ValueError:
        tiers = None
    if not isinstance(tiers, list):
        raise ValueError(f'must be a JSON list such as [{{"above": 300000, "per_share": "0.002"}}], got {value!r}')
    return cast("list[Any]", tiers)


COST_FIELDS: dict[str, Callable[[str], Any]] = {
    "commission_per_share": decimal_text,
    "commission_min": decimal_text,
    "commission_tiers": commission_tiers,
    "slippage_bps": decimal_text,
    "slippage_per_share": decimal_text,
    "spread_scale": decimal_text,
    "impact_coefficient": decimal_text,
    "adv_participation": decimal_text,
    "adv_sessions": whole_number,
    "borrow_rate": decimal_text,
    "financing_spread": decimal_text,
}


def parse_cost(text: str) -> tuple[str, Any]:
    key, separator, value = text.partition("=")
    parse = COST_FIELDS.get(key)
    if not separator or parse is None:
        raise argparse.ArgumentTypeError(f"expected one of {', '.join(COST_FIELDS)}=VALUE, got {text!r}")
    try:
        return key, parse(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"{key} {exc}") from None


def format_value(value: Any) -> str:
    if value is None:
        return "-"
    return f"{value:.4f}" if isinstance(value, float) else str(value)


def list_bots(api: Api) -> None:
    for bot in api.call("GET", "/api/bots"):
        print(f"{bot['name']:<20} {bot['stage']:<9} {bot['timeframe']:<4} {len(bot['live'])} live")


def variant_line(variant: dict[str, Any]) -> str:
    chosen: dict[str, Any] = variant.get("params") or {}
    params = " ".join(f"{name}={value}" for name, value in chosen.items())
    source = "" if variant.get("from_run") is None else f" from #{variant['from_run']}"
    return f"{variant['bot']}{' ' + params if params else ''}{source}"


def live_screened(api: Api) -> int:
    outcome = api.call("POST", "/api/bots/live")
    print(f"started {len(outcome['started'])}:")
    for variant in outcome["started"]:
        print(f"  {variant_line(variant)} -> run #{variant['run_id']}")
    declined = [variant for variant in outcome["skipped"] if variant.get("from_run") is not None]
    unscreened = [variant["bot"] for variant in outcome["skipped"] if variant.get("from_run") is None]
    if declined:
        print(f"not started {len(declined)}:")
        for variant in declined:
            print(f"  {variant_line(variant)}: {variant['reason']}")
    if unscreened:
        print(f"no variant passes the gate ({len(unscreened)}): {', '.join(unscreened)}")
    return 0


def trade_summary(api: Api, run_id: int) -> list[str]:
    orders: list[dict[str, Any]] = []
    after: int | None = None
    while True:
        query = f"?limit={ORDERS_PAGE}" + ("" if after is None else f"&after={after}")
        page = api.call("GET", f"/api/runs/{run_id}/orders{query}")
        orders += page["items"]
        after = page.get("next_after")
        if after is None:
            break
    statuses = Counter(order["status"] for order in orders)
    held = {position["symbol"]: position["qty"] for position in api.call("GET", f"/api/runs/{run_id}/positions")}
    traded = {order["symbol"] for order in orders if order["filled_qty"] > 0}
    lines = [f"orders: {len(orders)} ({', '.join(f'{n} {status}' for status, n in sorted(statuses.items()))})"]
    lines += [f"  {symbol:<8} net {held.get(symbol, 0):+d} shares" for symbol in sorted(traded | held.keys())]
    return lines


def backtest(api: Api, args: argparse.Namespace) -> int:
    bot = Path(args.bot).name
    body: dict[str, Any] = {"period": {"from": args.start, "to": args.end}, "allow_holdout": args.allow_holdout}
    manifest: dict[str, Any] = api.call("GET", f"/api/bots/{bot}")["manifest"] if args.param or args.risk else {}
    if args.param:
        params = dict(args.param)
        defaults: dict[str, Any] = manifest.get("params", {})
        if unknown := sorted(params.keys() - defaults.keys()):
            raise CliError(
                f"{bot} has no param {', '.join(unknown)}; its params are {', '.join(sorted(defaults)) or 'none'}"
            )
        body["params"] = params
    if args.cost:
        body["cost"] = dict(args.cost)
    if args.risk:
        body["risk"] = {**dict.fromkeys(RISK_LIMITS), **manifest.get("risk", {}), **dict(args.risk)}
    run = api.call("POST", f"/api/bots/{bot}/backtests", body)
    print(f"run {run['id']} queued for {bot} {args.start}..{args.end}")
    last_clock = None
    while run["status"] not in FINISHED:
        time.sleep(POLL_SECONDS)
        run = api.call("GET", f"/api/runs/{run['id']}")
        if run["clock"] != last_clock:
            last_clock = run["clock"]
            print(f"  {run['status']} {last_clock} equity {run['equity']}")
    completed = run["status"] == "completed"
    if completed or run["metrics"] is not None:
        hedged = run["config"].get("hedge_benchmark") is not None
        reported: dict[str, Any] = run["metrics"] or {}
        metrics = {name: value for name, value in reported.items() if hedged or not name.startswith("hedge_")}
        width = max(map(len, metrics), default=0)
        for name, value in metrics.items():
            print(f"{name:<{width}} {format_value(value)}")
        print("\n".join(trade_summary(api, run["id"])))
    if not completed:
        print(f"run {run['id']} {run['status']}: {run['message'] or 'no message'}", file=sys.stderr)
        return 1
    return 0


def sweep(api: Api, args: argparse.Namespace) -> int:
    bot = Path(args.bot).name
    started = api.call("POST", f"/api/bots/{bot}/sweeps", {"preset": args.preset, "grid": dict(args.grid)})
    period = started["period"]
    trials = started["trials"]
    print(
        f"{len(started['queued'])} runs queued for {bot} {args.preset} {period['from']}..{period['to']}, "
        f"{started['skipped']} already run"
    )
    print(f"trials: {trials['variants']} variants, {trials['runs']} runs")
    return 0


def event_time(text: str, where: str) -> str:
    try:
        at = datetime.fromisoformat(text.strip())
    except ValueError:
        raise CliError(f"{where}: at must be an ISO 8601 timestamp, got {text!r}") from None
    if at.tzinfo is None:
        raise CliError(f"{where}: at {text!r} must name its time zone, such as 2024-11-21T15:00:00Z")
    return at.astimezone(UTC).isoformat().replace("+00:00", "Z")


def study_event(row: dict[str, Any], where: str) -> dict[str, str]:
    symbol, at = row.get("symbol"), row.get("at")
    if not isinstance(symbol, str) or not symbol.strip() or not isinstance(at, str):
        raise CliError(f"{where}: an event needs a symbol and an at timestamp")
    return {"symbol": symbol.strip(), "at": event_time(at, where)}


def read_events(path: Path) -> list[dict[str, str]]:
    try:
        text = path.read_text()
    except OSError as exc:
        raise CliError(f"cannot read {path}: {exc}") from exc
    if path.suffix == ".jsonl":
        rows: list[tuple[int, Any]] = []
        for number, line in enumerate(text.splitlines(), 1):
            if line.strip():
                try:
                    rows.append((number, json.loads(line)))
                except ValueError:
                    raise CliError(f"{path}:{number}: not a JSON object") from None
        return [
            study_event(cast("dict[str, Any]", row) if isinstance(row, dict) else {}, f"{path}:{number}")
            for number, row in rows
        ]
    reader = csv.DictReader(text.splitlines())
    return [study_event(row, f"{path}:{reader.line_num}") for row in reader]


def percent(value: float | None) -> str:
    return "-" if value is None else f"{value * 100:+.2f}%"


def ratio(value: float | None) -> str:
    return "-" if value is None else f"{value:.2f}"


def event_study(api: Api, args: argparse.Namespace) -> int:
    path = Path(args.events)
    body: dict[str, Any] = {"label": args.label or path.stem, "events": read_events(path)}
    if args.benchmark:
        body["benchmark"] = args.benchmark
    if args.cost:
        body["cost"] = dict(args.cost)
    study = api.call("POST", "/api/studies", body)
    events: list[dict[str, Any]] = study["events"]
    skipped = Counter(event["skipped"] for event in events if event["skipped"])
    measured = [event for event in events if event["returns"][0] is not None]
    benchmarks = Counter(event["benchmark"] for event in measured)
    costs = [event["round_trip_cost"] for event in measured]
    print(
        f"study {study['label']}: {len(events)} events, {len(measured)} measured, "
        f"{sum(skipped.values())} skipped" + "".join(f", {n} {reason}" for reason, n in sorted(skipped.items()))
    )
    print(
        f"data through {study['data_through'] or '-'}; benchmarks "
        + (", ".join(f"{symbol} {n}" for symbol, n in sorted(benchmarks.items())) or "-")
        + f"; mean round trip cost {f'{sum(costs) / len(costs):.2%}' if costs else '-'} "
        f"at {study['position']} a position"
    )
    print(f"trials {study['trials']}, luck line t {ratio(study['luck_t'])}")
    print(
        f"{'sessions':>8} {'count':>6} {'months':>6} {'delisted':>8} {'mean':>8} {'median':>8} {'hit':>6} "
        f"{'t':>6} {'net mean':>9} {'net med':>8} {'net hit':>7} {'net t':>6}"
    )
    for horizon in study["horizons"]:
        gross, net = horizon["gross"], horizon["net"]
        hit = "-" if gross["hit_rate"] is None else f"{gross['hit_rate']:.0%}"
        net_hit = "-" if net["hit_rate"] is None else f"{net['hit_rate']:.0%}"
        print(
            f"{horizon['sessions']:>8} {horizon['count']:>6} {horizon['months']:>6} {horizon['delisted']:>8} "
            f"{percent(gross['mean']):>8} {percent(gross['median']):>8} {hit:>6} {ratio(gross['t_stat']):>6} "
            f"{percent(net['mean']):>9} {percent(net['median']):>8} {net_hit:>7} {ratio(net['t_stat']):>6}"
        )
    print(f"{'session':>8} {'count':>6} {'CAR':>8} {'net CAR':>8}")
    for point in study["curve"]:
        print(f"{point['session']:>8} {point['count']:>6} {percent(point['gross']):>8} {percent(point['net']):>8}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="buizz")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("bots", help="list bots")
    commands.add_parser(
        "live-screened", help="put every variant that passes the screening gate live, up to three per bot"
    )
    run = commands.add_parser("backtest", help="backtest a bot folder")
    run.add_argument("bot", help="bots/<name>")
    run.add_argument("--from", dest="start", required=True, metavar="YYYY-MM-DD")
    run.add_argument("--to", dest="end", required=True, metavar="YYYY-MM-DD")
    run.add_argument("--param", action="append", default=[], type=parse_param, metavar="KEY=VALUE")
    run.add_argument(
        "--risk",
        action="append",
        default=[],
        type=parse_risk,
        metavar="LIMIT=VALUE",
        help="override one of the bot's risk limits for this run, or remove it with none",
    )
    run.add_argument(
        "--cost",
        action="append",
        default=[],
        type=parse_cost,
        metavar="FIELD=VALUE",
        help="set one field of the cost model for this run; the others keep their defaults",
    )

    run.add_argument("--allow-holdout", action="store_true", help="allow an end date inside the locked holdout")
    grid = commands.add_parser("sweep", help="queue a backtest for every combination of param values")
    grid.add_argument("bot", help="bots/<name>")
    grid.add_argument("--preset", choices=SWEEP_PRESETS, default="10y", help="the period, ending before the holdout")
    grid.add_argument(
        "--grid",
        action="append",
        default=[],
        type=parse_grid,
        metavar="KEY=V1,V2,...",
        help="the values to try for one param; the others keep their defaults",
    )
    study = commands.add_parser(
        "event-study", help="measure abnormal returns after dated events, from a CSV or JSONL file of symbol,at"
    )
    study.add_argument("events", help="a .csv with symbol and at columns, or a .jsonl of {symbol, at} objects")
    study.add_argument(
        "--label", help="the study's name, counted as one trial per distinct event list; default the file name"
    )
    study.add_argument("--benchmark", help="one benchmark for every event instead of SPY for S&P 500 members, else IWM")
    study.add_argument(
        "--cost",
        action="append",
        default=[],
        type=parse_cost,
        metavar="FIELD=VALUE",
        help="set one field of the cost model; the others keep their defaults",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        api = Api(
            os.environ.get("BUIZZ_URL", DEFAULT_URL),
            os.environ.get("BUIZZ_ADMIN_USERNAME", "admin"),
            os.environ.get("BUIZZ_ADMIN_PASSWORD"),
        )
        if args.command == "bots":
            list_bots(api)
            return 0
        if args.command == "live-screened":
            return live_screened(api)
        if args.command == "sweep":
            return sweep(api, args)
        if args.command == "event-study":
            return event_study(api, args)
        return backtest(api, args)
    except CliError as exc:
        print(f"buizz: {exc}", file=sys.stderr)
        return 1
