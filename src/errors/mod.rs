use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Resource not found")]
    NotFound,

    #[error("Validation failed: {0}")]
    Validation(String),

    #[error("Database error")]
    Database(#[from] sqlx::Error),

    #[error("Internal server error")]
    Internal(#[from] anyhow::Error),

    #[error("Bad request: {0}")]
    BadRequest(String),
}

// tiberius errors → Internal (ไม่ส่ง SQL detail ออก client)
impl From<tiberius::error::Error> for AppError {
    fn from(e: tiberius::error::Error) -> Self {
        tracing::error!("MSSQL error: {:?}", e);
        AppError::Internal(anyhow::anyhow!("Database error"))
    }
}

impl From<bb8::RunError<bb8_tiberius::Error>> for AppError {
    fn from(e: bb8::RunError<bb8_tiberius::Error>) -> Self {
        tracing::error!("MSSQL pool error: {:?}", e);
        AppError::Internal(anyhow::anyhow!("Database connection error"))
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    data: Option<()>,
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                "The requested resource was not found".to_string(),
            ),
            AppError::Validation(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_ERROR",
                msg.clone(),
            ),
            AppError::Database(e) => {
                tracing::error!("Database error: {:?}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "DATABASE_ERROR", "A database error occurred".to_string())
            }
            AppError::Internal(e) => {
                tracing::error!("Internal error: {:?}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", "An internal error occurred".to_string())
            }
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "BAD_REQUEST", msg.clone()),
        };

        let body = ErrorResponse {
            data: None,
            error: ErrorDetail { code: code.to_string(), message },
        };
        (status, Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
