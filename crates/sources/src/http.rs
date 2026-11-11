use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::RETRY_AFTER;
use reqwest::{RequestBuilder, Response, StatusCode};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, de};
use serde_json::Number;
use tokio::sync::Mutex;
use tokio::time::Instant;

pub(crate) const REDACTED: &str = "<redacted>";

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("rate limited, retry after {retry_after:?}")]
    RateLimited { retry_after: Option<Duration> },
    #[error("http status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("unexpected payload: {0}")]
    Decode(String),
    #[error("stream: {0}")]
    Stream(String),
    #[error("ftp: {0}")]
    Ftp(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimit {
    pub requests: u32,
    pub per: Duration,
}

impl RateLimit {
    fn interval(self) -> Duration {
        self.per / self.requests.max(1)
    }
}

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    interval: Duration,
    next_slot: Arc<Mutex<Instant>>,
}

impl HttpClient {
    pub fn new(user_agent: &str, limit: RateLimit) -> Result<Self, SourceError> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            client,
            interval: limit.interval(),
            next_slot: Arc::new(Mutex::new(Instant::now())),
        })
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub async fn send(&self, request: RequestBuilder) -> Result<Response, SourceError> {
        self.wait_for_slot().await;
        let response = request.send().await?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
                .map(Duration::from_secs);
            return Err(SourceError::RateLimited { retry_after });
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SourceError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(response)
    }

    async fn wait_for_slot(&self) {
        let mut next_slot = self.next_slot.lock().await;
        let now = Instant::now();
        let slot = (*next_slot).max(now);
        *next_slot = slot + self.interval;
        drop(next_slot);
        tokio::time::sleep_until(slot).await;
    }
}

pub(crate) fn json_decimal(number: &Number) -> Option<Decimal> {
    let text = number.to_string();
    Decimal::from_str(&text)
        .or_else(|_| Decimal::from_scientific(&text))
        .ok()
}

pub(crate) fn decimal<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Decimal, D::Error> {
    let number = Number::deserialize(deserializer)?;
    json_decimal(&number).ok_or_else(|| de::Error::custom(format!("{number} is not a decimal")))
}
