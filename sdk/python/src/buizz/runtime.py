from __future__ import annotations

import os
import signal
from importlib import metadata
from typing import TYPE_CHECKING, NoReturn

from websockets.exceptions import ConnectionClosed

from buizz.context import Context
from buizz.errors import ProtocolError, SpineError
from buizz.transport import Connection
from buizz.wire import (
    PROTOCOL_VERSION,
    Ack,
    Data,
    End,
    EndReason,
    Error,
    FundamentalsRequest,
    FundamentalsResult,
    Hello,
    HistoryRequest,
    HistoryResult,
    Inbound,
    Query,
    Ready,
    Step,
    decode_inbound,
    encode,
)

if TYPE_CHECKING:
    from types import FrameType

    from buizz.bot import Bot

END_WAIT_SECONDS = 1.0


class Ended(BaseException):
    def __init__(self, reason: EndReason | None) -> None:
        super().__init__(reason)
        self.reason: EndReason | None = reason


def terminated(signum: int, frame: FrameType | None) -> NoReturn:
    raise Ended(None)


class Link:
    def __init__(self, connection: Connection) -> None:
        self._connection = connection

    def send(self, message: Hello | Query | Ack) -> None:
        try:
            self._connection.send(encode(message))
        except (ConnectionClosed, OSError) as exc:
            raise Ended(None) from exc

    def receive(self) -> Inbound:
        try:
            message = decode_inbound(self._connection.receive())
        except (ConnectionClosed, OSError) as exc:
            raise Ended(None) from exc
        if isinstance(message, End):
            raise Ended(message.reason)
        return message

    def end_reason(self) -> EndReason:
        self._connection.wait_at_most(END_WAIT_SECONDS)
        try:
            while True:
                self.receive()
        except Ended as ended:
            return "disconnected" if ended.reason is None else ended.reason
        except Exception:
            return "disconnected"


def run(bot: type[Bot]) -> None:
    url = os.environ["BUIZZ_SPINE_URL"].rstrip("/")
    token = os.environ["BUIZZ_RUN_TOKEN"]
    instance = bot()
    with Connection(f"{url}/bot/v1", token) as connection:
        link = Link(connection)
        try:
            link.send(Hello(PROTOCOL_VERSION, f"buizz-python/{metadata.version('buizz')}"))
            ready = link.receive()
        except Ended:
            return
        if not isinstance(ready, Ready):
            raise unexpected(ready)
        next_query_id = 0

        def query(request: HistoryRequest | FundamentalsRequest) -> HistoryResult | FundamentalsResult:
            nonlocal next_query_id
            next_query_id += 1
            link.send(Query(next_query_id, request))
            answer = link.receive()
            if isinstance(answer, Data) and answer.id == next_query_id:
                return answer.result
            if isinstance(answer, Error):
                raise SpineError(answer.code, answer.message)
            raise unexpected(answer)

        ctx = Context(ready, query)
        previous = signal.signal(signal.SIGTERM, terminated)
        try:
            instance.on_start(ctx)
            while True:
                step = link.receive()
                if not isinstance(step, Step):
                    raise unexpected(step)
                ctx.begin(step)
                instance.on_step(ctx)
                link.send(ctx.finish())
        except Ended as ended:
            signal.signal(signal.SIGTERM, signal.SIG_DFL)
            reason = link.end_reason() if ended.reason is None else ended.reason
            ctx.close()
            instance.on_end(ctx, reason)
        finally:
            signal.signal(signal.SIGTERM, signal.SIG_DFL if previous is None else previous)


def unexpected(message: object) -> Exception:
    if isinstance(message, Error):
        return SpineError(message.code, message.message)
    return ProtocolError(f"unexpected {type(message).__name__} frame from the spine")
