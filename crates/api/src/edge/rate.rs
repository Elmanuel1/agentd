//! Live's rate limits (L2, DEP-1 to DEP-5): each organization's requests are
//! counted per bucket in a fixed 60 s window; past a bucket's limit the
//! answer is 429 `rate_limit_error` with `retry-after`, the seconds left. A
//! limited route outside files carries `anthropic-ratelimit-requests-limit`,
//! `-remaining` and `-reset`. With the counter slow or down the request is
//! admitted, counted as unchecked, and logged once a minute.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;
use shared::ports::rate::{Counted, RateCounter};
use shared::{ApiError, AuthContext, Timestamp, wire_response};

/// One bucket: its name, its limit a minute, and whether it sends the headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bucket {
    name: &'static str,
    limit: u64,
    headers: bool,
}

const CREATE: Bucket = Bucket { name: "create", limit: 300, headers: true };
const READ: Bucket = Bucket { name: "read", limit: 1_200, headers: true };
const MEMORY_STORES: Bucket = Bucket { name: "memory_stores", limit: 30_000, headers: true };
const FILES: Bucket = Bucket { name: "files", limit: 500, headers: false };

/// Read routes live counts in one bucket.
const READ_UNDER: [&str; 5] =
    ["/v1/agents", "/v1/sessions", "/v1/vaults", "/v1/deployments", "/v1/deployment_runs"];

/// The bucket `method` on the route template `path` counts in, if any.
pub fn bucket(method: &Method, path: &str) -> Option<Bucket> {
    let under =
        |root: &str| path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'));
    if under("/v1/files") {
        return Some(FILES);
    }
    match *method {
        Method::POST if matches!(path, "/v1/agents" | "/v1/sessions" | "/v1/environments") => Some(CREATE),
        Method::GET if path == "/v1/memory_stores" => Some(MEMORY_STORES),
        Method::GET if READ_UNDER.iter().any(|root| under(root)) => Some(READ),
        _ => None,
    }
}

/// The counter, and when a refused check was last logged.
#[derive(Clone)]
pub struct Limits {
    counter: Arc<dyn RateCounter>,
    warned_at: Arc<AtomicI64>,
}

impl std::fmt::Debug for Limits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Limits").finish_non_exhaustive()
    }
}

impl Limits {
    pub fn new(counter: Arc<dyn RateCounter>) -> Self {
        Self { counter, warned_at: Arc::new(AtomicI64::new(i64::MIN)) }
    }

    fn unchecked(&self, error: &shared::BackendError) {
        crate::telemetry::edge::rate_unchecked();
        let now = Timestamp::now().as_datetime().timestamp();
        let last = self.warned_at.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= 60
            && self.warned_at.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            tracing::warn!(error = %error, "edge: the rate counter did not answer; requests are admitted unchecked");
        }
    }
}

/// Count the request in its bucket, refusing it past the bucket's limit.
pub async fn limit(State(limits): State<Limits>, req: Request, next: Next) -> Response {
    let path = req.extensions().get::<MatchedPath>().map(|p| p.as_str().to_owned()).unwrap_or_default();
    let (Some(bucket), Some(org)) = (
        bucket(req.method(), &path),
        req.extensions().get::<AuthContext>().map(|c| c.org.as_str().to_owned()),
    ) else {
        return next.run(req).await;
    };
    let counted = match limits.counter.hit(&org, bucket.name).await {
        Ok(counted) => counted,
        Err(e) => {
            limits.unchecked(&e);
            return next.run(req).await;
        }
    };
    // The reset is when the window counted here ends, however long the handler takes.
    let reset =
        Timestamp::now().as_datetime() + chrono::TimeDelta::from_std(counted.resets_in).unwrap_or_default();
    let mut response = match counted.count > bucket.limit {
        true => refused(counted),
        false => next.run(req).await,
    };
    if bucket.headers {
        add_headers(&mut response, bucket, counted, reset);
    }
    response
}

fn refused(counted: Counted) -> Response {
    crate::telemetry::edge::rejected("rate_limited");
    let mut response =
        wire_response(&ApiError::rate_limited("Number of requests has exceeded your rate limit"));
    let seconds = counted.resets_in.as_secs() + u64::from(counted.resets_in.subsec_nanos() > 0);
    if let Ok(v) = HeaderValue::from_str(&seconds.max(1).to_string()) {
        response.headers_mut().insert("retry-after", v);
    }
    response
}

fn add_headers(
    response: &mut Response,
    bucket: Bucket,
    counted: Counted,
    reset: chrono::DateTime<chrono::Utc>,
) {
    let values = [
        ("anthropic-ratelimit-requests-limit", bucket.limit.to_string()),
        ("anthropic-ratelimit-requests-remaining", bucket.limit.saturating_sub(counted.count).to_string()),
        ("anthropic-ratelimit-requests-reset", reset.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
    ];
    for (name, value) in values {
        if let Ok(v) = HeaderValue::from_str(&value) {
            response.headers_mut().insert(name, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_route_template_counts_in_live_s_bucket() {
        let b = |m: Method, p: &str| bucket(&m, p).map(|b| b.name);
        assert_eq!(b(Method::POST, "/v1/agents"), Some("create"));
        assert_eq!(b(Method::POST, "/v1/sessions"), Some("create"));
        assert_eq!(b(Method::POST, "/v1/environments"), Some("create"));
        assert_eq!(b(Method::GET, "/v1/agents/{agent_id}"), Some("read"));
        assert_eq!(b(Method::GET, "/v1/sessions/{session_id}/events"), Some("read"));
        assert_eq!(b(Method::GET, "/v1/deployment_runs"), Some("read"));
        assert_eq!(b(Method::GET, "/v1/memory_stores"), Some("memory_stores"));
        assert_eq!(b(Method::DELETE, "/v1/files/{file_id}"), Some("files"));
        assert_eq!(b(Method::POST, "/v1/sessions/{session_id}/events"), None, "events send is not limited");
        assert_eq!(b(Method::GET, "/v1/environments"), None);
        assert_eq!(b(Method::GET, "/v1/skills"), None);
        assert_eq!(b(Method::GET, "/v1/agentsx"), None, "a template, not a prefix");
    }
}
