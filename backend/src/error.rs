//! API errors as JSON: {"detail": "..."} plus optional hints for the UI.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::access::AccessDenied;
use crate::compiler::QueryError;

#[derive(Debug)]
pub enum ApiError {
    Status(StatusCode, String),
    Body(StatusCode, Value),
    Internal(anyhow::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> ApiError {
        ApiError::Status(StatusCode::BAD_REQUEST, msg.into())
    }
    pub fn forbidden(msg: impl Into<String>) -> ApiError {
        ApiError::Status(StatusCode::FORBIDDEN, msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> ApiError {
        ApiError::Status(StatusCode::NOT_FOUND, msg.into())
    }
    pub fn sign_in() -> ApiError {
        ApiError::Body(StatusCode::UNAUTHORIZED, json!({"detail": "Please sign in", "login": true}))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Status(code, detail) => (code, Json(json!({"detail": detail}))).into_response(),
            ApiError::Body(code, body) => (code, Json(body)).into_response(),
            ApiError::Internal(e) => {
                let reference = crate::store::random_token(4);
                tracing::error!(reference, "internal error: {e:#}");
                let detail = format!("internal error (ref {reference}); a system owner can look it up in the logs");
                (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": detail}))).into_response()
            }
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> ApiError {
        ApiError::Internal(e)
    }
}

impl From<deadpool_postgres::PoolError> for ApiError {
    fn from(e: deadpool_postgres::PoolError) -> ApiError {
        ApiError::Internal(e.into())
    }
}

impl From<QueryError> for ApiError {
    fn from(e: QueryError) -> ApiError {
        ApiError::bad_request(e.0)
    }
}

impl From<AccessDenied> for ApiError {
    fn from(e: AccessDenied) -> ApiError {
        ApiError::forbidden(e.0)
    }
}
