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
    pub fn wire_name(self) -> &'static str {
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
    pub fn status(self) -> u16 {
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
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::of(ErrorType::InvalidRequest, message)
    }

    /// 404 `not_found_error`.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::of(ErrorType::NotFound, message)
    }

    /// 401 `authentication_error`.
    pub fn authentication(message: impl Into<String>) -> Self {
        Self::of(ErrorType::Authentication, message)
    }

    /// 409 with the same `error.type` as a plain invalid request (live
    /// behavior: non-typed conflicts reuse `invalid_request_error`).
    pub fn conflict_invalid_request(message: impl Into<String>) -> Self {
        let mut e = Self::of(ErrorType::InvalidRequest, message);
        e.status = 409;
        e
    }

    /// 409 `memory_path_conflict_error` — exact live shape:
    /// `path `/a.md` is already used by `mem_…`; use update to modify it`
    /// plus `conflicting_path` / `conflicting_memory_id` fields.
    pub fn memory_path_conflict(path: &str, memory_id: &str) -> Self {
        let mut e = Self::of(
            ErrorType::MemoryPathConflict,
            format!("path `{path}` is already used by `{memory_id}`; use update to modify it"),
        );
        e.extra
            .insert("conflicting_path".into(), Value::String(path.into()));
        e.extra.insert(
            "conflicting_memory_id".into(),
            Value::String(memory_id.into()),
        );
        e
    }

    /// 429 `rate_limit_error`.
    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::of(ErrorType::RateLimit, message)
    }

    /// 500 `api_error`.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::of(ErrorType::Api, message)
    }

    fn of(error_type: ErrorType, message: impl Into<String>) -> Self {
        Self {
            status: error_type.status(),
            error_type,
            message: message.into(),
            extra: Map::new(),
        }
    }

    /// Render the full wire envelope with a request id.
    pub fn to_envelope(&self, request_id: &RequestId) -> Value {
        let mut error = Map::new();
        error.insert(
            "type".into(),
            Value::String(self.error_type.wire_name().into()),
        );
        error.insert("message".into(), Value::String(self.message.clone()));
        for (k, v) in &self.extra {
            error.insert(k.clone(), v.clone());
        }
        let mut envelope = Map::new();
        envelope.insert("type".into(), Value::String("error".into()));
        envelope.insert("error".into(), Value::Object(error));
        envelope.insert(
            "request_id".into(),
            Value::String(request_id.as_str().into()),
        );
        Value::Object(envelope)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}): {}",
            self.error_type.wire_name(),
            self.status,
            self.message
        )
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
mod tests {
    use super::*;

    fn req() -> RequestId {
        RequestId::generate()
    }

    #[test]
    fn envelope_matches_live_shape() {
        let e = ApiError::invalid_request("model: Field required");
        let rid = req();
        let v = e.to_envelope(&rid);
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "invalid_request_error");
        assert_eq!(v["error"]["message"], "model: Field required");
        assert_eq!(v["request_id"], rid.as_str());
        // Exactly three top-level keys.
        assert_eq!(v.as_object().unwrap().len(), 3);
    }

    #[test]
    fn memory_path_conflict_matches_live_shape() {
        // Live probe (409):
        // {"type":"error","error":{"type":"memory_path_conflict_error",
        //  "message":"path `/a.md` is already used by `mem_019f…`; use update to modify it",
        //  "conflicting_path":"/a.md","conflicting_memory_id":"mem_019f…"}, …}
        let e = ApiError::memory_path_conflict("/a.md", "mem_019fjZBeerJmg5Smuyy2mCS5");
        assert_eq!(e.status, 409);
        let v = e.to_envelope(&req());
        assert_eq!(v["error"]["type"], "memory_path_conflict_error");
        assert_eq!(
            v["error"]["message"],
            "path `/a.md` is already used by `mem_019fjZBeerJmg5Smuyy2mCS5`; use update to modify it"
        );
        assert_eq!(v["error"]["conflicting_path"], "/a.md");
        assert_eq!(
            v["error"]["conflicting_memory_id"],
            "mem_019fjZBeerJmg5Smuyy2mCS5"
        );
    }

    #[test]
    fn status_map_is_complete() {
        assert_eq!(ErrorType::InvalidRequest.status(), 400);
        assert_eq!(ErrorType::Authentication.status(), 401);
        assert_eq!(ErrorType::Permission.status(), 403);
        assert_eq!(ErrorType::NotFound.status(), 404);
        assert_eq!(ErrorType::MemoryPathConflict.status(), 409);
        assert_eq!(ErrorType::MemoryPreconditionFailed.status(), 409);
        assert_eq!(ErrorType::RequestTooLarge.status(), 413);
        assert_eq!(ErrorType::RateLimit.status(), 429);
        assert_eq!(ErrorType::Api.status(), 500);
        assert_eq!(ErrorType::Overloaded.status(), 529);
    }

    #[test]
    fn conflict_invalid_request_is_409_with_invalid_request_type() {
        let e = ApiError::conflict_invalid_request("cannot archive while running");
        assert_eq!(e.status, 409);
        assert_eq!(e.error_type.wire_name(), "invalid_request_error");
    }
}
