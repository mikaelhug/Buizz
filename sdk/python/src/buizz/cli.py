from __future__ import annotations

import argparse
import contextlib
import json
import os
import sys
import time
from collections import Counter
from http.cookiejar import CookieJar
from pathlib import Path
from typing import Any
from urllib.error import HTTPError
from urllib.request import HTTPCookieProcessor, Request, build_opener

DEFAULT_URL = "http://127.0.0.1:8080"
FINISHED = {"completed", "failed", "cancelled"}
POLL_SECONDS = 1.0
ORDERS_PAGE = 1000


class CliError(Exception):
    pass


class Api:
    def __init__(self, base: str, password: str | None) -> None:
        self._base = base.rstrip("/")
        self._opener = build_opener(HTTPCookieProcessor(CookieJar()))
        if password is not None:
            self.call("POST", "/api/auth/login", {"password": password})

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


def format_value(value: Any) -> str:
    if value is None:
        return "-"
    return f"{value:.4f}" if isinstance(value, float) else str(value)


def list_bots(api: Api) -> None:
    for bot in api.call("GET", "/api/bots"):
        print(f"{bot['name']:<20} {bot['status']:<12} {bot['timeframe']:<4} {bot['equity'] or '-'}")


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
    if args.param:
        body["params"] = dict(args.param)
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
        metrics: dict[str, Any] = run["metrics"] or {}
        for name, value in metrics.items():
            print(f"{name:<16} {format_value(value)}")
        print("\n".join(trade_summary(api, run["id"])))
    if not completed:
        print(f"run {run['id']} {run['status']}: {run['message'] or 'no message'}", file=sys.stderr)
        return 1
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="buizz")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("bots", help="list bots")
    run = commands.add_parser("backtest", help="backtest a bot folder")
    run.add_argument("bot", help="bots/<name>")
    run.add_argument("--from", dest="start", required=True, metavar="YYYY-MM-DD")
    run.add_argument("--to", dest="end", required=True, metavar="YYYY-MM-DD")
    run.add_argument("--param", action="append", default=[], type=parse_param, metavar="KEY=VALUE")
    run.add_argument("--allow-holdout", action="store_true", help="allow an end date inside the locked holdout")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        api = Api(os.environ.get("BUIZZ_URL", DEFAULT_URL), os.environ.get("BUIZZ_ADMIN_PASSWORD"))
        if args.command == "bots":
            list_bots(api)
            return 0
        return backtest(api, args)
    except CliError as exc:
        print(f"buizz: {exc}", file=sys.stderr)
        return 1
