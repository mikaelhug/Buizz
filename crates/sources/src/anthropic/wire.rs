use serde::Deserialize;
use serde_json::Value;

use super::{Answer, BatchOutcome, BatchResult, Usage};

const REFUSAL: &str = "refusal";
const MAX_TOKENS: &str = "max_tokens";
const RETRYABLE_ERRORS: [&str; 3] = ["api_error", "overloaded_error", "rate_limit_error"];

#[derive(Debug, Deserialize)]
pub(super) struct WireMessage {
    content: Vec<WireBlock>,
    stop_reason: Option<String>,
    stop_details: Option<WireStopDetails>,
    pub(super) usage: Usage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireBlock {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct WireStopDetails {
    category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WireResultLine {
    pub(super) custom_id: String,
    pub(super) result: WireResult,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum WireResult {
    Succeeded { message: WireMessage },
    Errored { error: Value },
    Canceled,
    Expired,
}

impl WireMessage {
    pub(super) fn answer(self) -> Answer {
        match self.stop_reason.as_deref() {
            Some(REFUSAL) => Answer::Refused {
                category: self.stop_details.and_then(|details| details.category),
            },
            Some(MAX_TOKENS) => Answer::Truncated,
            _ => {
                let text: String = self
                    .content
                    .into_iter()
                    .filter_map(|block| match block {
                        WireBlock::Text { text } => Some(text),
                        WireBlock::Other => None,
                    })
                    .collect();
                serde_json::from_str(&text)
                    .map_or_else(|_| Answer::Malformed { text }, Answer::Json)
            }
        }
    }
}

pub(super) fn errored(custom_id: String, error: &Value) -> BatchResult {
    let kind = error
        .pointer("/error/type")
        .or_else(|| error.pointer("/type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    BatchResult {
        custom_id,
        outcome: BatchOutcome::Errored {
            retryable: RETRYABLE_ERRORS.contains(&kind.as_str()),
            kind,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn answer(message: Value) -> Answer {
        serde_json::from_value::<WireMessage>(message)
            .unwrap()
            .answer()
    }

    #[test]
    fn a_refusal_or_a_cut_off_answer_is_not_read_as_json() {
        let usage = json!({"input_tokens": 900, "output_tokens": 0});
        let refused = answer(json!({
            "content": [],
            "stop_reason": "refusal",
            "stop_details": {"type": "refusal", "category": "cyber", "explanation": null},
            "usage": usage,
        }));
        assert_eq!(
            refused,
            Answer::Refused {
                category: Some("cyber".to_owned())
            }
        );
        let truncated = answer(json!({
            "content": [{"type": "text", "text": "{\"officers\": [{\"name\": \"Ja"}],
            "stop_reason": "max_tokens",
            "stop_details": null,
            "usage": usage,
        }));
        assert_eq!(truncated, Answer::Truncated);
        let answered = answer(json!({
            "content": [
                {"type": "thinking", "thinking": "", "signature": "sig"},
                {"type": "text", "text": "{\"officers\": []}"},
            ],
            "stop_reason": "end_turn",
            "stop_details": null,
            "usage": usage,
        }));
        assert_eq!(answered, Answer::Json(json!({"officers": []})));
    }

    #[test]
    fn an_errored_batch_request_says_whether_it_can_be_sent_again() {
        let line = |kind: &str| {
            let line: WireResultLine = serde_json::from_value(json!({
                "custom_id": "0000950170-24-001234-5.02",
                "result": {"type": "errored", "error": {
                    "type": "error",
                    "error": {"type": kind, "message": "..."},
                }},
            }))
            .unwrap();
            let WireResult::Errored { error } = line.result else {
                panic!("an errored result");
            };
            errored(line.custom_id, &error).outcome
        };
        assert_eq!(
            line("invalid_request_error"),
            BatchOutcome::Errored {
                kind: "invalid_request_error".to_owned(),
                retryable: false,
            }
        );
        assert_eq!(
            line("overloaded_error"),
            BatchOutcome::Errored {
                kind: "overloaded_error".to_owned(),
                retryable: true,
            }
        );
    }
}
