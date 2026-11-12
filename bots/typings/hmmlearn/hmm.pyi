from typing import Literal, Self

import numpy as np
from numpy.typing import NDArray

class GaussianHMM:
    means_: NDArray[np.float64]
    covars_: NDArray[np.float64]
    transmat_: NDArray[np.float64]
    startprob_: NDArray[np.float64]
    def __init__(
        self,
        n_components: int = 1,
        covariance_type: Literal["spherical", "diag", "full", "tied"] = "diag",
        min_covar: float = 0.001,
        *,
        random_state: int | None = None,
        n_iter: int = 10,
        tol: float = 0.01,
    ) -> None: ...
    def fit(self, X: NDArray[np.float64], lengths: NDArray[np.int64] | None = None) -> Self: ...
    def score(self, X: NDArray[np.float64], lengths: NDArray[np.int64] | None = None) -> float: ...
    def decode(
        self,
        X: NDArray[np.float64],
        lengths: NDArray[np.int64] | None = None,
        algorithm: Literal["viterbi", "map"] | None = None,
    ) -> tuple[float, NDArray[np.int64]]: ...
