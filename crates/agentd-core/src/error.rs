//! The wire error envelope.
//!
//! Live shape (all captured from the real API):
//! ```json
//! {"type":"error",
//!  "error":{"type":"invalid_request_error","message":"model: Field required"},
//!  "request_id":"req_011CcfvHvqyhVMVsfTNRo4SG"}
//! ```
//! Typed conflict errors carry extra sibling fields inside `error`, e.g.
//! `memory_path_conflict_error` adds `conflicting_path` and
//! `conflicting_memory_id`.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::id::RequestId;

mod backend;

pub use backend::BackendError;

/// Every wire `error.type` the live API emits, with its canonical HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    /// 400 — also used for 409 conflicts other than typed ones, and for
    /// malformed IDs (live probe: bad agent id → 400, **not** 404).
    InvalidRequest,
    /// 401
    Authentication,
    /// 403
    Permission,
    /// 404
    NotFound,
    /// 409 — memory create at an occupied path.
    MemoryPathConflict,
    /// 409 — memory update precondition failed.
    MemoryPreconditionFailed,
    /// 413
    RequestTooLarge,
    /// 429
    RateLimit,
    /// 500
    Api,
    /// 529
    Overloaded,
}

impl ErrorType {
    /// The wire string for `error.type`.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request_error",
            Self::Authentication => "authentication_error",
            Self::Permission => "permission_error",
            Self::NotFound => "not_found_error",
            Self::MemoryPathConflict => "memory_path_conflict_error",
            Self::MemoryPreconditionFailed => "memory_precondition_failed_error",
            Self::RequestTooLarge => "request_too_large",
            Self::RateLimit => "rate_limit_error",
            Self::Api => "api_error",
            Self::Overloaded => "overloaded_error",
        }
    }

    /// Canonical HTTP status code.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::InvalidRequest => 400,
            Self::Authentication => 401,
            Self::Permission => 403,
            Self::NotFound => 404,
            Self::MemoryPathConflict | Self::MemoryPreconditionFailed => 409,
            Self::RequestTooLarge => 413,
            Self::RateLimit => 429,
            Self::Api => 500,
            Self::Overloaded => 529,
        }
    }
}

/// A fully-formed API error, ready to render as the wire envelope.
#[derive(Debug, Clone)]
pub struct ApiError {
    /// HTTP status. Usually `error_type.status()`, overridable for the cases
    /// where the same `error.type` ships under a different status (e.g. 409
    /// business conflicts using `invalid_request_error`).
    pub status: u16,
    /// The `error.type` discriminant.
    pub error_type: ErrorType,
    /// The `error.message` — exact wire strings, see [`crate::limits`].
    pub message: String,
    /// Extra sibling fields inside `error` (typed conflicts).
    pub extra: Map<String, Value>,
}

impl ApiError {
    /// 400 `invalid_request_error`.
    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::of(ErrorType::InvalidRequest, message)
    }

    /// 404 `not_found_error`.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::of(ErrorType::NotFound, message)
    }

    /// 401 `authentication_error`.
    #[must_use]
    pub fn authentication(message: impl Into<String>) -> Self {
        Self::of(ErrorType::Authentication, message)
    }

    /// 409 with the same `error.type` as a plain invalid request (live
    /// behavior: non-typed conflicts reuse `invalid_request_error`).
    #[must_use]
    pub fn conflict_invalid_request(message: impl Into<String>) -> Self {
        let mut e = Self::of(ErrorType::InvalidRequest, message);
        e.status = 409;
        e
    }

    /// 409 `memory_path_conflict_error` — exact live shape:
    /// `path `/a.md` is already used by `mem_…`; use update to modify it`
    /// plus `conflicting_path` / `conflicting_memory_id` fields.
    #[must_use]
    pub fn memory_path_conflict(path: &str, memory_id: &str) -> Self {
        let mut e = Self::of(
            ErrorType::MemoryPathConflict,
            format!("path `{path}` is already used by `{memory_id}`; use update to modify it"),
        );
        e.extra.insert("conflicting_path".into(), Value::String(path.into()));
        e.extra.insert("conflicting_memory_id".into(), Value::String(memory_id.into()));
        e
    }

    /// 429 `rate_limit_error`.
    #[must_use]
    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::of(ErrorType::RateLimit, message)
    }

    /// 500 `api_error`.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::of(ErrorType::Api, message)
    }

    fn of(error_type: ErrorType, message: impl Into<String>) -> Self {
        Self { status: error_type.status(), error_type, message: message.into(), extra: Map::new() }
    }

    /// Render the full wire envelope with a request id.
    pub fn to_envelope(&self, request_id: &RequestId) -> Value {
        let mut error = Map::new();
        error.insert("type".into(), Value::String(self.error_type.wire_name().into()));
        error.insert("message".into(), Value::String(self.message.clone()));
        for (k, v) in &self.extra {
            error.insert(k.clone(), v.clone());
        }
        let mut envelope = Map::new();
        envelope.insert("type".into(), Value::String("error".into()));
        envelope.insert("error".into(), Value::Object(error));
        envelope.insert("request_id".into(), Value::String(request_id.as_str().into()));
        Value::Object(envelope)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.error_type.wire_name(), self.status, self.message)
    }
}

impl std::error::Error for ApiError {}

/// Envelope wrapper used when a handler needs a serializable success body —
/// kept here so all crates serialize identically.
#[derive(Debug, Serialize)]
pub struct DataEnvelope<T> {
    /// The `data` array.
    pub data: Vec<T>,
}

#[cfg(test)]
mod tests;
