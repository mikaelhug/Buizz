import numpy as np
import numpy.typing as npt
from scipy.stats import rankdata

from buizz import History

VOLATILITY_DAYS = 252
VOLATILITY_MIN_DAYS = 120
CORRELATION_DAYS = 1260
CORRELATION_MIN_DAYS = 750
OVERLAP_DAYS = 3
SHRINK_TO_ONE = 0.6
HISTORY_BARS = CORRELATION_DAYS + OVERLAP_DAYS


def log_returns_on(market: History, stock: History) -> npt.NDArray[np.float64]:
    closes = np.full(market.start.size, np.nan)
    position = np.searchsorted(market.start, stock.start)
    inside = position < market.start.size
    matched = np.zeros(stock.start.size, dtype=bool)
    matched[inside] = market.start[position[inside]] == stock.start[inside]
    closes[position[matched]] = stock.close[matched]
    return np.diff(np.log(closes))


def overlapping_sums(returns: npt.NDArray[np.float64]) -> npt.NDArray[np.float64]:
    return np.lib.stride_tricks.sliding_window_view(returns, OVERLAP_DAYS).sum(axis=1)


def frazzini_pedersen_beta(stock: History, market: History) -> float | None:
    stock_returns = log_returns_on(market, stock)
    market_returns = np.diff(np.log(market.close))
    if market_returns.size < VOLATILITY_DAYS:
        return None
    recent = stock_returns[-VOLATILITY_DAYS:]
    recent_valid = np.isfinite(recent)
    if recent_valid.sum() < VOLATILITY_MIN_DAYS:
        return None
    stock_volatility = float(np.std(recent[recent_valid], ddof=1))
    market_volatility = float(np.std(market_returns[-VOLATILITY_DAYS:], ddof=1))
    window = CORRELATION_DAYS + OVERLAP_DAYS - 1
    stock_three_day = overlapping_sums(stock_returns[-window:])
    market_three_day = overlapping_sums(market_returns[-window:])
    valid = np.isfinite(stock_three_day)
    if valid.sum() < CORRELATION_MIN_DAYS or market_volatility == 0:
        return None
    correlation = float(np.corrcoef(stock_three_day[valid], market_three_day[valid])[0, 1])
    time_series_beta = correlation * stock_volatility / market_volatility
    return SHRINK_TO_ONE * time_series_beta + (1 - SHRINK_TO_ONE)


def ols_beta(stock: History, market: History, days: int) -> float | None:
    stock_returns = log_returns_on(market, stock)[-days:]
    market_returns = np.diff(np.log(market.close))[-days:]
    valid = np.isfinite(stock_returns)
    if valid.sum() < VOLATILITY_MIN_DAYS or market_returns.size < days:
        return None
    market_valid = market_returns[valid]
    variance = float(np.var(market_valid, ddof=1))
    return None if variance == 0 else float(np.cov(stock_returns[valid], market_valid)[0, 1]) / variance


def bab_weights(betas: dict[str, float]) -> dict[str, float]:
    symbols = sorted(betas)
    beta = np.array([betas[symbol] for symbol in symbols])
    centered = rankdata(beta) - (len(symbols) + 1) / 2
    spread = np.abs(centered).sum()
    if spread == 0:
        return {}
    high = 2 / spread * np.maximum(centered, 0)
    low = 2 / spread * np.maximum(-centered, 0)
    weights = low / float(low @ beta) - high / float(high @ beta)
    return dict(zip(symbols, weights.tolist(), strict=True))
