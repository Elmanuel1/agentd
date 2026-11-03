//! The HTTP boundary: one span and one duration sample per request.

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::middleware::Next;
use axum::response::Response;
use futures::StreamExt;
use opentelemetry::propagation::Extractor;
use opentelemetry::KeyValue;
use shared::{ErrorCause, RequestId};
use tracing::field::Empty;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use super::Metrics;

/// The route label of a request no route matched, so unknown paths share one
/// series instead of one per path.
const UNMATCHED: &str = "unmatched";

/// Trace, time and count the request. A 5xx is logged at `error`, the
/// response status code is on the span and the duration sample.
pub async fn observe(State(metrics): State<Arc<Metrics>>, req: Request, next: Next) -> Response {
    let method = req.method().as_str().to_owned();
    let route = req.extensions().get::<MatchedPath>().map_or(UNMATCHED, MatchedPath::as_str).to_owned();
    let request_id = req.extensions().get::<RequestId>().map(|id| id.as_str().to_owned()).unwrap_or_default();
    let span = tracing::info_span!(
        "http.request",
        otel.name = %format!("{method} {route}"),
        otel.kind = "server",
        otel.status_code = Empty,
        http.request.method = %method,
        http.route = %route,
        http.response.status_code = Empty,
        request_id = %request_id,
    );
    let parent = opentelemetry::global::get_text_map_propagator(|p| p.extract(&Headers(req.headers())));
    let _ = span.set_parent(parent);
    let attrs =
        [KeyValue::new("http.request.method", method.clone()), KeyValue::new("http.route", route.clone())];
    metrics.http_active.add(1, &attrs);
    let started = Instant::now();
    let response = next.run(req).instrument(span.clone()).await;
    metrics.http_active.add(-1, &attrs);
    let status = response.status();
    span.record("http.response.status_code", status.as_u16());
    if status.is_server_error() {
        span.record("otel.status_code", "ERROR");
        let cause = response.extensions().get::<ErrorCause>().map_or("", |c| c.0.as_str());
        span.in_scope(|| {
            tracing::error!(%method, %route, status = status.as_u16(), %request_id, error = cause, "request failed");
        });
    }
    let mut labels = attrs.to_vec();
    labels.push(KeyValue::new("http.response.status_code", i64::from(status.as_u16())));
    metrics.http_duration.record(started.elapsed().as_secs_f64(), &labels);
    stream_guard(response, &metrics, attrs.to_vec())
}

/// An event stream counts as open until its body is dropped, which is when
/// the client disconnects or the stream ends.
fn stream_guard(response: Response, metrics: &Metrics, attrs: Vec<KeyValue>) -> Response {
    let is_stream = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    if !is_stream {
        return response;
    }
    let guard = Open::new(metrics.sse_active.clone(), attrs);
    let (parts, body) = response.into_parts();
    let body = body.into_data_stream().map(move |chunk| {
        let _open = &guard;
        chunk
    });
    Response::from_parts(parts, Body::from_stream(body))
}

struct Open {
    counter: opentelemetry::metrics::UpDownCounter<i64>,
    attrs: Vec<KeyValue>,
}

impl Open {
    fn new(counter: opentelemetry::metrics::UpDownCounter<i64>, attrs: Vec<KeyValue>) -> Self {
        counter.add(1, &attrs);
        Self { counter, attrs }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.counter.add(-1, &self.attrs);
    }
}

struct Headers<'a>(&'a axum::http::HeaderMap);

impl Extractor for Headers<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(axum::http::HeaderName::as_str).collect()
    }
}
