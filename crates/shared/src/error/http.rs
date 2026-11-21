//! Each wire `error.type` and the HTTP status it ships under.

/// Every wire `error.type` the live API emits, with its canonical HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    /// 400, also used for 409 conflicts other than typed ones, and for
    /// malformed IDs (live probe: bad agent id → 400, **not** 404).
    InvalidRequest,
    /// 401
    Authentication,
    /// 403
    Permission,
    /// 404
    NotFound,
    /// 409: memory create at an occupied path.
    MemoryPathConflict,
    /// 409: memory update precondition failed.
    MemoryPreconditionFailed,
    /// 413
    RequestTooLarge,
    /// 429
    RateLimit,
    /// 500
    Api,
    /// 504: the response did not start before the server's time limit.
    Timeout,
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
            Self::Timeout => "timeout_error",
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
            Self::Timeout => 504,
            Self::Overloaded => 529,
        }
    }
}
