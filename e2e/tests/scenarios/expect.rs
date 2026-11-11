use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::world::repository;

pub fn scenario(name: &str) -> Value {
    let path = repository().join(format!("fixtures/scenarios/{name}.expected.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("scenario expectations are JSON")
}

pub fn assert_same(expected: &Value, actual: &Value, tolerance: f64, at: &str) {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, value) in expected {
                let actual = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("{at}.{key} is missing"));
                assert_same(value, actual, tolerance, &format!("{at}.{key}"));
            }
        }
        (Value::Array(expected), Value::Array(actual)) => {
            assert_eq!(expected.len(), actual.len(), "{at} length: {actual:?}");
            for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
                assert_same(expected, actual, tolerance, &format!("{at}[{index}]"));
            }
        }
        (Value::String(expected), Value::String(actual)) => {
            match (Decimal::from_str(expected), Decimal::from_str(actual)) {
                (Ok(expected), Ok(actual)) => assert_eq!(expected, actual, "{at}"),
                _ => assert_eq!(expected, actual, "{at}"),
            }
        }
        (Value::Number(expected), Value::Number(actual))
            if expected.is_f64() || actual.is_f64() =>
        {
            let (expected, actual) = (
                expected.as_f64().expect("a number"),
                actual.as_f64().expect("a number"),
            );
            assert!(
                (expected - actual).abs() <= tolerance,
                "{at}: {actual} vs {expected}"
            );
        }
        _ => assert_eq!(expected, actual, "{at}"),
    }
}
