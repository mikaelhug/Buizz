use buizz_core::market::{CorporateActionKind, Feed, Symbol};
use buizz_core::time::Timestamp;
use buizz_core::wire;
use duckdb::ToSql;
use duckdb::types::{
    Decimal as DbDecimal, FromSql, FromSqlError, FromSqlResult, TimeUnit, ToSqlOutput, Value,
    ValueRef,
};
use rust_decimal::Decimal;
use serde::Serialize;
use serde::de::DeserializeOwned;

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
        wire::from_name(value.as_str()?)
            .map(Self)
            .map_err(|error| FromSqlError::Other(Box::new(error)))
    }
}

pub(crate) fn wire_name<T: Serialize>(value: &T) -> String {
    wire::name(value).expect("market columns hold unit enum variants")
}

pub(crate) fn feed_name(feed: Feed) -> &'static str {
    match feed {
        Feed::Sip => "sip",
        Feed::Iex => "iex",
    }
}

pub(crate) struct FeedColumn(pub Feed);

impl FromSql for FeedColumn {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let name = value.as_str()?;
        [Feed::Sip, Feed::Iex]
            .into_iter()
            .find(|feed| feed_name(*feed) == name)
            .map(Self)
            .ok_or(FromSqlError::InvalidType)
    }
}

#[derive(Default)]
pub(crate) struct ActionColumns {
    pub kind: &'static str,
    pub other_symbol: String,
    pub old_rate: Option<Decimal>,
    pub new_rate: Option<Decimal>,
    pub rate: Option<Decimal>,
}

impl ActionColumns {
    pub(crate) fn of(kind: &CorporateActionKind) -> Self {
        let other = |symbol: &Symbol| symbol.as_str().to_owned();
        match kind {
            CorporateActionKind::Split { old_rate, new_rate } => Self {
                kind: "split",
                old_rate: Some(*old_rate),
                new_rate: Some(*new_rate),
                ..Self::default()
            },
            CorporateActionKind::CashDividend { rate } => Self {
                kind: "cash_dividend",
                rate: Some(*rate),
                ..Self::default()
            },
            CorporateActionKind::StockDividend { rate } => Self {
                kind: "stock_dividend",
                rate: Some(*rate),
                ..Self::default()
            },
            CorporateActionKind::SpinOff {
                new_symbol,
                old_rate,
                new_rate,
            } => Self {
                kind: "spin_off",
                other_symbol: other(new_symbol),
                old_rate: Some(*old_rate),
                new_rate: Some(*new_rate),
                rate: None,
            },
            CorporateActionKind::CashMerger { rate } => Self {
                kind: "cash_merger",
                rate: Some(*rate),
                ..Self::default()
            },
            CorporateActionKind::StockMerger {
                acquirer,
                old_rate,
                new_rate,
                cash_rate,
            } => Self {
                kind: "stock_merger",
                other_symbol: other(acquirer),
                old_rate: Some(*old_rate),
                new_rate: Some(*new_rate),
                rate: Some(*cash_rate),
            },
            CorporateActionKind::WorthlessRemoval => Self {
                kind: "worthless_removal",
                ..Self::default()
            },
            CorporateActionKind::NameChange { new_symbol } => Self {
                kind: "name_change",
                other_symbol: other(new_symbol),
                ..Self::default()
            },
        }
    }

    pub(crate) fn parse(
        kind: &str,
        other_symbol: &str,
        old_rate: Option<Decimal>,
        new_rate: Option<Decimal>,
        rate: Option<Decimal>,
    ) -> Option<CorporateActionKind> {
        let other = || (!other_symbol.is_empty()).then(|| Symbol::new(other_symbol));
        Some(match kind {
            "split" => CorporateActionKind::Split {
                old_rate: old_rate?,
                new_rate: new_rate?,
            },
            "cash_dividend" => CorporateActionKind::CashDividend { rate: rate? },
            "stock_dividend" => CorporateActionKind::StockDividend { rate: rate? },
            "spin_off" => CorporateActionKind::SpinOff {
                new_symbol: other()?,
                old_rate: old_rate?,
                new_rate: new_rate?,
            },
            "cash_merger" => CorporateActionKind::CashMerger { rate: rate? },
            "stock_merger" => CorporateActionKind::StockMerger {
                acquirer: other()?,
                old_rate: old_rate?,
                new_rate: new_rate?,
                cash_rate: rate?,
            },
            "worthless_removal" => CorporateActionKind::WorthlessRemoval,
            "name_change" => CorporateActionKind::NameChange {
                new_symbol: other()?,
            },
            _ => return None,
        })
    }
}

pub(crate) fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

pub(crate) type Params = Vec<Box<dyn ToSql>>;

pub(crate) const IN_SYMBOLS: &str = "IN (SELECT unnest(?::VARCHAR[]))";

pub(crate) fn symbol_list(symbols: &[Symbol]) -> Box<dyn ToSql> {
    Box::new(Value::List(
        symbols
            .iter()
            .map(|symbol| Value::Text(symbol.as_str().to_owned()))
            .collect(),
    ))
}
