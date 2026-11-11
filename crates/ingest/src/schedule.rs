use std::time::Duration;

use buizz_core::api::JobView;
use buizz_core::ops::{JobOutcome, JobRun};
use buizz_core::time::Timestamp;
use chrono::TimeDelta;

use crate::JobSpec;

pub(crate) const CALENDAR: &str = "calendar";
pub(crate) const INSTRUMENTS: &str = "instruments";
pub(crate) const BARS_1D: &str = "bars_1d";
pub(crate) const CORPORATE_ACTIONS: &str = "corporate_actions";
pub(crate) const SEC_INDEX: &str = "sec_index";
pub(crate) const FUNDAMENTALS: &str = "fundamentals";
pub(crate) const FILINGS: &str = "filings";
pub(crate) const INDEX_MEMBERSHIP: &str = "index_membership";
pub(crate) const RATES: &str = "rates";
pub(crate) const BORROW: &str = "borrow";
pub(crate) const MACRO: &str = "macro";
pub(crate) const STATE_SNAPSHOT: &str = "state_snapshot";
pub(crate) const MARKET_SNAPSHOT: &str = "market_snapshot";
pub(crate) const TIINGO_HISTORY: &str = "tiingo_history";

const HOUR: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const SIX_HOURS: Duration = Duration::from_secs(6 * 60 * 60);
const SHORT_RETRY: Duration = HOUR;
const FIRST_FAILURE_RETRY: Duration = Duration::from_secs(5 * 60);
const STALE_AFTER_INTERVALS: i32 = 3;

pub(crate) const JOBS: [JobSpec; 14] = [
    JobSpec {
        name: CALENDAR,
        every: DAY,
    },
    JobSpec {
        name: INSTRUMENTS,
        every: DAY,
    },
    JobSpec {
        name: INDEX_MEMBERSHIP,
        every: DAY,
    },
    JobSpec {
        name: BARS_1D,
        every: SIX_HOURS,
    },
    JobSpec {
        name: CORPORATE_ACTIONS,
        every: DAY,
    },
    JobSpec {
        name: SEC_INDEX,
        every: SIX_HOURS,
    },
    JobSpec {
        name: FUNDAMENTALS,
        every: SIX_HOURS,
    },
    JobSpec {
        name: FILINGS,
        every: SIX_HOURS,
    },
    JobSpec {
        name: RATES,
        every: DAY,
    },
    JobSpec {
        name: BORROW,
        every: DAY,
    },
    JobSpec {
        name: MACRO,
        every: DAY,
    },
    JobSpec {
        name: STATE_SNAPSHOT,
        every: DAY,
    },
    JobSpec {
        name: MARKET_SNAPSHOT,
        every: DAY,
    },
    JobSpec {
        name: TIINGO_HISTORY,
        every: HOUR,
    },
];

fn delta(duration: Duration) -> TimeDelta {
    TimeDelta::from_std(duration).unwrap_or(TimeDelta::MAX)
}

pub(crate) fn due_at(spec: &JobSpec, latest: Option<&JobRun>, failures: u32) -> Option<Timestamp> {
    let run = latest?;
    let wait = match run.outcome {
        Some(JobOutcome::Noop | JobOutcome::Backoff) => SHORT_RETRY,
        Some(JobOutcome::Failed) => {
            FIRST_FAILURE_RETRY.saturating_mul(1 << failures.saturating_sub(1).min(16))
        }
        Some(JobOutcome::Ok) => spec.every,
        Some(JobOutcome::Partial) | None => Duration::ZERO,
    };
    Some(run.started_at + delta(wait.min(spec.every)))
}

pub(crate) fn view(
    spec: &JobSpec,
    latest: Option<&JobRun>,
    failures: u32,
    success: Option<&JobRun>,
    now: Timestamp,
) -> JobView {
    let healthy_since = success.map(|run| run.finished_at.unwrap_or(run.started_at));
    let stale_after = delta(spec.every) * STALE_AFTER_INTERVALS;
    JobView {
        name: spec.name.to_owned(),
        every: describe(spec.every),
        last_started: latest.map(|run| run.started_at),
        last_finished: latest.and_then(|run| run.finished_at),
        last_outcome: latest.and_then(|run| run.outcome),
        last_message: latest.and_then(|run| run.message.clone()),
        next_due: due_at(spec, latest, failures),
        stale: healthy_since.is_none_or(|since| now - since > stale_after),
    }
}

fn describe(every: Duration) -> String {
    let seconds = every.as_secs();
    if seconds.is_multiple_of(DAY.as_secs()) {
        format!("{}d", seconds / DAY.as_secs())
    } else if seconds.is_multiple_of(HOUR.as_secs()) {
        format!("{}h", seconds / HOUR.as_secs())
    } else if seconds.is_multiple_of(60) {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_killed_before_it_finished_is_due_at_once() {
        let started_at = Timestamp::from_nanos(1_000_000_000);
        let killed = JobRun {
            id: 1,
            job: BARS_1D.to_owned(),
            started_at,
            finished_at: None,
            outcome: None,
            rows: 0,
            message: None,
        };
        let bars = JOBS.iter().find(|spec| spec.name == BARS_1D).unwrap();
        assert_eq!(due_at(bars, Some(&killed), 0), Some(started_at));
    }
}
