from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from buizz.context import Context


class Bot:
    def on_start(self, ctx: Context) -> None:
        return None

    def on_step(self, ctx: Context) -> None:
        raise NotImplementedError
