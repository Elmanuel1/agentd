//! The one retry layer (M-2): a call that fails with 408, 409, 429, 5xx or a
//! transport error is tried again with exponential backoff and full jitter,
//! never sooner than the provider's `retry-after`, up to the attempts and
//! inside the step's deadline. Each provider has a breaker on this replica
//! (SH-47): 5 retryable failures in a row, across every step, open it for
//! 30 s or the provider's `retry-after` if later. While it is open a step
//! waits instead of calling, spending none of its tries; only a step whose
//! tries or deadline run out fails (SH-48).

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
        let answer = self.attempts(provider, || client.complete(req.clone())).await;
        let outcome = if answer.is_ok() { "ok" } else { "error" };
        crate::metrics::model_call(provider, started.elapsed().as_secs_f64(), outcome);
        answer
    }

    /// `model`'s window, read with the same retries as a call, so a
    /// transient failure of the provider's model list does not fail a turn.
    pub async fn window(&self, client: &dyn ModelClient, model: &str) -> Result<u64, ModelError> {
        self.attempts(super::ModelProvider::of(model).name(), || client.window(model)).await
    }

    /// Wait while `provider`'s breaker holds calls, inside `end`; an error once `end` would pass first.
    async fn admitted(&self, provider: &'static str, end: Instant) -> Result<(), ModelError> {
        loop {
            let until = match self.breakers.pass(&provider) {
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

    async fn attempts<T, F, Fut>(&self, provider: &'static str, try_once: F) -> Result<T, ModelError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, ModelError>>,
    {
        let end = Instant::now() + self.deadline;
        let mut backoff = self.first;
        let mut tried = 0;
        loop {
            self.admitted(provider, end).await?;
            tried += 1;
            let left = end.saturating_duration_since(Instant::now());
            let answer = tokio::time::timeout(left, try_once()).await;
            let error = match answer {
                Ok(Ok(response)) => {
                    self.breakers.succeeded(&provider);
                    return Ok(response);
                }
                Ok(Err(e @ ModelError::Fatal(_))) => {
                    self.breakers.succeeded(&provider);
                    return Err(e);
                }
                Ok(Err(e)) => e,
                Err(_) => return Err(ModelError::Retryable("the step's deadline passed".into())),
            };
            let floor = match &error {
                ModelError::RetryAfter(_, after) => *after,
                _ => Duration::ZERO,
            };
            if self.breakers.failed(&provider, Some(floor).filter(|f| !f.is_zero())) {
                crate::metrics::model_breaker_opened(provider);
                tracing::warn!(provider, "loop: the provider failed 5 calls in a row; steps wait for it");
            }
            let jitter = Duration::from_millis(rand::rng().random_range(0..=millis(backoff)));
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::NativeModelClient;
    use super::*;

    /// Fails its first `failing` calls with `error`, then answers; records when each call came.
    struct Flaky {
        failing: Mutex<usize>,
        error: ModelError,
        calls: Mutex<Vec<Instant>>,
    }

    impl Flaky {
        fn new(failing: usize, error: ModelError) -> Self {
            Self { failing: Mutex::new(failing), error, calls: Mutex::default() }
        }
    }

    impl NativeModelClient for Flaky {
        async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, ModelError> {
            Err(ModelError::Fatal("unused".into()))
        }

        async fn window(&self, _: &str) -> Result<u64, ModelError> {
            self.calls.lock().unwrap().push(Instant::now());
            let mut failing = self.failing.lock().unwrap();
            if *failing == 0 {
                return Ok(200_000);
            }
            *failing -= 1;
            Err(self.error.clone())
        }
    }

    fn retry(deadline: Duration) -> Retry {
        Retry { attempts: 5, first: Duration::from_millis(10), deadline, ..Retry::default() }
    }

    #[tokio::test(start_paused = true)]
    async fn after_5_failures_the_provider_gets_no_call_for_30_s_and_steps_wait_then_one_success_closes_it() {
        let flaky = Flaky::new(5, ModelError::Retryable("529".into()));
        let r = retry(Duration::from_secs(600));
        assert!(r.window(&flaky, "claude-haiku-4-5").await.is_err(), "the first step spends its 5 tries");
        let opened = Instant::now();
        assert_eq!(
            r.window(&flaky, "claude-haiku-4-5").await.unwrap(),
            200_000,
            "the next step waits, not fails"
        );
        let calls = flaky.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 6);
        assert!(calls[5] - opened >= breaker_open(), "no call while open");
        let before = Instant::now();
        r.window(&flaky, "claude-haiku-4-5").await.unwrap();
        assert_eq!(Instant::now(), before, "closed: the call goes at once");
    }

    fn breaker_open() -> Duration {
        shared::breaker::OPEN - Duration::from_millis(100)
    }

    #[tokio::test(start_paused = true)]
    async fn a_retry_after_longer_than_30_s_holds_the_breaker_open() {
        let flaky = Flaky::new(5, ModelError::RetryAfter("429".into(), Duration::from_secs(90)));
        let r = retry(Duration::from_secs(1_000));
        let started = Instant::now();
        assert!(r.window(&flaky, "claude-haiku-4-5").await.is_err());
        let failed = Instant::now();
        r.window(&flaky, "claude-haiku-4-5").await.unwrap();
        assert!(
            flaky.calls.lock().unwrap()[5] - failed >= Duration::from_secs(89),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_step_whose_deadline_passes_while_open_fails_without_a_call() {
        let flaky = Flaky::new(5, ModelError::Retryable("503".into()));
        let r = retry(Duration::from_secs(600));
        assert!(r.window(&flaky, "claude-haiku-4-5").await.is_err());
        let short = Retry { deadline: Duration::from_secs(10), ..r.clone() };
        assert!(short.window(&flaky, "claude-haiku-4-5").await.is_err(), "ends as today, retries exhausted");
        assert_eq!(flaky.calls.lock().unwrap().len(), 5, "no call while open");
    }
}
