import math

import numpy as np
import numpy.typing as npt

VARIANCE_DAYS_PER_YEAR = 261
TRUNCATED_WEIGHT = 1e-4


def ewma_sessions(centre_of_mass: float) -> int:
    return math.ceil(math.log(TRUNCATED_WEIGHT) / math.log(centre_of_mass / (centre_of_mass + 1)))


def ex_ante_volatility(closes: npt.NDArray[np.float64], centre_of_mass: float) -> float:
    newest_first = (closes[1:] / closes[:-1] - 1)[::-1]
    delta = centre_of_mass / (centre_of_mass + 1)
    weights = delta ** np.arange(newest_first.size)
    weights /= weights.sum()
    mean = float(weights @ newest_first)
    return math.sqrt(VARIANCE_DAYS_PER_YEAR * float(weights @ (newest_first - mean) ** 2))
