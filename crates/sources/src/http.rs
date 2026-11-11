use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, RETRY_AFTER};
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
    pub cooldown: Duration,
}

const RETRIES: u32 = 3;
const FIRST_RETRY_DELAY: Duration = Duration::from_millis(500);
const MAX_INLINE_WAIT: Duration = Duration::from_secs(60);
const RATE_LIMIT_RESET: &str = "x-ratelimit-reset";

impl RateLimit {
    fn interval(self) -> Duration {
        self.per / self.requests.max(1)
    }
}

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    interval: Duration,
    cooldown: Duration,
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
            cooldown: limit.cooldown,
            next_slot: Arc::new(Mutex::new(Instant::now())),
        })
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub async fn send(&self, request: RequestBuilder) -> Result<Response, SourceError> {
        let mut retry_delay = FIRST_RETRY_DELAY;
        for attempt in 0..=RETRIES {
            let last = attempt == RETRIES;
            let Some(this) = request.try_clone() else {
                return self.send_once(request).await;
            };
            match self.send_once(this).await {
                Err(SourceError::RateLimited { retry_after })
                    if !last && retry_after.is_some_and(|wait| wait <= MAX_INLINE_WAIT) =>
                {
                    self.hold(retry_after.unwrap_or_default()).await;
                }
                Err(error) if !last && transient(&error) => {
                    tokio::time::sleep(retry_delay).await;
                    retry_delay *= 2;
                }
                outcome => return outcome,
            }
        }
        unreachable!("the last attempt returns")
    }

    async fn send_once(&self, request: RequestBuilder) -> Result<Response, SourceError> {
        self.wait_for_slot().await;
        let response = request.send().await?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = retry_after(response.headers()).unwrap_or(self.cooldown);
            return Err(SourceError::RateLimited {
                retry_after: Some(wait),
            });
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

    async fn hold(&self, wait: Duration) {
        let mut next_slot = self.next_slot.lock().await;
        let until = Instant::now() + wait;
        *next_slot = (*next_slot).max(until);
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

fn transient(error: &SourceError) -> bool {
    match error {
        SourceError::Http(error) => error.is_timeout() || error.is_connect() || error.is_request(),
        SourceError::Status { status, .. } => *status >= 500,
        _ => false,
    }
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let number =
        |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.trim().parse().ok() };
    if let Some(seconds) = number(RETRY_AFTER.as_str()) {
        return Some(Duration::from_secs(seconds));
    }
    let reset = number(RATE_LIMIT_RESET)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(Duration::from_secs(reset.saturating_sub(now).max(1)))
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
