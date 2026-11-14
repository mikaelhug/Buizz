from datetime import date

from buizz import Context, Metric
from buizz_bots.fundamentals import Annual, amount, amount_or_zero, total_debt

FINANCIALS = range(6000, 7000)
UTILITIES = range(4900, 4950)
EBIT_METRICS: list[Metric] = ["operating_income", "pretax_income", "interest_expense"]
NET_DEBT_METRICS: list[Metric] = [
    "cash_and_equivalents",
    "short_term_investments",
    "short_term_debt",
    "short_term_borrowings",
    "current_long_term_debt",
    "long_term_debt",
    "long_term_debt_noncurrent",
    "preferred_stock",
]


def operating_companies(ctx: Context, symbols: list[str]) -> list[str]:
    profiles = ctx.instruments(symbols)
    return [
        symbol
        for symbol in symbols
        if symbol in profiles
        and (sic := profiles[symbol].sic) is not None
        and sic not in FINANCIALS
        and sic not in UTILITIES
    ]


def excess_cash(annual: Annual, symbol: str, period_end: date) -> float:
    return amount_or_zero(annual, symbol, "cash_and_equivalents", period_end) + amount_or_zero(
        annual, symbol, "short_term_investments", period_end
    )


def net_debt(annual: Annual, symbol: str, period_end: date) -> float:
    return (
        total_debt(annual, symbol, period_end)
        + amount_or_zero(annual, symbol, "preferred_stock", period_end)
        - excess_cash(annual, symbol, period_end)
    )


def ebit(annual: Annual, symbol: str, period_end: date) -> float | None:
    operating = amount(annual, symbol, "operating_income", period_end)
    if operating is not None:
        return operating
    pretax = amount(annual, symbol, "pretax_income", period_end)
    if pretax is None:
        return None
    return pretax + amount_or_zero(annual, symbol, "interest_expense", period_end)
