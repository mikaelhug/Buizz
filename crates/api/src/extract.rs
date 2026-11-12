use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{FromRequest, FromRequestParts};
use axum_extra::extract::QueryRejection;

use crate::error::Failure;

#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(Failure))]
pub(crate) struct JsonBody<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(Failure))]
pub(crate) struct Path<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum_extra::extract::Query), rejection(Failure))]
pub(crate) struct Query<T>(pub T);

macro_rules! rejected_as_failure {
    ($($rejection:ty),*) => {
        $(impl From<$rejection> for Failure {
            fn from(rejection: $rejection) -> Self {
                Self::new(rejection.status(), rejection.body_text())
            }
        })*
    };
}

rejected_as_failure!(JsonRejection, PathRejection, QueryRejection);
