//! The account-wide model budget (M-4): requests and input tokens per
//! provider and minute, shared by every loop worker. A step over it is
//! deferred to the next minute without calling the model.

use shared::ports::BoxFuture;
use shared::BackendError;
use std::future::Future;

/// Whether a step may call the model now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// It may; the request is counted.
    Yes,
    /// Not before the next minute starts, this many seconds from now.
    Later(i64),
}

/// The shared budget.
pub trait ModelBudget: Send + Sync {
    /// Count one request to `provider`, unless this minute's requests or
    /// input tokens are already at the budget.
    fn admit<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Result<Admit, BackendError>>;
    /// Add the input tokens a request to `provider` used.
    fn spend<'a>(&'a self, provider: &'a str, input_tokens: u64) -> BoxFuture<'a, Result<(), BackendError>>;
}

/// What an adapter implements to provide [`ModelBudget`]; the methods are documented there.
pub trait NativeModelBudget: Send + Sync {
    fn admit(&self, provider: &str) -> impl Future<Output = Result<Admit, BackendError>> + Send;
    fn spend(
        &self,
        provider: &str,
        input_tokens: u64,
    ) -> impl Future<Output = Result<(), BackendError>> + Send;
}

impl<T: NativeModelBudget> ModelBudget for T {
    fn admit<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Result<Admit, BackendError>> {
        Box::pin(NativeModelBudget::admit(self, provider))
    }
    fn spend<'a>(&'a self, provider: &'a str, input_tokens: u64) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(NativeModelBudget::spend(self, provider, input_tokens))
    }
}

/// No budget: every request is admitted (limits unset, or no Valkey).
#[derive(Debug, Clone, Copy, Default)]
pub struct Unlimited;

impl NativeModelBudget for Unlimited {
    async fn admit(&self, _: &str) -> Result<Admit, BackendError> {
        Ok(Admit::Yes)
    }

    async fn spend(&self, _: &str, _: u64) -> Result<(), BackendError> {
        Ok(())
    }
}
