from __future__ import annotations

from datetime import date, datetime
from decimal import Decimal
from typing import Any, Literal, overload

import msgspec

type FactStatus = Literal["ok", "not_stated"]


class EarningsGuidance(msgspec.Struct, frozen=True, kw_only=True):
    symbol: str
    accession: str
    status: FactStatus
    available_at: datetime
    evidence: str | None
    basis: Literal["gaap", "non_gaap", "not_stated"] | None = None
    fiscal_quarter: int | None = None
    fiscal_year: int | None = None
    high: Decimal | None = None
    low: Decimal | None = None
    metric: (
        Literal[
            "revenue",
            "eps_gaap",
            "eps_adjusted",
            "operating_income",
            "operating_margin",
            "gross_margin",
            "ebitda_adjusted",
            "free_cash_flow",
            "capex",
            "tax_rate",
            "other",
        ]
        | None
    ) = None
    metric_label: str | None = None
    period_label: str | None = None
    stated_action: (
        Literal["initiated", "raised", "lowered", "reaffirmed", "narrowed", "withdrawn", "not_stated"] | None
    ) = None
    unit: Literal["usd", "usd_per_share", "percent", "percent_growth", "other_currency", "other"] | None = None


class ListingNotice(msgspec.Struct, frozen=True, kw_only=True):
    symbol: str
    accession: str
    status: FactStatus
    available_at: datetime
    evidence: str | None
    exchange: Literal["nyse", "nyse_american", "nasdaq", "other"] | None = None
    rule: str | None = None


type Fact = EarningsGuidance | ListingNotice
type RecipeName = Literal["earnings_guidance", "listing_notice"]

RECIPES: dict[str, type[Fact]] = {"earnings_guidance": EarningsGuidance, "listing_notice": ListingNotice}


class FactQueries:
    @overload
    def facts(
        self, recipe: Literal["earnings_guidance"], symbols: list[str], since: date
    ) -> list[EarningsGuidance]: ...
    @overload
    def facts(self, recipe: Literal["listing_notice"], symbols: list[str], since: date) -> list[ListingNotice]: ...
    def facts(self, recipe: RecipeName, symbols: list[str], since: date) -> list[Any]:
        return self._facts(recipe, symbols, since)

    def _facts(self, recipe: str, symbols: list[str], since: date) -> list[Fact]:
        raise NotImplementedError
