//! Evidence-owned content storage. Authority tables and identities are never imported.
pub mod authority;
pub mod backup;
pub mod files;
pub mod http;
pub mod storage;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

/// Errors deliberately omit inputs, database messages, paths and credentials.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid evidence input")]
    Invalid,
    #[error("evidence request too large")]
    TooLarge,
    #[error("evidence not found")]
    NotFound,
    #[error("evidence conflict")]
    Conflict,
    #[error("evidence storage unavailable")]
    Unavailable,
    #[error("verified author identity required")]
    AuthorRequired,
    #[error("authority denied")]
    Authority(StatusCode),
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Unavailable
    }
}
impl From<sea_orm::DbErr> for Error {
    fn from(_: sea_orm::DbErr) -> Self {
        Self::Unavailable
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            Self::Invalid => (StatusCode::BAD_REQUEST, "invalid_evidence"),
            Self::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "evidence_too_large"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Conflict => (StatusCode::CONFLICT, "revision_conflict"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"),
            Self::AuthorRequired => (StatusCode::FORBIDDEN, "author_identity_required"),
            Self::Authority(status) => (status, "authority_denied"),
        };
        (
            status,
            [("cache-control", "private, no-store")],
            Json(json!({"error":code})),
        )
            .into_response()
    }
}
pub type Result<T> = std::result::Result<T, Error>;

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .checked_div(1_000_000)
        .and_then(|n| i64::try_from(n).ok())
        .unwrap_or(i64::MAX)
}
pub fn hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}
