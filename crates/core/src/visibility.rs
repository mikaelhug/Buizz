use chrono::{Datelike, NaiveDate, NaiveTime, TimeDelta};

use crate::market::{Calendar, FomcMeeting, Session, Timeframe};
use crate::time::Timestamp;

pub fn bar_visible_at(timeframe: Timeframe, start: Timestamp, session: &Session) -> Timestamp {
    match timeframe {
        Timeframe::Minute => start + TimeDelta::seconds(60),
        Timeframe::Day => session.close,
    }
}

pub fn fundamental_visible_at(filed: NaiveDate, calendar: &Calendar) -> Option<Timestamp> {
    calendar.next_after(filed).map(|session| session.open)
}

pub fn corporate_action_visible_at(ex_date: NaiveDate, calendar: &Calendar) -> Option<Timestamp> {
    calendar
        .first_on_or_after(ex_date)
        .map(|session| session.open)
}

pub fn order_session(
    timeframe: Timeframe,
    decided_at: Timestamp,
    calendar: &Calendar,
) -> Option<&Session> {
    let sessions = calendar.sessions();
    let index = sessions.partition_point(|session| session.last_bar_start(timeframe) < decided_at);
    sessions.get(index)
}

pub fn open_auction_session(decided_at: Timestamp, calendar: &Calendar) -> Option<&Session> {
    let sessions = calendar.sessions();
    sessions.get(sessions.partition_point(|session| session.open <= decided_at))
}

pub fn filing_visible_at(accepted_at: Timestamp, calendar: &Calendar) -> Option<Timestamp> {
    open_auction_session(accepted_at, calendar).map(|session| session.open)
}

pub fn close_auction_session(decided_at: Timestamp, calendar: &Calendar) -> Option<&Session> {
    let sessions = calendar.sessions();
    sessions.get(sessions.partition_point(|session| session.close <= decided_at))
}

pub const CALENDAR_HORIZON: TimeDelta = TimeDelta::days(366);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LateClosure {
    pub date: NaiveDate,
    pub announced: NaiveDate,
}

const fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
}

pub const LATE_CLOSURES: [LateClosure; 3] = [
    LateClosure {
        date: day(2018, 12, 5),
        announced: day(2018, 12, 1),
    },
    LateClosure {
        date: day(2022, 6, 20),
        announced: day(2021, 10, 5),
    },
    LateClosure {
        date: day(2025, 1, 9),
        announced: day(2024, 12, 30),
    },
];

const REGULAR_OPEN: NaiveTime = NaiveTime::from_hms_opt(9, 30, 0).expect("valid time");
const REGULAR_CLOSE: NaiveTime = NaiveTime::from_hms_opt(16, 0, 0).expect("valid time");

pub fn calendar_known_through(clock: Timestamp) -> NaiveDate {
    clock.exchange_date() + CALENDAR_HORIZON
}

pub fn published_visible_at(published: NaiveDate, calendar: &Calendar) -> Option<Timestamp> {
    calendar.next_after(published).map(|session| session.open)
}

pub const SHORT_INTEREST_SESSIONS_TO_PUBLICATION: usize = 8;

pub fn short_interest_published(settlement: NaiveDate, calendar: &Calendar) -> Option<NaiveDate> {
    let sessions = calendar.sessions();
    let after = sessions.partition_point(|session| session.date <= settlement);
    sessions
        .get(after + SHORT_INTEREST_SESSIONS_TO_PUBLICATION - 1)
        .map(|session| session.date)
}

pub const FAILS_HALF_MONTHS_FROM: NaiveDate = day(2009, 7, 1);
const FAILS_FIRST_HALF_ENDS: u32 = 15;

fn month_end(year: i32, month: u32) -> NaiveDate {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    day(next_year, next_month, 1) - TimeDelta::days(1)
}

fn months_later(date: NaiveDate, months: u32) -> (i32, u32) {
    let index = date.year() * 12 + i32::try_from(date.month0() + months).unwrap_or(0);
    (
        index.div_euclid(12),
        u32::try_from(index.rem_euclid(12)).unwrap_or(0) + 1,
    )
}

pub fn fails_to_deliver_published(settlement: NaiveDate, downloaded: NaiveDate) -> NaiveDate {
    let assumed = if settlement < FAILS_HALF_MONTHS_FROM {
        let (year, month) = months_later(settlement, 2);
        month_end(year, month)
    } else {
        let (year, month) = months_later(settlement, 1);
        if settlement.day() <= FAILS_FIRST_HALF_ENDS {
            day(year, month, FAILS_FIRST_HALF_ENDS)
        } else {
            month_end(year, month)
        }
    };
    assumed.min(downloaded)
}

pub const PAGE_VIEWS_RECORDED_WITHIN: TimeDelta = TimeDelta::days(7);

pub fn page_views_published(date: NaiveDate, downloaded: NaiveDate) -> NaiveDate {
    let morning_after = date + TimeDelta::days(1);
    if downloaded <= date + PAGE_VIEWS_RECORDED_WITHIN {
        downloaded.max(morning_after)
    } else {
        morning_after
    }
}

pub fn page_views_visible_at(published: NaiveDate, calendar: &Calendar) -> Option<Timestamp> {
    calendar
        .first_on_or_after(published)
        .map(|session| session.open)
}

pub fn fomc_visible_at(meeting: &FomcMeeting, calendar: &Calendar) -> Option<Timestamp> {
    if meeting.scheduled {
        Timestamp::at_exchange_time(
            NaiveDate::from_ymd_opt(meeting.date.year(), 1, 1)?,
            NaiveTime::MIN,
        )
    } else {
        published_visible_at(meeting.date, calendar)
    }
}

pub fn published_before(clock: Timestamp, calendar: &Calendar) -> Option<NaiveDate> {
    let sessions = calendar.sessions();
    let opened = sessions.partition_point(|session| session.open <= clock);
    opened.checked_sub(1).map(|index| sessions[index].date)
}

pub fn published_sessions(
    calendar: &Calendar,
    from: NaiveDate,
    to: NaiveDate,
    clock: Timestamp,
) -> Vec<Session> {
    let mut sessions = calendar.between(from, to).to_vec();
    let unannounced = LATE_CLOSURES.iter().filter(|closure| {
        (from..=to).contains(&closure.date)
            && calendar.session(closure.date).is_none()
            && calendar.previous_before(closure.date).is_some()
            && calendar.next_after(closure.date).is_some()
            && published_visible_at(closure.announced, calendar).is_none_or(|known| known > clock)
    });
    sessions.extend(unannounced.filter_map(|closure| {
        Some(Session {
            date: closure.date,
            open: Timestamp::at_exchange_time(closure.date, REGULAR_OPEN)?,
            close: Timestamp::at_exchange_time(closure.date, REGULAR_CLOSE)?,
        })
    }));
    sessions.sort_by_key(|session| session.date);
    sessions
}

pub fn universe_visible_at(date: NaiveDate, calendar: &Calendar) -> Timestamp {
    calendar
        .previous_before(date)
        .map_or(Timestamp::MIN, |session| session.close)
}
