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

impl ApiError {
    /// The status and JSON body the API answers with (internal errors logged with a reference).
    pub fn into_parts(self) -> (StatusCode, Value) {
        match self {
            ApiError::Status(code, detail) => (code, json!({"detail": detail})),
            ApiError::Body(code, body) => (code, body),
            ApiError::Internal(e) => {
                let reference = crate::store::random_token(4);
                tracing::error!(reference, "internal error: {e:#}");
                let detail = format!("internal error (ref {reference}); a system owner can look it up in the logs");
                (StatusCode::INTERNAL_SERVER_ERROR, json!({"detail": detail}))
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, body) = self.into_parts();
        (code, Json(body)).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> ApiError {
        if unreachable(&e) {
            tracing::warn!("database unreachable: {e:#}");
            return ApiError::Status(
                StatusCode::SERVICE_UNAVAILABLE,
                "the database is unreachable right now; try again shortly".into(),
            );
        }
        ApiError::Internal(e)
    }
}

/// A database that can't be reached (as opposed to a query it refused): a 503, not a bug.
fn unreachable(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        if let Some(p) = c.downcast_ref::<deadpool_postgres::PoolError>() {
            return !matches!(p, deadpool_postgres::PoolError::Backend(pg) if pg.as_db_error().is_some());
        }
        c.downcast_ref::<tokio_postgres::Error>()
            .is_some_and(|pg| pg.is_closed() || pg.as_db_error().is_none() && pg.to_string().contains("connect"))
    })
}

impl From<deadpool_postgres::PoolError> for ApiError {
    fn from(e: deadpool_postgres::PoolError) -> ApiError {
        anyhow::Error::from(e).into()
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

impl From<crate::orgdb::OrgDbError> for ApiError {
    fn from(e: crate::orgdb::OrgDbError) -> ApiError {
        use crate::orgdb::OrgDbError;
        match e {
            OrgDbError::NotConfigured(_) | OrgDbError::Unavailable(_) => {
                tracing::warn!("{e}");
                ApiError::Status(StatusCode::SERVICE_UNAVAILABLE, e.to_string())
            }
            OrgDbError::Internal(e) => ApiError::Internal(e),
        }
    }
}

impl From<crate::connectors::ConnError> for ApiError {
    fn from(e: crate::connectors::ConnError) -> ApiError {
        use crate::connectors::ConnError;
        match e {
            ConnError::Missing(_) | ConnError::Invalid(_) => ApiError::bad_request(e.to_string()),
            ConnError::Internal(e) => e.into(),
        }
    }
}
