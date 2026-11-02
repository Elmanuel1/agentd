//! The model call of a step: retried with backoff when the failure is worth
//! retrying, and abandoned when the session is interrupted.

use std::time::Duration;

use super::Runner;
use crate::model::{ModelError, ModelRequest, ModelResponse};

/// Attempts at a model call before the thread gives up.
const ATTEMPTS: u32 = 5;

impl Runner {
    /// The model's answer, retrying what is worth retrying with backoff;
    /// `None` when the session is interrupted first.
    pub(super) async fn call(&self, req: ModelRequest) -> Result<Option<ModelResponse>, ModelError> {
        let mut delay = Duration::from_secs(2);
        let mut cancel = self.cancel.clone();
        for attempt in 1..=ATTEMPTS {
            let result = tokio::select! {
                r = self.models.complete(req.clone()) => r,
                _ = cancel.wait_for(|c| *c) => return Ok(None),
            };
            match result {
                Ok(r) => return Ok(Some(r)),
                Err(ModelError::Retryable(e)) if attempt < ATTEMPTS => {
                    tracing::warn!(error = %e, attempt, "engine: model call failed; retrying");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
                Err(e) => return Err(e),
            }
        }
        Err(ModelError::Fatal("model call attempts exhausted".into()))
    }
}
