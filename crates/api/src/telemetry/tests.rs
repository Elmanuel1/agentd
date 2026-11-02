use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::sse::{Event, Sse};
use axum::routing::get;
use axum::Router;
use bytes::Bytes;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{KeyValue, Value};
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use shared::ports::blobs::{BlobError, BlobStore};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;

use super::ports::Observed;
use super::Metrics;

struct Harness {
    metrics: Arc<Metrics>,
    meters: SdkMeterProvider,
    metric_out: InMemoryMetricExporter,
    spans: SdkTracerProvider,
    span_out: InMemorySpanExporter,
    _subscriber: tracing::subscriber::DefaultGuard,
}

fn harness() -> Harness {
    let metric_out = InMemoryMetricExporter::default();
    let meters =
        SdkMeterProvider::builder().with_reader(PeriodicReader::builder(metric_out.clone()).build()).build();
    let span_out = InMemorySpanExporter::default();
    let spans = SdkTracerProvider::builder().with_simple_exporter(span_out.clone()).build();
    let subscriber =
        tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(spans.tracer("test")));
    Harness {
        metrics: Arc::new(Metrics::new(&meters.meter("test"))),
        meters,
        metric_out,
        spans,
        span_out,
        _subscriber: tracing::subscriber::set_default(subscriber),
    }
}

impl Harness {
    /// Every histogram sample's attributes for `name`, with its count.
    fn histogram(&self, name: &str) -> Vec<(Vec<KeyValue>, u64)> {
        self.meters.force_flush().unwrap();
        let mut out = Vec::new();
        for rm in self.metric_out.get_finished_metrics().unwrap() {
            for m in rm.scope_metrics().flat_map(|s| s.metrics()).filter(|m| m.name() == name) {
                if let AggregatedMetrics::F64(MetricData::Histogram(h)) = m.data() {
                    out.extend(h.data_points().map(|p| (p.attributes().cloned().collect(), p.count())));
                }
            }
        }
        out
    }

    fn histogram_bounds(&self, name: &str) -> Vec<f64> {
        self.meters.force_flush().unwrap();
        let metrics = self.metric_out.get_finished_metrics().unwrap();
        let bounds =
            metrics.iter().flat_map(|rm| rm.scope_metrics()).flat_map(|s| s.metrics()).find_map(|m| {
                match (m.name() == name, m.data()) {
                    (true, AggregatedMetrics::F64(MetricData::Histogram(h))) => {
                        h.data_points().next().map(|p| p.bounds().collect::<Vec<_>>())
                    }
                    _ => None,
                }
            });
        bounds.unwrap_or_default()
    }

    fn span_names(&self) -> Vec<String> {
        self.spans.force_flush().unwrap();
        self.span_out.get_finished_spans().unwrap().into_iter().map(|s| s.name.into_owned()).collect()
    }
}

fn attr<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a Value> {
    attrs.iter().find(|kv| kv.key.as_str() == key).map(|kv| &kv.value)
}

fn app(metrics: Arc<Metrics>) -> Router {
    Router::new()
        .route("/things/{id}", get(|| async { "ok" }))
        .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
        .route(
            "/stream",
            get(|| async {
                Sse::new(futures::stream::iter([Ok::<_, std::convert::Infallible>(
                    Event::default().data("x"),
                )]))
            }),
        )
        .layer(from_fn_with_state(metrics, super::http::observe))
}

async fn send(app: &Router, uri: &str) -> StatusCode {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let _ = axum::body::to_bytes(res.into_body(), usize::MAX).await;
    status
}

#[tokio::test(flavor = "current_thread")]
async fn every_request_is_a_span_and_a_sample_by_route_and_status() {
    let h = harness();
    let app = app(h.metrics.clone());
    assert_eq!(send(&app, "/things/1").await, StatusCode::OK);
    assert_eq!(send(&app, "/things/2").await, StatusCode::OK);
    assert_eq!(send(&app, "/boom").await, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(send(&app, "/nowhere/7").await, StatusCode::NOT_FOUND);
    let samples = h.histogram("http.server.request.duration");
    let count = |route: &str, status: i64| {
        samples
            .iter()
            .filter(|(a, _)| {
                attr(a, "http.route") == Some(&Value::from(route.to_owned()))
                    && attr(a, "http.response.status_code") == Some(&Value::I64(status))
            })
            .map(|(_, n)| n)
            .sum::<u64>()
    };
    assert_eq!(count("/things/{id}", 200), 2, "one series per route template, not per path");
    assert_eq!(count("/boom", 500), 1);
    assert_eq!(count("unmatched", 404), 1);
    let names = h.span_names();
    assert!(names.contains(&"GET /things/{id}".to_owned()), "{names:?}");
    let fast_bucket = h.histogram_bounds("http.server.request.duration");
    assert_eq!(fast_bucket.first(), Some(&0.005), "seconds buckets, not the millisecond defaults");
    assert!(names.contains(&"GET /boom".to_owned()));
}

#[tokio::test(flavor = "current_thread")]
async fn an_event_stream_is_open_until_its_body_is_dropped() {
    let h = harness();
    let app = app(h.metrics.clone());
    let res = app.clone().oneshot(Request::get("/stream").body(Body::empty()).unwrap()).await.unwrap();
    let open = |h: &Harness| {
        h.meters.force_flush().unwrap();
        h.metric_out
            .get_finished_metrics()
            .unwrap()
            .last()
            .into_iter()
            .flat_map(|rm| rm.scope_metrics().flat_map(|s| s.metrics()).collect::<Vec<_>>())
            .filter(|m| m.name() == "agentd.sse.active_streams")
            .find_map(|m| match m.data() {
                AggregatedMetrics::I64(MetricData::Sum(s)) => {
                    Some(s.data_points().map(|p| p.value()).sum::<i64>())
                }
                _ => None,
            })
            .unwrap_or_default()
    };
    assert_eq!(open(&h), 1);
    drop(res);
    assert_eq!(open(&h), 0);
}

struct Failing;

#[async_trait]
impl BlobStore for Failing {
    async fn put(&self, key: &str, _bytes: Bytes) -> Result<(), BlobError> {
        Err(BlobError::NotFound(key.to_owned()))
    }
    async fn get(&self, key: &str) -> Result<Bytes, BlobError> {
        Err(BlobError::NotFound(key.to_owned()))
    }
    async fn delete(&self, _key: &str) -> Result<(), BlobError> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_port_operation_is_a_span_and_a_sample_with_its_outcome() {
    let h = harness();
    let blobs = Observed::new(Arc::new(Failing), h.metrics.clone());
    assert!(blobs.put("k", Bytes::new()).await.is_err());
    blobs.delete("k").await.unwrap();
    let samples = h.histogram("agentd.port.operation.duration");
    let outcome_of = |op: &str| {
        samples
            .iter()
            .find(|(a, _)| attr(a, "agentd.operation") == Some(&Value::from(op.to_owned())))
            .and_then(|(a, _)| attr(a, "outcome").cloned())
    };
    assert_eq!(outcome_of("put"), Some(Value::from("error")));
    assert_eq!(outcome_of("delete"), Some(Value::from("ok")));
    assert!(h.span_names().contains(&"blobs.put".to_owned()));
}
