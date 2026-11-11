use std::fmt;
use std::ops::{Add, Sub};
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, NaiveTime, SecondsFormat, TimeDelta, TimeZone, Utc};
use chrono_tz::America::New_York;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type};
use utoipa::openapi::{RefOr, Schema};
use utoipa::{PartialSchema, ToSchema};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const MIN: Self = Self(i64::MIN);
    pub const MAX: Self = Self(i64::MAX);

    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    pub const fn nanos(self) -> i64 {
        self.0
    }

    pub fn now() -> Self {
        Self::from_datetime(Utc::now()).expect("the current time fits in i64 nanoseconds")
    }

    pub fn from_datetime(datetime: DateTime<Utc>) -> Option<Self> {
        datetime.timestamp_nanos_opt().map(Self)
    }

    pub fn to_datetime(self) -> DateTime<Utc> {
        DateTime::from_timestamp_nanos(self.0)
    }

    pub fn at_exchange_time(date: NaiveDate, time: NaiveTime) -> Option<Self> {
        New_York
            .from_local_datetime(&date.and_time(time))
            .single()
            .and_then(|local| Self::from_datetime(local.with_timezone(&Utc)))
    }

    pub fn exchange_date(self) -> NaiveDate {
        self.to_datetime().with_timezone(&New_York).date_naive()
    }

    pub fn exchange_time(self) -> NaiveTime {
        self.to_datetime().with_timezone(&New_York).time()
    }
}

impl Add<TimeDelta> for Timestamp {
    type Output = Self;

    fn add(self, delta: TimeDelta) -> Self {
        let nanos = delta
            .num_nanoseconds()
            .expect("time delta fits in i64 nanoseconds");
        Self(self.0 + nanos)
    }
}

impl Sub<TimeDelta> for Timestamp {
    type Output = Self;

    fn sub(self, delta: TimeDelta) -> Self {
        let nanos = delta
            .num_nanoseconds()
            .expect("time delta fits in i64 nanoseconds");
        Self(self.0 - nanos)
    }
}

impl Sub for Timestamp {
    type Output = TimeDelta;

    fn sub(self, other: Self) -> TimeDelta {
        TimeDelta::nanoseconds(self.0 - other.0)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            &self
                .to_datetime()
                .to_rfc3339_opts(SecondsFormat::AutoSi, true),
        )
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid RFC 3339 timestamp: {0}")]
pub struct ParseTimestampError(String);

impl FromStr for Timestamp {
    type Err = ParseTimestampError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        DateTime::parse_from_rfc3339(text)
            .ok()
            .and_then(|parsed| Self::from_datetime(parsed.with_timezone(&Utc)))
            .ok_or_else(|| ParseTimestampError(text.to_owned()))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.collect_str(self)
        } else {
            serializer.serialize_i64(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TimestampVisitor;

        impl Visitor<'_> for TimestampVisitor {
            type Value = Timestamp;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an RFC 3339 string or integer nanoseconds since the Unix epoch")
            }

            fn visit_i64<E: de::Error>(self, nanos: i64) -> Result<Timestamp, E> {
                Ok(Timestamp(nanos))
            }

            fn visit_u64<E: de::Error>(self, nanos: u64) -> Result<Timestamp, E> {
                i64::try_from(nanos)
                    .map(Timestamp)
                    .map_err(|_| E::custom("timestamp out of range"))
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Timestamp, E> {
                text.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_any(TimestampVisitor)
    }
}

impl PartialSchema for Timestamp {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::String)
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::DateTime)))
            .into()
    }
}

impl ToSchema for Timestamp {}
