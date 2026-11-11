use std::fmt::Display;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use buizz_core::api::ApiError;
use buizz_core::ops::CoverageGap;
use buizz_feed::FeedError;
use buizz_ingest::IngestError;
use buizz_market::MarketError;
use buizz_runner::RunnerError;
use buizz_state::StateError;

const LISTED_GAPS: usize = 5;

#[derive(Debug)]
pub(crate) struct Failure {
    status: StatusCode,
    message: String,
}

impl Failure {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub(crate) fn not_found(what: impl Display) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("{what} does not exist"))
    }

    fn internal(error: &dyn Display) -> Self {
        tracing::error!(%error, "request failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let body = ApiError {
            error: self.message,
        };
        (self.status, Json(body)).into_response()
    }
}

macro_rules! internal_from {
    ($($error:ty),*) => {
        $(impl From<$error> for Failure {
            fn from(error: $error) -> Self {
                Self::internal(&error)
            }
        })*
    };
}

internal_from!(StateError, MarketError, IngestError, std::io::Error);

fn describe_gaps(gaps: &[CoverageGap]) -> String {
    let listed: Vec<String> = gaps
        .iter()
        .take(LISTED_GAPS)
        .map(|gap| {
            format!(
                "{} {} {}..{}",
                gap.symbol,
                gap.timeframe.as_str(),
                gap.from,
                gap.to
            )
        })
        .collect();
    let listed = match gaps.len().saturating_sub(LISTED_GAPS) {
        0 => listed.join(", "),
        more => format!("{} and {more} more", listed.join(", ")),
    };
    match gaps.iter().map(|gap| gap.to).max() {
        Some(last) => format!("{listed}; a period starting after {last} has none"),
        None => listed,
    }
}

impl From<FeedError> for Failure {
    fn from(error: FeedError) -> Self {
        match &error {
            FeedError::CapExceeded { holders, .. } => {
                let holders: Vec<String> = holders
                    .iter()
                    .map(|holder| {
                        format!(
                            "{} (run {}, {} symbols)",
                            holder.bot,
                            holder.run_id,
                            holder.symbols.len()
                        )
                    })
                    .collect();
                Self::new(
                    StatusCode::CONFLICT,
                    format!("{error}; held by {}", holders.join(", ")),
                )
            }
            FeedError::Unavailable => Self::new(StatusCode::SERVICE_UNAVAILABLE, error.to_string()),
            FeedError::Source(_) | FeedError::Market(_) => Self::internal(&error),
        }
    }
}

impl From<RunnerError> for Failure {
    fn from(error: RunnerError) -> Self {
        match error {
            RunnerError::UnknownBot(_) | RunnerError::UnknownRun(_) => {
                Self::new(StatusCode::NOT_FOUND, error.to_string())
            }
            RunnerError::InvalidRequest(_) => Self::new(StatusCode::BAD_REQUEST, error.to_string()),
            RunnerError::HoldoutLocked | RunnerError::AlreadyLive(_) | RunnerError::NotLive(_) => {
                Self::new(StatusCode::CONFLICT, error.to_string())
            }
            RunnerError::CoverageGaps(gaps) => Self::new(
                StatusCode::CONFLICT,
                format!("market data has gaps: {}", describe_gaps(&gaps)),
            ),
            RunnerError::Feed(error) => error.into(),
            RunnerError::Ingest(error) => error.into(),
            RunnerError::Market(error) => error.into(),
            RunnerError::State(error) => error.into(),
            RunnerError::Supervisor(error) => Self::internal(&error),
        }
    }
}
