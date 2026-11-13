mod wire;

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::RequestBuilder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::http::{HttpClient, REDACTED, RateLimit, SourceError, decode_json, transient};
use wire::{WireMessage, WireResult, WireResultLine};

pub const BASE_URL: &str = "https://api.anthropic.com";
pub const KEY_ENV: &str = "ANTHROPIC_API_KEY";
pub const URL_ENV: &str = "ANTHROPIC_URL";
pub const SPEND_CAP_ENV: &str = "ANTHROPIC_MONTHLY_CAP_USD";
pub const DEFAULT_SPEND_CAP: Decimal = Decimal::from_parts(5, 0, 0, false, 0);
pub const MODEL: &str = "claude-haiku-5-5";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 50,
    per: Duration::from_secs(60),
    cooldown: Duration::from_secs(60),
};
const API_VERSION: &str = "2026-10-21";
const TIMEOUT: Duration = Duration::from_secs(600);
const MESSAGES_PATH: &str = "/v1/messages";
const BATCHES_PATH: &str = "/v1/messages/batches";
const TOKENS_PER_PRICE_UNIT: Decimal = Decimal::from_parts(1_000_000, 0, 0, false, 0);
const SHORT_PROMPT_TOKENS: u64 = 100_000;
const SHORT_PROMPT: Rates = Rates {
    input: Decimal::from_parts(10, 0, 0, false, 2),
    output: Decimal::from_parts(50, 0, 0, false, 2),
};
const LONG_PROMPT: Rates = Rates {
    input: Decimal::from_parts(50, 0, 0, false, 2),
    output: Decimal::from_parts(250, 0, 0, false, 2),
};
const CACHE_WRITE_MULTIPLIER: Decimal = Decimal::from_parts(125, 0, 0, false, 2);
const CACHE_READ_MULTIPLIER: Decimal = Decimal::from_parts(1, 0, 0, false, 1);
const BATCH_DISCOUNT: Decimal = Decimal::from_parts(5, 0, 0, false, 1);

#[derive(Clone, PartialEq, Eq)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub base_url: String,
    pub spend_cap: Decimal,
}

impl fmt::Debug for AnthropicConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicConfig")
            .field("api_key", &REDACTED)
            .field("base_url", &self.base_url)
            .field("spend_cap", &self.spend_cap)
            .finish()
    }
}

impl AnthropicConfig {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: BASE_URL.to_owned(),
            spend_cap: DEFAULT_SPEND_CAP,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClaudeError {
    #[error("the Anthropic API is unavailable for now: {0}")]
    Unavailable(SourceError),
    #[error("the Anthropic API rejected the request: {0}")]
    Rejected(SourceError),
    #[error("the spend cap of ${} is reached: ${} spent", .0.cap, .0.spent.round_dp(2))]
    SpendCapReached(Spend),
}

impl ClaudeError {
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

impl From<SourceError> for ClaudeError {
    fn from(error: SourceError) -> Self {
        if matches!(error, SourceError::RateLimited { .. }) || transient(&error) {
            Self::Unavailable(error)
        } else {
            Self::Rejected(error)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Spend {
    pub spent: Decimal,
    pub cap: Decimal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

struct Rates {
    input: Decimal,
    output: Decimal,
}

impl Usage {
    pub fn cost(&self, batched: bool) -> Decimal {
        let prompt_tokens =
            self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens;
        let rates = if prompt_tokens <= SHORT_PROMPT_TOKENS {
            SHORT_PROMPT
        } else {
            LONG_PROMPT
        };
        let input = Decimal::from(self.input_tokens)
            + Decimal::from(self.cache_creation_input_tokens) * CACHE_WRITE_MULTIPLIER
            + Decimal::from(self.cache_read_input_tokens) * CACHE_READ_MULTIPLIER;
        let list = (input * rates.input + Decimal::from(self.output_tokens) * rates.output)
            / TOKENS_PER_PRICE_UNIT;
        if batched { list * BATCH_DISCOUNT } else { list }
    }
}

pub struct Prompt {
    pub system: String,
    pub schema: Value,
    pub max_tokens: u32,
}

pub struct Retry<'a> {
    pub answer: &'a str,
    pub failure: &'a str,
}

impl Prompt {
    pub fn worst_case(&self, document: &str, batched: bool) -> Decimal {
        let prompt_bytes =
            (self.system.len() + self.schema.to_string().len() + document.len()) as u64;
        Usage {
            input_tokens: prompt_bytes,
            output_tokens: u64::from(self.max_tokens),
            ..Usage::default()
        }
        .cost(batched)
    }

    fn params(&self, document: &str, retry: Option<&Retry<'_>>) -> Value {
        let mut messages = vec![json!({"role": "user", "content": document})];
        if let Some(retry) = retry {
            messages.push(json!({"role": "assistant", "content": retry.answer}));
            messages.push(json!({
                "role": "user",
                "content": format!(
                    "Your answer failed these checks: {}. Answer again with the whole list of facts, \
                     correcting them.",
                    retry.failure
                ),
            }));
        }
        json!({
            "model": MODEL,
            "max_tokens": self.max_tokens,
            "thinking": {"type": "disabled"},
            "output_config": {"format": {"type": "json_schema", "schema": self.schema}},
            "system": [{
                "type": "text",
                "text": self.system,
                "cache_control": {"type": "ephemeral"},
            }],
            "messages": messages,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Json(Value),
    Refused { category: Option<String> },
    Truncated,
    Malformed { text: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub answer: Answer,
    pub usage: Usage,
    pub cost: Decimal,
}

pub struct BatchRequest {
    pub custom_id: String,
    pub document: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    InProgress,
    Canceling,
    Ended,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct RequestCounts {
    pub processing: u64,
    pub succeeded: u64,
    pub errored: u64,
    pub canceled: u64,
    pub expired: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Batch {
    pub id: String,
    pub processing_status: BatchStatus,
    pub request_counts: RequestCounts,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BatchOutcome {
    Answered(Reply),
    Errored { kind: String, retryable: bool },
    Canceled,
    Expired,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BatchResult {
    pub custom_id: String,
    pub outcome: BatchOutcome,
}

#[derive(Clone)]
pub struct AnthropicClient {
    http: HttpClient,
    config: AnthropicConfig,
    spent: Arc<Mutex<Decimal>>,
}

impl AnthropicClient {
    pub fn new(config: AnthropicConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::with_timeout("buizz", RATE_LIMIT, TIMEOUT)?,
            config: AnthropicConfig {
                base_url: config.base_url.trim_end_matches('/').to_owned(),
                ..config
            },
            spent: Arc::default(),
        })
    }

    pub fn budget(&self, cap: Decimal) -> Self {
        Self {
            http: self.http.clone(),
            config: AnthropicConfig {
                spend_cap: cap.min(self.config.spend_cap),
                ..self.config.clone()
            },
            spent: Arc::default(),
        }
    }

    pub fn cap(&self) -> Decimal {
        self.config.spend_cap
    }

    pub fn spend(&self) -> Spend {
        Spend {
            spent: *self.spent.lock().unwrap_or_else(PoisonError::into_inner),
            cap: self.config.spend_cap,
        }
    }

    pub async fn message(
        &self,
        prompt: &Prompt,
        document: &str,
        retry: Option<&Retry<'_>>,
    ) -> Result<Reply, ClaudeError> {
        let mut worst = prompt.worst_case(document, false);
        if let Some(retry) = retry {
            worst += prompt.worst_case(retry.answer, false);
        }
        self.within_cap(worst)?;
        let request = self
            .authorized(self.http.client().post(self.url(MESSAGES_PATH)))
            .json(&prompt.params(document, retry));
        let body = self.http.fetch(request).await?;
        Ok(self.reply(decode_json(&body)?, false))
    }

    pub async fn create_batch(
        &self,
        prompt: &Prompt,
        requests: &[BatchRequest],
    ) -> Result<Batch, ClaudeError> {
        let worst = requests
            .iter()
            .map(|request| prompt.worst_case(&request.document, true))
            .sum();
        self.within_cap(worst)?;
        let requests: Vec<Value> = requests
            .iter()
            .map(|request| {
                json!({
                    "custom_id": request.custom_id,
                    "params": prompt.params(&request.document, None),
                })
            })
            .collect();
        let request = self
            .authorized(self.http.client().post(self.url(BATCHES_PATH)))
            .json(&json!({ "requests": requests }));
        let batch = decode_json(&self.http.fetch(request).await?)?;
        *self.spent.lock().unwrap_or_else(PoisonError::into_inner) += worst;
        Ok(batch)
    }

    pub async fn batch(&self, id: &str) -> Result<Batch, ClaudeError> {
        let url = self.url(&format!("{BATCHES_PATH}/{id}"));
        let request = self.authorized(self.http.client().get(url));
        Ok(decode_json(&self.http.fetch(request).await?)?)
    }

    pub async fn batch_results(&self, id: &str) -> Result<BatchResults, ClaudeError> {
        let url = self.url(&format!("{BATCHES_PATH}/{id}/results"));
        let request = self.authorized(self.http.client().get(url));
        Ok(BatchResults {
            client: self.clone(),
            response: self.http.open(request).await?,
            pending: Vec::new(),
            finished: false,
        })
    }

    fn within_cap(&self, worst: Decimal) -> Result<(), ClaudeError> {
        let spend = self.spend();
        if spend.spent + worst > spend.cap {
            return Err(ClaudeError::SpendCapReached(spend));
        }
        Ok(())
    }

    fn reply(&self, message: WireMessage, batched: bool) -> Reply {
        let usage = message.usage;
        let cost = usage.cost(batched);
        *self.spent.lock().unwrap_or_else(PoisonError::into_inner) += cost;
        Reply {
            answer: message.answer(),
            usage,
            cost,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.base_url)
    }

    fn authorized(&self, request: RequestBuilder) -> RequestBuilder {
        request
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", API_VERSION)
    }
}

pub struct BatchResults {
    client: AnthropicClient,
    response: reqwest::Response,
    pending: Vec<u8>,
    finished: bool,
}

impl BatchResults {
    pub async fn next(&mut self) -> Option<Result<BatchResult, ClaudeError>> {
        loop {
            if let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                if line.trim_ascii().is_empty() {
                    continue;
                }
                return Some(self.result(&line));
            }
            if self.finished {
                let line = std::mem::take(&mut self.pending);
                return (!line.trim_ascii().is_empty()).then(|| self.result(&line));
            }
            match self.response.chunk().await {
                Ok(Some(chunk)) => self.pending.extend_from_slice(&chunk),
                Ok(None) => self.finished = true,
                Err(error) => return Some(Err(SourceError::from(error).into())),
            }
        }
    }

    fn result(&self, line: &[u8]) -> Result<BatchResult, ClaudeError> {
        let line: WireResultLine = decode_json(line)?;
        let custom_id = line.custom_id;
        Ok(match line.result {
            WireResult::Succeeded { message } => BatchResult {
                custom_id,
                outcome: BatchOutcome::Answered(self.client.reply(message, true)),
            },
            WireResult::Errored { error } => wire::errored(custom_id, &error),
            WireResult::Canceled => BatchResult {
                custom_id,
                outcome: BatchOutcome::Canceled,
            },
            WireResult::Expired => BatchResult {
                custom_id,
                outcome: BatchOutcome::Expired,
            },
        })
    }
}
