from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from buizz.bot import Bot


def run(bot: type[Bot]) -> None:
    raise NotImplementedError
