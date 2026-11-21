//! The one retry layer (M-2): a call that fails with 408, 409, 429, 5xx or a
//! transport error is tried again with exponential backoff and full jitter,
//! never sooner than the provider's `retry-after`, up to the attempts and
//! inside the step's deadline. Each provider has a breaker on this replica
//! (SH-47): 5 retryable failures in a row, across every step, open it for
//! 30 s or the provider's `retry-after` if later, at most 5 minutes; an
//! attempt that never answers counts as a failure. While it is open a step
//! waits instead of calling, spending none of its tries; only a step whose
//! tries or deadline run out fails (SH-48). Completions alone go through the
//! breaker: a model's window can come from cache, so its lookup is neither
//! held nor counted.

use std::sync::Arc;
use std::time::Duration;

use rand::RngExt;
use shared::breaker::{Breakers, Pass};
use tokio::time::Instant;

use super::{ModelClient, ModelError, ModelRequest, ModelResponse};

/// How long a step waits for the answer of another step's probe before it asks again.
const PROBE_WAIT: Duration = Duration::from_millis(500);

/// How a step retries its model call.
#[derive(Debug, Clone)]
pub struct Retry {
    /// Tries in all (`AGENTD_MODEL_ATTEMPTS`).
    pub attempts: u32,
    /// The first backoff, doubled each try.
    pub first: Duration,
    /// The step's deadline for the call and its retries.
    pub deadline: Duration,
    /// The providers' breakers, shared by every step of the replica.
    pub breakers: Arc<Breakers<&'static str>>,
}

impl Default for Retry {
    fn default() -> Self {
        let (first, deadline) = (Duration::from_secs(2), Duration::from_secs(600));
        Self { attempts: 5, first, deadline, breakers: Arc::default() }
    }
}

impl Retry {
    /// Call `client` until it answers, fails for good, or runs out of tries
    /// or time.
    pub async fn call(
        &self,
        client: &dyn ModelClient,
        req: ModelRequest,
    ) -> Result<ModelResponse, ModelError> {
        let started = Instant::now();
        let provider = super::ModelProvider::of(&req.model).name();
        let answer = self.attempts(provider, Some(&self.breakers), || client.complete(req.clone())).await;
        let outcome = if answer.is_ok() { "ok" } else { "error" };
        crate::metrics::model_call(provider, started.elapsed().as_secs_f64(), outcome);
        answer
    }

    /// `model`'s window, read with the same retries as a call, so a
    /// transient failure of the provider's model list does not fail a turn.
    /// It leaves the provider's breaker alone: a window can come from cache,
    /// so its answer says nothing of whether completions recovered.
    pub async fn window(&self, client: &dyn ModelClient, model: &str) -> Result<u64, ModelError> {
        self.attempts(super::ModelProvider::of(model).name(), None, || client.window(model)).await
    }

    /// Wait while `provider`'s breaker holds calls, inside `end`; an error once `end` would pass first.
    async fn admitted(
        breakers: &Breakers<&'static str>,
        provider: &'static str,
        end: Instant,
    ) -> Result<(), ModelError> {
        loop {
            let until = match breakers.pass(&provider) {
                Pass::Go => return Ok(()),
                Pass::Until(until) => until,
                Pass::Probing => Instant::now() + PROBE_WAIT,
            };
            if until >= end {
                return Err(ModelError::Retryable(format!(
                    "{provider} is unavailable past the step's deadline"
                )));
            }
            tokio::time::sleep_until(until).await;
        }
    }

    async fn attempts<T, F, Fut>(
        &self,
        provider: &'static str,
        breakers: Option<&Breakers<&'static str>>,
        try_once: F,
    ) -> Result<T, ModelError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, ModelError>>,
    {
        let end = Instant::now() + self.deadline;
        let mut backoff = self.first;
        let mut tried = 0;
        loop {
            if let Some(b) = breakers {
                Self::admitted(b, provider, end).await?;
            }
            tried += 1;
            let left = end.saturating_duration_since(Instant::now());
            let answer = tokio::time::timeout(left, try_once()).await;
            let error = match answer {
                Ok(Ok(response)) => {
                    succeeded(breakers, provider);
                    return Ok(response);
                }
                Ok(Err(e @ ModelError::Fatal(_))) => {
                    succeeded(breakers, provider);
                    return Err(e);
                }
                Ok(Err(e)) => e,
                Err(_) => {
                    // A provider that never answers counts as failing too.
                    failed(breakers, provider, None);
                    return Err(ModelError::Retryable("the step's deadline passed".into()));
                }
            };
            let floor = match &error {
                ModelError::RetryAfter(_, after) => *after,
                _ => Duration::ZERO,
            };
            failed(breakers, provider, Some(floor).filter(|f| !f.is_zero()));
            let jitter = Duration::from_millis(rand::rng().random_range(0..=millis(backoff)));
            let wait = jitter.max(floor);
            if tried >= self.attempts || Instant::now().checked_add(wait).is_none_or(|at| at >= end) {
                return Err(error);
            }
            tokio::time::sleep(wait).await;
            backoff = backoff.saturating_mul(2);
        }
    }
}

fn succeeded(breakers: Option<&Breakers<&'static str>>, provider: &'static str) {
    if let Some(b) = breakers {
        b.succeeded(&provider);
    }
}

fn failed(breakers: Option<&Breakers<&'static str>>, provider: &'static str, hold: Option<Duration>) {
    if breakers.is_some_and(|b| b.failed(&provider, hold)) {
        crate::metrics::model_breaker_opened(provider);
        tracing::warn!(provider, "loop: the provider failed 5 calls in a row; steps wait for it");
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
