use rust_decimal::{Decimal, RoundingStrategy};

pub const PRICE_DECIMALS: u32 = 4;
pub const CASH_DECIMALS: u32 = 2;

pub fn round_price(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(PRICE_DECIMALS, RoundingStrategy::MidpointNearestEven)
}

pub fn round_cash(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(CASH_DECIMALS, RoundingStrategy::MidpointNearestEven)
}
