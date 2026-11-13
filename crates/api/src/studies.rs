use axum::Json;
use axum::extract::State;
use buizz_core::api::ApiError;
use buizz_core::study::{EventStudy, StudyRequest};

use crate::AppState;
use crate::error::Failure;
use crate::extract::JsonBody;

pub(crate) const STUDY_BODY_LIMIT: usize = 64 * 1024 * 1024;

#[utoipa::path(post, path = "/api/studies", tag = "research",
    request_body = StudyRequest,
    responses((status = 200, body = EventStudy), (status = 400, body = ApiError)))]
pub(crate) async fn event_study(
    State(app): State<AppState>,
    JsonBody(request): JsonBody<StudyRequest>,
) -> Result<Json<EventStudy>, Failure> {
    Ok(Json(app.runner.event_study(request).await?))
}
