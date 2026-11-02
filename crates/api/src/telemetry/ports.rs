//! The outbound ports and scheduled fires: every blob, secret and sandbox
//! operation and every fire is a span and a duration sample, and a failure is
//! logged with the port and operation.

use std::fmt::Display;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use opentelemetry::KeyValue;
use shared::ports::blobs::{BlobError, BlobStore};
use shared::ports::fire::{FireError, FireHandler};
use shared::ports::keys::{KeyError, KeySource};
use shared::ports::secrets::{SecretError, SecretKey, SecretStore, SecretString};
use shared::ports::{JobSpec, Runtime, RuntimeError, RuntimeHandle, TaskState};
use shared::AuthContext;
use tracing::field::Empty;
use tracing::Instrument;

use super::{outcome, Metrics};

/// A port wrapped in its observation.
#[derive(Debug, Clone)]
pub struct Observed<T: ?Sized> {
    inner: Arc<T>,
    metrics: Arc<Metrics>,
}

impl<T: ?Sized> Observed<T> {
    /// Observe `inner` into `metrics`.
    pub fn new(inner: Arc<T>, metrics: Arc<Metrics>) -> Self {
        Self { inner, metrics }
    }

    async fn run<R, E: Display>(
        &self,
        port: &'static str,
        op: &'static str,
        call: impl Future<Output = Result<R, E>>,
    ) -> Result<R, E> {
        let span = tracing::info_span!(
            "port.operation",
            otel.name = %format!("{port}.{op}"),
            otel.kind = "client",
            otel.status_code = Empty,
            agentd.port = port,
            agentd.operation = op,
        );
        let started = Instant::now();
        let result = call.instrument(span.clone()).await;
        if let Err(e) = &result {
            span.record("otel.status_code", "ERROR");
            span.in_scope(|| tracing::warn!(agentd.port = port, agentd.operation = op, error = %e, "port operation failed"));
        }
        let attrs = [
            KeyValue::new("agentd.port", port),
            KeyValue::new("agentd.operation", op),
            KeyValue::new("outcome", outcome(&result)),
        ];
        self.metrics.port_duration.record(started.elapsed().as_secs_f64(), &attrs);
        result
    }
}

#[async_trait]
impl<T: BlobStore + ?Sized> BlobStore for Observed<T> {
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), BlobError> {
        self.run("blobs", "put", self.inner.put(key, bytes)).await
    }

    async fn get(&self, key: &str) -> Result<Bytes, BlobError> {
        self.run("blobs", "get", self.inner.get(key)).await
    }

    async fn delete(&self, key: &str) -> Result<(), BlobError> {
        self.run("blobs", "delete", self.inner.delete(key)).await
    }
}

#[async_trait]
impl<T: SecretStore + ?Sized> SecretStore for Observed<T> {
    async fn put(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretError> {
        self.run("secrets", "put", self.inner.put(key, secret)).await
    }

    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        self.run("secrets", "get", self.inner.get(key)).await
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        self.run("secrets", "delete", self.inner.delete(key)).await
    }
}

/// A request's key lookup, as `keys.resolve`.
#[async_trait]
impl<T: KeySource + ?Sized> KeySource for Observed<T> {
    async fn resolve(&self, key: &str) -> Result<Option<AuthContext>, KeyError> {
        self.run("keys", "resolve", self.inner.resolve(key)).await
    }
}

#[async_trait]
impl<T: Runtime + ?Sized> Runtime for Observed<T> {
    async fn launch(&self, job: JobSpec) -> Result<RuntimeHandle, RuntimeError> {
        self.run("sandbox", "launch", self.inner.launch(job)).await
    }

    async fn status(&self, handle: &RuntimeHandle) -> Result<TaskState, RuntimeError> {
        self.run("sandbox", "status", self.inner.status(handle)).await
    }

    async fn stop(&self, handle: &RuntimeHandle) -> Result<(), RuntimeError> {
        self.run("sandbox", "stop", self.inner.stop(handle)).await
    }

    fn handle_for(&self, target_id: &str) -> Option<RuntimeHandle> {
        self.inner.handle_for(target_id)
    }
}

/// A scheduled fire, as `fires.<kind>`.
#[async_trait]
impl<T: FireHandler + ?Sized> FireHandler for Observed<T> {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }

    async fn fire(
        &self,
        tenant: &AuthContext,
        id: &str,
        scheduled_at: DateTime<Utc>,
    ) -> Result<(), FireError> {
        self.run("fires", self.inner.kind(), self.inner.fire(tenant, id, scheduled_at)).await
    }
}
