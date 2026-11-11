//! The account-wide model budget (M-4): requests and input tokens per
//! provider and minute, shared by every loop worker. A step over it is
//! deferred to the next minute without calling the model.

use async_trait::async_trait;
use shared::BackendError;

/// Whether a step may call the model now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// It may; the request is counted.
    Yes,
    /// Not before the next minute starts, this many seconds from now.
    Later(i64),
}

/// The shared budget.
#[async_trait]
pub trait ModelBudget: Send + Sync {
    /// Count one request to `provider`, unless this minute's requests or
    /// input tokens are already at the budget.
    async fn admit(&self, provider: &str) -> Result<Admit, BackendError>;
    /// Add the input tokens a request to `provider` used.
    async fn spend(&self, provider: &str, input_tokens: u64) -> Result<(), BackendError>;
}

/// No budget: every request is admitted (limits unset, or no Valkey).
#[derive(Debug, Clone, Copy, Default)]
pub struct Unlimited;

#[async_trait]
impl ModelBudget for Unlimited {
    async fn admit(&self, _: &str) -> Result<Admit, BackendError> {
        Ok(Admit::Yes)
    }

    async fn spend(&self, _: &str, _: u64) -> Result<(), BackendError> {
        Ok(())
    }
}
