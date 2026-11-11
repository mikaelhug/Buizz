use chrono::{NaiveDate, TimeDelta};

use crate::market::{Calendar, Session, Timeframe};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CyclePhase {
    CorporateActions,
    OrderExpiry,
    Fills,
    Mark,
    RiskLimits,
    WakeBot,
    AcceptOrders,
}

impl CyclePhase {
    pub const ORDER: [Self; 7] = [
        Self::CorporateActions,
        Self::OrderExpiry,
        Self::Fills,
        Self::Mark,
        Self::RiskLimits,
        Self::WakeBot,
        Self::AcceptOrders,
    ];

    pub fn at_bar_open(self) -> bool {
        matches!(
            self,
            Self::CorporateActions | Self::OrderExpiry | Self::Fills
        )
    }
}
