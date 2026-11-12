from collections.abc import Mapping

import numpy as np
from scipy.stats import rankdata

MIN_CROSS_SECTION = 2
TIE_DECIMALS = 9


def highest_first(scores: Mapping[str, float]) -> list[str]:
    return sorted(scores, key=lambda symbol: (-round(scores[symbol], TIE_DECIMALS), symbol))


def rank_z(values: Mapping[str, float]) -> dict[str, float]:
    if len(values) < MIN_CROSS_SECTION:
        return {}
    symbols = sorted(values)
    ranks = rankdata([values[symbol] for symbol in symbols])
    spread = float(np.std(ranks))
    if spread == 0:
        return {}
    scores = (ranks - ranks.mean()) / spread
    return dict(zip(symbols, scores.tolist(), strict=True))


def percentiles(values: Mapping[str, float]) -> dict[str, float]:
    symbols = sorted(values)
    ranks = rankdata([values[symbol] for symbol in symbols]) / len(symbols)
    return dict(zip(symbols, ranks.tolist(), strict=True))
