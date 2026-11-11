use std::error::Error;

use buizz_core::time::Timestamp;
use rusqlite::Row;
use rusqlite::types::{Type, ValueRef};
use rust_decimal::Decimal;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::ser::Error as _;
use serde_json::Value;

use crate::{Result, StateError};

pub fn enum_text<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value)? {
        Value::String(text) => Ok(text),
        _ => Err(StateError::Encoding(serde_json::Error::custom(
            "expected a unit enum",
        ))),
    }
}

pub fn json_text<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

pub fn int(value: u64) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|error| rusqlite::Error::ToSqlConversionFailure(error.into()))
}

fn conversion(index: usize, error: impl Into<Box<dyn Error + Send + Sync>>) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, error.into())
}

fn parsed<T, E: Into<Box<dyn Error + Send + Sync>>>(
    row: &Row,
    index: usize,
    parse: impl FnOnce(String) -> std::result::Result<T, E>,
) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    parse(text).map_err(|error| conversion(index, error))
}

pub fn read_enum<T: DeserializeOwned>(row: &Row, index: usize) -> rusqlite::Result<T> {
    parsed(row, index, |text| {
        serde_json::from_value(Value::String(text))
    })
}

pub fn read_json<T: DeserializeOwned>(row: &Row, index: usize) -> rusqlite::Result<T> {
    parsed(row, index, |text| serde_json::from_str(&text))
}

pub fn read_decimal(row: &Row, index: usize) -> rusqlite::Result<Decimal> {
    parsed(row, index, |text| text.parse::<Decimal>())
}

pub fn read_u64(row: &Row, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|error| conversion(index, error))
}

pub fn read_timestamp(row: &Row, index: usize) -> rusqlite::Result<Timestamp> {
    Ok(Timestamp::from_nanos(row.get(index)?))
}

pub fn read_optional<T>(
    row: &Row,
    index: usize,
    read: impl FnOnce(&Row, usize) -> rusqlite::Result<T>,
) -> rusqlite::Result<Option<T>> {
    if row.get_ref(index)? == ValueRef::Null {
        Ok(None)
    } else {
        read(row, index).map(Some)
    }
}
