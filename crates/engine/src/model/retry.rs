//! The one retry layer (M-2): a call that fails with 408, 409, 429, 5xx or a
//! transport error is tried again with exponential backoff and full jitter,
//! never sooner than the provider's `retry-after`, up to the attempts and
//! inside the step's deadline.

use std::time::Duration;

use rand::Rng;
use tokio::time::Instant;

use super::{ModelClient, ModelError, ModelRequest, ModelResponse};

/// How a step retries its model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// Tries in all (`AGENTD_MODEL_ATTEMPTS`).
    pub attempts: u32,
    /// The first backoff, doubled each try.
    pub first: Duration,
    /// The step's deadline for the call and its retries.
    pub deadline: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self { attempts: 5, first: Duration::from_secs(2), deadline: Duration::from_secs(600) }
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
        let end = Instant::now() + self.deadline;
        let mut backoff = self.first;
        let mut tried = 0;
        loop {
            tried += 1;
            let left = end.saturating_duration_since(Instant::now());
            let answer = tokio::time::timeout(left, client.complete(req.clone())).await;
            let error = match answer {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(e @ ModelError::Fatal(_))) => return Err(e),
                Ok(Err(e)) => e,
                Err(_) => return Err(ModelError::Retryable("the step's deadline passed".into())),
            };
            let floor = match &error {
                ModelError::RetryAfter(_, after) => *after,
                _ => Duration::ZERO,
            };
            let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..=millis(backoff)));
            let wait = jitter.max(floor);
            if tried >= self.attempts || Instant::now() + wait >= end {
                return Err(error);
            }
            tokio::time::sleep(wait).await;
            backoff = backoff.saturating_mul(2);
        }
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
