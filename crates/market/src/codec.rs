use buizz_core::market::{CorporateActionKind, Feed, Symbol};
use buizz_core::time::Timestamp;
use duckdb::ToSql;
use duckdb::types::{
    Decimal as DbDecimal, FromSql, FromSqlError, FromSqlResult, TimeUnit, ToSqlOutput, Value,
    ValueRef,
};
use rust_decimal::Decimal;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::de::value::{Error as WireError, StrDeserializer};

pub(crate) struct Ts(pub Timestamp);

impl ToSql for Ts {
    fn to_sql(&self) -> duckdb::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Timestamp(
            TimeUnit::Microsecond,
            self.0.nanos().div_euclid(
                crate::TIMESTAMP_RESOLUTION
                    .num_nanoseconds()
                    .expect("the resolution fits in nanoseconds"),
            ),
        )))
    }
}

impl FromSql for Ts {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let ValueRef::Timestamp(unit, amount) = value else {
            return Err(FromSqlError::InvalidType);
        };
        let per_unit = match unit {
            TimeUnit::Second => 1_000_000_000,
            TimeUnit::Millisecond => 1_000_000,
            TimeUnit::Microsecond => 1_000,
            TimeUnit::Nanosecond => 1,
        };
        amount
            .checked_mul(per_unit)
            .map(|nanos| Self(Timestamp::from_nanos(nanos)))
            .ok_or(FromSqlError::OutOfRange(amount.into()))
    }
}

pub(crate) struct Dec(pub Decimal);

impl ToSql for Dec {
    fn to_sql(&self) -> duckdb::Result<ToSqlOutput<'_>> {
        let decimal = DbDecimal::new(
            DbDecimal::MAX_WIDTH,
            self.0.scale() as u8,
            self.0.mantissa(),
        )?;
        Ok(ToSqlOutput::Owned(Value::Decimal(decimal)))
    }
}

impl FromSql for Dec {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let decimal = DbDecimal::column_result(value)?;
        Decimal::try_from_i128_with_scale(decimal.value(), decimal.scale().into())
            .map(Self)
            .map_err(|error| FromSqlError::Other(Box::new(error)))
    }
}

pub(crate) struct Sym(pub Symbol);

impl FromSql for Sym {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_str().map(|text| Self(Symbol::new(text)))
    }
}

pub(crate) struct Wire<T>(pub T);

impl<T: DeserializeOwned> FromSql for Wire<T> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        parse_wire(value.as_str()?)
            .map(Self)
            .map_err(|error| FromSqlError::Other(Box::new(error)))
    }
}

pub(crate) fn parse_wire<T: DeserializeOwned>(text: &str) -> Result<T, WireError> {
    T::deserialize(StrDeserializer::<WireError>::new(text))
}

pub(crate) fn wire_name<T: Serialize>(value: &T) -> String {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    let written = writer
        .serialize(value)
        .ok()
        .and_then(|()| writer.into_inner().ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    written
        .map(|text| text.trim_end().to_owned())
        .unwrap_or_default()
}

pub(crate) fn feed_name(feed: Feed) -> &'static str {
    match feed {
        Feed::Sip => "sip",
        Feed::Iex => "iex",
    }
}

pub(crate) struct ActionColumns {
    pub kind: &'static str,
    pub old_rate: Option<Dec>,
    pub new_rate: Option<Dec>,
    pub rate: Option<Dec>,
}

pub(crate) fn action_columns(kind: &CorporateActionKind) -> ActionColumns {
    match kind {
        CorporateActionKind::Split { old_rate, new_rate } => ActionColumns {
            kind: "split",
            old_rate: Some(Dec(*old_rate)),
            new_rate: Some(Dec(*new_rate)),
            rate: None,
        },
        CorporateActionKind::CashDividend { rate } => ActionColumns {
            kind: "cash_dividend",
            old_rate: None,
            new_rate: None,
            rate: Some(Dec(*rate)),
        },
    }
}

pub(crate) fn action_kind(
    kind: &str,
    old_rate: Option<Decimal>,
    new_rate: Option<Decimal>,
    rate: Option<Decimal>,
) -> Option<CorporateActionKind> {
    match (kind, old_rate, new_rate, rate) {
        ("split", Some(old_rate), Some(new_rate), _) => {
            Some(CorporateActionKind::Split { old_rate, new_rate })
        }
        ("cash_dividend", _, _, Some(rate)) => Some(CorporateActionKind::CashDividend { rate }),
        _ => None,
    }
}

pub(crate) fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

pub(crate) type Params = Vec<Box<dyn ToSql>>;

pub(crate) fn symbol_params(symbols: &[Symbol]) -> Params {
    symbols
        .iter()
        .map(|symbol| Box::new(symbol.as_str().to_owned()) as Box<dyn ToSql>)
        .collect()
}
