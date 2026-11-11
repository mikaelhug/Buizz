from __future__ import annotations


class BuizzError(Exception):
    pass


class SpineError(BuizzError):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


class ProtocolError(BuizzError):
    pass


class StateTooLarge(BuizzError):
    def __init__(self, size: int, limit: int) -> None:
        super().__init__(f"bot state is {size} bytes encoded, the limit is {limit}; keep ctx.state small")
        self.size = size
        self.limit = limit
