use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::ser::Error as _;
use serde_json::Value;

pub fn name<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    match serde_json::to_value(value)? {
        Value::String(name) => Ok(name),
        _ => Err(serde_json::Error::custom("expected a unit enum variant")),
    }
}

pub fn from_name<T: DeserializeOwned>(name: &str) -> Result<T, serde_json::Error> {
    serde_json::from_value(Value::String(name.to_owned()))
}
