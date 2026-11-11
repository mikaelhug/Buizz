use rust_decimal::{Decimal, RoundingStrategy};

pub const PRICE_DECIMALS: u32 = 4;
pub const CASH_DECIMALS: u32 = 2;

pub fn round_price(value: Decimal) -> Decimal {
    round_price_with(value, RoundingStrategy::MidpointNearestEven)
}

pub fn round_price_with(value: Decimal, strategy: RoundingStrategy) -> Decimal {
    fixed(value, PRICE_DECIMALS, strategy)
}

pub fn round_cash(value: Decimal) -> Decimal {
    fixed(value, CASH_DECIMALS, RoundingStrategy::MidpointNearestEven)
}

fn fixed(value: Decimal, decimals: u32, strategy: RoundingStrategy) -> Decimal {
    let mut fixed = value.round_dp_with_strategy(decimals, strategy);
    fixed.rescale(decimals);
    fixed
}

macro_rules! wire_format {
    ($name:ident, $optional:ident, $round:path) => {
        pub mod $name {
            use rust_decimal::Decimal;
            use serde::{Deserialize, Deserializer, Serialize, Serializer};

            pub fn serialize<S: Serializer>(
                value: &Decimal,
                serializer: S,
            ) -> Result<S::Ok, S::Error> {
                Serialize::serialize(&$round(*value), serializer)
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
                value.map($round).serialize(serializer)
            }

            pub fn deserialize<'de, D: Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Option<Decimal>, D::Error> {
                Option::<Decimal>::deserialize(deserializer)
            }
        }
    };
}

wire_format!(cash, cash_option, super::round_cash);
wire_format!(price, price_option, super::round_price);
