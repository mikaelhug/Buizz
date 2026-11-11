use buizz_core::time::Timestamp;
use chrono::{NaiveDate, Utc};

pub(crate) fn now() -> Timestamp {
    Timestamp::from_datetime(Utc::now()).expect("the current time fits in a timestamp")
}

pub(crate) fn exchange_today() -> NaiveDate {
    now().exchange_date()
}
