use rust_decimal::{Decimal, RoundingStrategy};

pub const PRICE_DECIMALS: u32 = 4;
pub const CASH_DECIMALS: u32 = 2;

pub fn round_price(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(PRICE_DECIMALS, RoundingStrategy::MidpointNearestEven)
}

pub fn round_cash(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(CASH_DECIMALS, RoundingStrategy::MidpointNearestEven)
}

fn fixed(value: Decimal, decimals: u32) -> Decimal {
    let mut fixed = value.round_dp_with_strategy(decimals, RoundingStrategy::MidpointNearestEven);
    fixed.rescale(decimals);
    fixed
}

macro_rules! wire_format {
    ($name:ident, $optional:ident, $decimals:expr) => {
        pub mod $name {
            use rust_decimal::Decimal;
            use serde::{Deserialize, Deserializer, Serialize, Serializer};

            pub fn serialize<S: Serializer>(
                value: &Decimal,
                serializer: S,
            ) -> Result<S::Ok, S::Error> {
                Serialize::serialize(&super::fixed(*value, $decimals), serializer)
            }

            pub fn deserialize<'de, D: Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Decimal, D::Error> {
                <Decimal as Deserialize>::deserialize(deserializer)
            }
        }

        pub mod $optional {
            use rust_decimal::Decimal;
            use serde::{Deserialize, Deserializer, Serialize, Serializer};

            pub fn serialize<S: Serializer>(
                value: &Option<Decimal>,
                serializer: S,
            ) -> Result<S::Ok, S::Error> {
                value
                    .map(|value| super::fixed(value, $decimals))
                    .serialize(serializer)
            }

            pub fn deserialize<'de, D: Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Option<Decimal>, D::Error> {
                Option::<Decimal>::deserialize(deserializer)
            }
        }
    };
}

wire_format!(cash, cash_option, super::CASH_DECIMALS);
wire_format!(price, price_option, super::PRICE_DECIMALS);
