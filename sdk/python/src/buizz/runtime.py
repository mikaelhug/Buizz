from __future__ import annotations

import os
from importlib import metadata
from typing import TYPE_CHECKING

from buizz.context import Context
from buizz.errors import ProtocolError, SpineError
from buizz.transport import Connection
from buizz.wire import (
    PROTOCOL_VERSION,
    Data,
    End,
    Error,
    FundamentalsRequest,
    FundamentalsResult,
    Hello,
    HistoryRequest,
    HistoryResult,
    Query,
    Ready,
    Step,
    decode_inbound,
    encode,
)

if TYPE_CHECKING:
    from buizz.bot import Bot


def run(bot: type[Bot]) -> None:
    url = os.environ["BUIZZ_SPINE_URL"].rstrip("/")
    token = os.environ["BUIZZ_RUN_TOKEN"]
    instance = bot()
    with Connection(f"{url}/bot/v1", token) as connection:
        connection.send(encode(Hello(PROTOCOL_VERSION, f"buizz-python/{metadata.version('buizz')}")))
        ready = decode_inbound(connection.receive())
        if isinstance(ready, End):
            return
        if not isinstance(ready, Ready):
            raise unexpected(ready)
        next_query_id = 0

        def query(request: HistoryRequest | FundamentalsRequest) -> HistoryResult | FundamentalsResult:
            nonlocal next_query_id
            next_query_id += 1
            connection.send(encode(Query(next_query_id, request)))
            answer = decode_inbound(connection.receive())
            if isinstance(answer, Data) and answer.id == next_query_id:
                return answer.result
            if isinstance(answer, Error):
                raise SpineError(answer.code, answer.message)
            raise unexpected(answer)

        ctx = Context(ready, query)
        instance.on_start(ctx)
        while True:
            message = decode_inbound(connection.receive())
            if isinstance(message, Step):
                ctx.begin(message)
                instance.on_step(ctx)
                connection.send(encode(ctx.finish()))
            elif isinstance(message, End):
                instance.on_end(ctx, message.reason)
                return
            else:
                raise unexpected(message)


def unexpected(message: object) -> Exception:
    if isinstance(message, Error):
        return SpineError(message.code, message.message)
    return ProtocolError(f"unexpected {type(message).__name__} frame from the spine")
