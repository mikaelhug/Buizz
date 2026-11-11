from __future__ import annotations

import contextlib
import socket
import ssl
from collections import deque
from typing import TYPE_CHECKING, Self

from websockets.client import ClientProtocol
from websockets.exceptions import ConnectionClosed
from websockets.frames import Frame, Opcode
from websockets.http11 import Response
from websockets.protocol import State
from websockets.uri import parse_uri

if TYPE_CHECKING:
    from types import TracebackType

CHUNK = 1 << 18
NORMAL_CLOSURE = 1000


class Connection:
    def __init__(self, url: str, token: str) -> None:
        uri = parse_uri(url)
        sock = socket.create_connection((uri.host, uri.port))
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        if uri.secure:
            sock = ssl.create_default_context().wrap_socket(sock, server_hostname=uri.host)
        self._sock = sock
        self._protocol = ClientProtocol(uri, max_size=None)
        self._parts: list[bytes] = []
        self._messages: deque[bytes] = deque()
        request = self._protocol.connect()
        request.headers["Authorization"] = f"Bearer {token}"
        self._protocol.send_request(request)
        self._flush()
        while self._protocol.state is State.CONNECTING:
            self._pump()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self, exc_type: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None
    ) -> None:
        with contextlib.suppress(OSError, ConnectionClosed):
            if self._protocol.state is State.OPEN:
                self._protocol.send_close(NORMAL_CLOSURE)
                self._flush()
        self._sock.close()

    def send(self, message: bytes) -> None:
        self._protocol.send_binary(message)
        self._flush()

    def receive(self) -> bytes:
        while not self._messages:
            self._pump()
        return self._messages.popleft()

    def _flush(self) -> None:
        for chunk in self._protocol.data_to_send():
            if chunk:
                self._sock.sendall(chunk)
            else:
                self._sock.shutdown(socket.SHUT_WR)

    def _pump(self) -> None:
        data = self._sock.recv(CHUNK)
        if data:
            self._protocol.receive_data(data)
        else:
            self._protocol.receive_eof()
        self._flush()
        for event in self._protocol.events_received():
            if isinstance(event, Response):
                if self._protocol.handshake_exc is not None:
                    raise self._protocol.handshake_exc
            elif isinstance(event, Frame) and event.opcode in (Opcode.BINARY, Opcode.CONT):
                payload = event.data if isinstance(event.data, bytes) else bytes(event.data)
                if event.fin and not self._parts:
                    self._messages.append(payload)
                else:
                    self._parts.append(payload)
                    if event.fin:
                        self._messages.append(b"".join(self._parts))
                        self._parts.clear()
        if not data and not self._messages:
            raise self._protocol.close_exc
