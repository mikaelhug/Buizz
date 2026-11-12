from datetime import date

import numpy as np
import numpy.typing as npt

from buizz import History

MAX_SESSIONS_PER_MONTH = 23


def month_end_closes(history: History, today: date) -> npt.NDArray[np.float64]:
    days = history.start.astype("datetime64[ns]").astype("datetime64[D]")
    if days.size == 0 or days[-1] != np.datetime64(today):
        return np.empty(0)
    months = days.astype("datetime64[M]")
    return history.close[np.append(months[1:] != months[:-1], True)]
