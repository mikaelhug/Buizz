from __future__ import annotations

from datetime import date, datetime
from typing import Literal

import msgspec

type FactStatus = Literal["ok", "not_stated"]


class ListingNotice(msgspec.Struct, frozen=True, kw_only=True):
    symbol: str
    accession: str
    status: FactStatus
    available_at: datetime
    evidence: str | None
    exchange: Literal["nyse", "nyse_american", "nasdaq", "other"] | None = None
    rule: str | None = None


type Fact = ListingNotice
type RecipeName = Literal["listing_notice"]

RECIPES: dict[str, type[Fact]] = {"listing_notice": ListingNotice}


class FactQueries:
    def facts(self, recipe: RecipeName, symbols: list[str], since: date) -> list[Fact]:
        return self._facts(recipe, symbols, since)

    def _facts(self, recipe: str, symbols: list[str], since: date) -> list[Fact]:
        raise NotImplementedError
