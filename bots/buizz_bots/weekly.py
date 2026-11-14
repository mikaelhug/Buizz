from datetime import date

import numpy as np
import numpy.typing as npt

from buizz import History
from buizz_bots.monthly import period_end_closes

DAYS_FROM_EPOCH_TO_MONDAY = 3


def completed_week_closes(history: History, today: date, week_ends_today: bool) -> npt.NDArray[np.float64]:
    return period_end_closes(
        history, today, lambda days: (days.astype(np.int64) + DAYS_FROM_EPOCH_TO_MONDAY) // 7, week_ends_today
    )
