use std::time::Duration;

use buizz_core::api::JobView;
use buizz_core::market::{DataSource, Dataset};
use buizz_core::ops::{JobOutcome, JobRun};
use buizz_core::time::Timestamp;
use chrono::TimeDelta;

use crate::JobSpec;

pub(crate) const CALENDAR: &str = "calendar";
pub(crate) const INSTRUMENTS: &str = "instruments";
pub(crate) const BARS_1D: &str = "bars_1d";
pub(crate) const BARS_1M: &str = "bars_1m";
pub(crate) const CORPORATE_ACTIONS: &str = "corporate_actions";
pub(crate) const SEC_INDEX: &str = "sec_index";
pub(crate) const DELISTINGS: &str = "delistings";
pub(crate) const FUNDAMENTALS: &str = "fundamentals";
pub(crate) const FILINGS: &str = "filings";
pub(crate) const INSIDER_TRADES: &str = "insider_trades";
pub(crate) const PRESS_RELEASES: &str = "press_releases";
pub(crate) const FILING_DOCUMENTS: &str = "filing_documents";
pub(crate) const FILING_SECTIONS: &str = "filing_sections";
pub(crate) const RECIPE_FACTS: &str = "recipe_facts";
pub(crate) const NEWS: &str = "news";
pub(crate) const INDEX_MEMBERSHIP: &str = "index_membership";
pub(crate) const RATES: &str = "rates";
pub(crate) const BORROW: &str = "borrow";
pub(crate) const OPTION_CHAINS: &str = "option_chains";
pub(crate) const TRADING_HALTS: &str = "trading_halts";
pub(crate) const ETF_HOLDINGS: &str = "etf_holdings";
pub(crate) const NASDAQ_NORDIC: &str = "nasdaq_nordic";
pub(crate) const EURONEXT_OSLO: &str = "euronext_oslo";
pub(crate) const FX_RATES: &str = "fx_rates";
pub(crate) const SHORT_INTEREST: &str = "short_interest";
pub(crate) const SHORT_VOLUME: &str = "short_volume";
pub(crate) const FAILS_TO_DELIVER: &str = "fails_to_deliver";
pub(crate) const PAGE_VIEWS: &str = "page_views";
pub(crate) const MACRO: &str = "macro";
pub(crate) const VIX: &str = "vix";
pub(crate) const PUT_CALL: &str = "put_call";
pub(crate) const FOMC: &str = "fomc";
pub(crate) const STATE_SNAPSHOT: &str = "state_snapshot";
pub(crate) const MARKET_SNAPSHOT: &str = "market_snapshot";
pub(crate) const TIINGO_HISTORY: &str = "tiingo_history";
pub(crate) const EARNINGS_INSIGHT: &str = "earnings_insight";
pub(crate) const EARNINGS_INSIGHT_ARCHIVE: &str = "earnings_insight_archive";

const FIVE_MINUTES: Duration = Duration::from_secs(5 * 60);
const TEN_MINUTES: Duration = Duration::from_secs(10 * 60);
const HOUR: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const SIX_HOURS: Duration = Duration::from_secs(6 * 60 * 60);
const WEEK: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const SHORT_RETRY: Duration = HOUR;
const FIRST_FAILURE_RETRY: Duration = Duration::from_secs(5 * 60);
const STALE_AFTER_INTERVALS: i32 = 3;

pub(crate) const JOBS: [JobSpec; 37] = [
    JobSpec {
        name: CALENDAR,
        source: Some(DataSource::Alpaca),
        datasets: &[Dataset::Calendar],
        every: DAY,
    },
    JobSpec {
        name: INSTRUMENTS,
        source: Some(DataSource::Alpaca),
        datasets: &[Dataset::Instruments],
        every: DAY,
    },
    JobSpec {
        name: INDEX_MEMBERSHIP,
        source: Some(DataSource::Wikipedia),
        datasets: &[Dataset::IndexMembership],
        every: DAY,
    },
    JobSpec {
        name: BARS_1D,
        source: Some(DataSource::Alpaca),
        datasets: &[Dataset::Bars1d],
        every: SIX_HOURS,
    },
    JobSpec {
        name: BARS_1M,
        source: Some(DataSource::Alpaca),
        datasets: &[Dataset::Bars1m],
        every: SIX_HOURS,
    },
    JobSpec {
        name: CORPORATE_ACTIONS,
        source: Some(DataSource::Alpaca),
        datasets: &[Dataset::CorporateActions],
        every: DAY,
    },
    JobSpec {
        name: SEC_INDEX,
        source: Some(DataSource::Edgar),
        datasets: &[Dataset::SecFilings],
        every: SIX_HOURS,
    },
    JobSpec {
        name: DELISTINGS,
        source: Some(DataSource::Edgar),
        datasets: &[Dataset::Delistings],
        every: SIX_HOURS,
    },
    JobSpec {
        name: FUNDAMENTALS,
        source: Some(DataSource::Edgar),
        datasets: &[Dataset::Fundamentals],
        every: SIX_HOURS,
    },
    JobSpec {
        name: FILINGS,
        source: Some(DataSource::Edgar),
        datasets: &[
            Dataset::Companies,
            Dataset::EarningsReleases,
            Dataset::CurrentReports,
            Dataset::PeriodicReports,
        ],
        every: SIX_HOURS,
    },
    JobSpec {
        name: INSIDER_TRADES,
        source: Some(DataSource::Edgar),
        datasets: &[Dataset::InsiderDataSets, Dataset::InsiderTrades],
        every: SIX_HOURS,
    },
    JobSpec {
        name: PRESS_RELEASES,
        source: Some(DataSource::Edgar),
        datasets: &[Dataset::PressReleases],
        every: SIX_HOURS,
    },
    JobSpec {
        name: FILING_DOCUMENTS,
        source: Some(DataSource::Edgar),
        datasets: &[Dataset::FilingDocuments],
        every: SIX_HOURS,
    },
    JobSpec {
        name: FILING_SECTIONS,
        source: None,
        datasets: &[Dataset::FilingSections],
        every: SIX_HOURS,
    },
    JobSpec {
        name: RECIPE_FACTS,
        source: None,
        datasets: &[Dataset::ListingNotices, Dataset::EarningsGuidance],
        every: SIX_HOURS,
    },
    JobSpec {
        name: RATES,
        source: Some(DataSource::Treasury),
        datasets: &[Dataset::Rates],
        every: DAY,
    },
    JobSpec {
        name: BORROW,
        source: Some(DataSource::InteractiveBrokers),
        datasets: &[Dataset::Borrow],
        every: DAY,
    },
    JobSpec {
        name: OPTION_CHAINS,
        source: Some(DataSource::Alpaca),
        datasets: &[Dataset::OptionChains],
        every: TEN_MINUTES,
    },
    JobSpec {
        name: TRADING_HALTS,
        source: Some(DataSource::Nasdaq),
        datasets: &[Dataset::TradingHalts],
        every: FIVE_MINUTES,
    },
    JobSpec {
        name: ETF_HOLDINGS,
        source: Some(DataSource::StateStreet),
        datasets: &[Dataset::EtfHoldings],
        every: DAY,
    },
    JobSpec {
        name: NASDAQ_NORDIC,
        source: Some(DataSource::NasdaqNordic),
        datasets: &[Dataset::NordicListings, Dataset::NordicBars1d],
        every: DAY,
    },
    JobSpec {
        name: EURONEXT_OSLO,
        source: Some(DataSource::Euronext),
        datasets: &[Dataset::NordicListings, Dataset::NordicBars1d],
        every: DAY,
    },
    JobSpec {
        name: FX_RATES,
        source: Some(DataSource::Ecb),
        datasets: &[Dataset::FxRates],
        every: DAY,
    },
    JobSpec {
        name: SHORT_INTEREST,
        source: Some(DataSource::Finra),
        datasets: &[Dataset::ShortInterest],
        every: DAY,
    },
    JobSpec {
        name: SHORT_VOLUME,
        source: Some(DataSource::Finra),
        datasets: &[Dataset::ShortVolume],
        every: DAY,
    },
    JobSpec {
        name: FAILS_TO_DELIVER,
        source: Some(DataSource::Edgar),
        datasets: &[Dataset::FailsToDeliver],
        every: DAY,
    },
    JobSpec {
        name: PAGE_VIEWS,
        source: Some(DataSource::Wikipedia),
        datasets: &[Dataset::PageViews],
        every: DAY,
    },
    JobSpec {
        name: MACRO,
        source: Some(DataSource::Fred),
        datasets: &[Dataset::Macro],
        every: DAY,
    },
    JobSpec {
        name: NEWS,
        source: Some(DataSource::Alpaca),
        datasets: &[Dataset::News],
        every: SIX_HOURS,
    },
    JobSpec {
        name: VIX,
        source: Some(DataSource::Cboe),
        datasets: &[Dataset::Macro],
        every: DAY,
    },
    JobSpec {
        name: PUT_CALL,
        source: Some(DataSource::Cboe),
        datasets: &[Dataset::Macro],
        every: DAY,
    },
    JobSpec {
        name: FOMC,
        source: Some(DataSource::FederalReserve),
        datasets: &[Dataset::Fomc],
        every: DAY,
    },
    JobSpec {
        name: EARNINGS_INSIGHT,
        source: Some(DataSource::Factset),
        datasets: &[Dataset::Estimates],
        every: DAY,
    },
    JobSpec {
        name: EARNINGS_INSIGHT_ARCHIVE,
        source: Some(DataSource::Factset),
        datasets: &[Dataset::Estimates],
        every: WEEK,
    },
    JobSpec {
        name: STATE_SNAPSHOT,
        source: None,
        datasets: &[],
        every: DAY,
    },
    JobSpec {
        name: MARKET_SNAPSHOT,
        source: None,
        datasets: &[],
        every: DAY,
    },
    JobSpec {
        name: TIINGO_HISTORY,
        source: Some(DataSource::Tiingo),
        datasets: &[Dataset::Bars1d, Dataset::CorporateActions],
        every: HOUR,
    },
];

fn delta(duration: Duration) -> TimeDelta {
    TimeDelta::from_std(duration).unwrap_or(TimeDelta::MAX)
}

pub(crate) const FOLLOWERS: [&str; 3] = [FILING_DOCUMENTS, FILING_SECTIONS, RECIPE_FACTS];

pub(crate) const FOUNDATION: [&str; 4] = [CALENDAR, INSTRUMENTS, INDEX_MEMBERSHIP, BARS_1D];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    Source(Option<DataSource>),
    Own(&'static str),
}

fn lane(spec: &JobSpec) -> Lane {
    match spec.name {
        TIINGO_HISTORY => Lane::Source(Some(DataSource::Alpaca)),
        FILING_DOCUMENTS | FILING_SECTIONS | RECIPE_FACTS => Lane::Own(FILING_DOCUMENTS),
        OPTION_CHAINS => Lane::Own(spec.name),
        _ => Lane::Source(spec.source),
    }
}

pub(crate) fn lanes() -> Vec<Vec<&'static JobSpec>> {
    let mut lanes: Vec<(Lane, Vec<&'static JobSpec>)> = Vec::new();
    for spec in &JOBS {
        let lane = lane(spec);
        match lanes.iter_mut().find(|(source, _)| *source == lane) {
            Some((_, jobs)) => jobs.push(spec),
            None => lanes.push((lane, vec![spec])),
        }
    }
    lanes.into_iter().map(|(_, jobs)| jobs).collect()
}

pub(crate) fn due_at(
    spec: &JobSpec,
    latest: Option<&JobRun>,
    failures: u32,
    limited_until: Option<Timestamp>,
    settles: Option<Timestamp>,
) -> Option<Timestamp> {
    let run = latest?;
    let settles = settles.filter(|at| *at > run.started_at);
    let wait = match run.outcome {
        Some(JobOutcome::Backoff) => {
            if let Some(until) = limited_until.filter(|until| *until > run.started_at) {
                return Some(until);
            }
            SHORT_RETRY
        }
        Some(JobOutcome::Waiting) => SHORT_RETRY,
        Some(JobOutcome::Failed) | None => {
            FIRST_FAILURE_RETRY.saturating_mul(1 << failures.saturating_sub(1).min(16))
        }
        Some(JobOutcome::Ok | JobOutcome::Noop) => spec.every,
        Some(JobOutcome::Partial) => Duration::ZERO,
    };
    let due = run.started_at + delta(wait.min(spec.every));
    Some(settles.map_or(due, |at| due.min(at)))
}

pub(crate) fn view(
    spec: &JobSpec,
    latest: Option<&JobRun>,
    next_due: Option<Timestamp>,
    success: Option<&JobRun>,
    first_due: Timestamp,
    now: Timestamp,
) -> JobView {
    let stale_after = delta(spec.every) * STALE_AFTER_INTERVALS;
    let stale = match success {
        Some(run) => now - run.finished_at.unwrap_or(run.started_at) > stale_after,
        None => {
            latest.is_some_and(|run| run.outcome == Some(JobOutcome::Failed))
                || now - first_due > stale_after
        }
    };
    JobView {
        name: spec.name.to_owned(),
        source: spec.source,
        datasets: spec.datasets.to_vec(),
        every: describe(spec.every),
        last_started: latest.map(|run| run.started_at),
        last_finished: latest.and_then(|run| run.finished_at),
        last_outcome: latest.and_then(|run| run.outcome),
        last_message: latest.and_then(|run| run.message.clone()),
        next_due,
        stale,
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
    fn a_run_killed_before_it_finished_backs_off_like_a_failure() {
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
        let retry =
            |failures| due_at(bars, Some(&killed), failures, None, None).unwrap() - started_at;
        assert_eq!(retry(1), TimeDelta::minutes(5));
        assert_eq!(retry(3), TimeDelta::minutes(20));
    }

    #[test]
    fn daily_bars_are_due_when_a_session_settles() {
        let started_at = Timestamp::from_nanos(1_000_000_000);
        let settles = started_at + TimeDelta::hours(2);
        let bars = JOBS.iter().find(|spec| spec.name == BARS_1D).unwrap();
        let run = |outcome| JobRun {
            id: 1,
            job: BARS_1D.to_owned(),
            started_at,
            finished_at: Some(started_at),
            outcome: Some(outcome),
            rows: 0,
            message: None,
        };
        let due = |outcome| due_at(bars, Some(&run(outcome)), 0, None, Some(settles)).unwrap();
        assert_eq!(due(JobOutcome::Ok), settles);
        assert_eq!(due(JobOutcome::Partial), started_at);
    }

    #[test]
    fn a_job_on_its_first_run_is_not_late_until_three_intervals_pass() {
        let booted = Timestamp::from_nanos(1_000_000_000);
        let fails = JOBS
            .iter()
            .find(|spec| spec.name == FAILS_TO_DELIVER)
            .unwrap();
        let running = JobRun {
            id: 1,
            job: FAILS_TO_DELIVER.to_owned(),
            started_at: booted,
            finished_at: None,
            outcome: None,
            rows: 0,
            message: None,
        };
        let stale = |latest: Option<&JobRun>, after: TimeDelta| {
            view(fails, latest, None, None, booted, booted + after).stale
        };
        assert!(!stale(None, TimeDelta::hours(1)));
        assert!(!stale(Some(&running), TimeDelta::hours(1)));
        assert!(stale(Some(&running), TimeDelta::days(4)));
        let failed = JobRun {
            outcome: Some(JobOutcome::Failed),
            ..running
        };
        assert!(stale(Some(&failed), TimeDelta::hours(1)));
    }
}
