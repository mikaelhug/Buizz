import numpy as np
from numpy.typing import NDArray
from scipy.cluster.hierarchy import leaves_list, linkage
from scipy.spatial.distance import pdist

Matrix = NDArray[np.float64]


def inverse_variance(covariance: Matrix) -> Matrix:
    inverse = 1 / np.diag(covariance)
    return inverse / inverse.sum()


def correlation_distance(covariance: Matrix) -> Matrix:
    volatility = np.sqrt(np.diag(covariance))
    correlation = np.clip(covariance / np.outer(volatility, volatility), -1, 1)
    return np.sqrt((1 - correlation) / 2)


def quasi_diagonal(covariance: Matrix) -> list[int]:
    distance_of_distances = pdist(correlation_distance(covariance), "euclidean")
    return [int(leaf) for leaf in leaves_list(linkage(distance_of_distances, "single"))]


def cluster_variance(covariance: Matrix, items: list[int]) -> float:
    block = covariance[np.ix_(items, items)]
    weights = inverse_variance(block)
    return float(weights @ block @ weights)


def bisection_shares(covariance: Matrix, order: list[int]) -> list[list[float]]:
    shares: list[list[float]] = [[] for _ in order]
    clusters = [order]
    while clusters:
        clusters = [
            half
            for items in clusters
            if len(items) > 1
            for half in (items[: len(items) // 2], items[len(items) // 2 :])
        ]
        for left, right in zip(clusters[::2], clusters[1::2], strict=True):
            left_variance = cluster_variance(covariance, left)
            right_variance = cluster_variance(covariance, right)
            alpha = 1 - left_variance / (left_variance + right_variance)
            for item in left:
                shares[item].append(alpha)
            for item in right:
                shares[item].append(1 - alpha)
    return shares


def hrp_weights(covariance: Matrix) -> Matrix:
    shares = bisection_shares(covariance, quasi_diagonal(covariance))
    return np.array([np.prod(path) for path in shares])
