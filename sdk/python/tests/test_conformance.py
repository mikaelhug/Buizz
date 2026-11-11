from __future__ import annotations

import threading
from datetime import date
from decimal import Decimal
from http import HTTPStatus
from pathlib import Path
from typing import TYPE_CHECKING, Any

import msgspec
import pytest
from websockets.sync.server import ServerConnection, serve

from buizz import Bar, Bot, Context, EndReason, SpineError, run

if TYPE_CHECKING:
    from collections.abc import Callable

FIXTURES = Path(__file__).resolve().parents[3] / "fixtures" / "protocol"
TOKEN = "run-token"
observed: dict[str, Any] = {}


def golden(name: str) -> bytes:
    return (FIXTURES / f"{name}.msgpack").read_bytes()


class RedGreen(Bot):
    def on_start(self, ctx: Context) -> None:
        observed["universe"] = ctx.universe
        observed["params"] = ctx.params

    def on_step(self, ctx: Context) -> None:
        if ctx.session.date.isoformat() != "2024-11-21":
            observed["live_bars"] = ctx.bars
            observed["live_events"] = ctx.events
            observed["live_position"] = ctx.position("AAA")
            return
        observed["bars"] = ctx.bars
        observed["clock"] = ctx.clock
        observed["position"] = ctx.position("BBB")
        observed["history"] = ctx.history(["AAA"], "1d", 2, "split")["AAA"]
        observed["fundamentals"] = ctx.fundamentals(["BBB"], ["net_income", "shares_outstanding"])
        with pytest.raises(SpineError) as raised:
            ctx.history(["ZZZ"], "1d", 1)
        observed["error"] = raised.value.code
        observed["calendar"] = ctx.calendar(date(2024, 11, 28), date(2024, 12, 2))
        ctx.order_target_weight("AAA", ctx.params["weight"], reason="red day")
        ctx.order("BBB", -100, reason="take profit", limit_price=Decimal("49.60"), client_id="BBB-limit-1")
        ctx.cancel("BBB-limit-0")
        ctx.log("AAA closed red")
        ctx.state["days_red"] = 1

    def on_end(self, ctx: Context, reason: EndReason) -> None:
        observed["end"] = reason


class PausedMidStep(Bot):
    def on_step(self, ctx: Context) -> None:
        ctx.state["decided"] = True
        ctx.history(["AAA"], "1d", 2, "split")
        observed["after_end"] = True

    def on_end(self, ctx: Context, reason: EndReason) -> None:
        observed["paused"] = (reason, ctx.state)


def play_spine(connection: ServerConnection, received: list[bytes]) -> None:
    def expect() -> bytes:
        frame = connection.recv()
        assert isinstance(frame, bytes)
        received.append(frame)
        return frame

    expect()
    connection.send(golden("ready_backtest"))
    connection.send(golden("step_backtest"))
    expect()
    connection.send(golden("data_history"))
    expect()
    connection.send(golden("data_fundamentals"))
    expect()
    connection.send(golden("error"))
    expect()
    connection.send(golden("data_calendar"))
    expect()
    connection.send(golden("step_live"))
    expect()
    connection.send(golden("end_backtest"))


def pause_mid_step(connection: ServerConnection, received: list[bytes]) -> None:
    received.append(as_bytes(connection.recv()))
    connection.send(golden("ready_backtest"))
    connection.send(golden("step_backtest"))
    received.append(as_bytes(connection.recv()))
    connection.send(golden("end_live"))
    connection.close()


def as_bytes(frame: str | bytes) -> bytes:
    assert isinstance(frame, bytes)
    return frame


def authorize(connection: ServerConnection, request: Any) -> Any:
    if request.headers.get("Authorization") != f"Bearer {TOKEN}":
        return connection.respond(HTTPStatus.UNAUTHORIZED, "bad token")
    return None


def play(
    monkeypatch: pytest.MonkeyPatch, spine: Callable[[ServerConnection, list[bytes]], None], bot: type[Bot]
) -> list[bytes]:
    received: list[bytes] = []
    with serve(lambda c: spine(c, received), "127.0.0.1", 0, process_request=authorize) as server:
        threading.Thread(target=server.serve_forever, daemon=True).start()
        monkeypatch.setenv("BUIZZ_SPINE_URL", f"ws://127.0.0.1:{server.socket.getsockname()[1]}")
        monkeypatch.setenv("BUIZZ_RUN_TOKEN", TOKEN)
        run(bot)
        server.shutdown()
    return received


def test_bot_replies_match_the_golden_frames(monkeypatch: pytest.MonkeyPatch) -> None:
    received = play(monkeypatch, play_spine, RedGreen)

    hello, query_history, query_fundamentals, failing_query, query_calendar, ack_first, ack_second = received
    sent_hello = msgspec.msgpack.decode(hello)
    expected_hello = msgspec.msgpack.decode(golden("hello"))
    assert sent_hello["protocol"] == expected_hello["protocol"]
    assert sent_hello["sdk"].startswith("buizz-python/")
    assert query_history == golden("query_history")
    assert query_fundamentals == golden("query_fundamentals")
    assert msgspec.msgpack.decode(failing_query)["request"]["symbols"] == ["ZZZ"]
    assert query_calendar == golden("query_calendar")
    assert ack_first == golden("ack_backtest")
    assert ack_second == golden("ack_live")

    assert observed["universe"] == ["AAA", "BBB"]
    assert observed["params"] == {"weight": 0.25}
    assert observed["error"] == "bad_query"
    assert observed["end"] == "completed"
    assert observed["position"].qty == 510
    assert isinstance(observed["bars"]["AAA"], Bar)
    assert observed["bars"]["BBB"].close == 48.8
    assert observed["history"].close.tolist() == [98.0, 97.0]
    assert observed["fundamentals"][0].value == Decimal(410000000)
    assert [(s.date, s.close.hour) for s in observed["calendar"]] == [(date(2024, 11, 29), 18), (date(2024, 12, 2), 21)]
    assert len(observed["live_bars"]) == 2
    assert observed["live_events"][0].symbol == "AAA"


def test_a_pause_while_the_bot_waits_on_a_query_ends_it_cleanly(monkeypatch: pytest.MonkeyPatch) -> None:
    received = play(monkeypatch, pause_mid_step, PausedMidStep)

    assert len(received) == 2
    assert "after_end" not in observed
    assert observed["paused"] == ("paused", {"decided": True})
