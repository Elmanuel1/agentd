//! Scheduled fires. Each resource that owns scheduled work registers one
//! [`FireHandler`] for its [`crate::ports::FireTarget::kind`]; the
//! [`FireRegistry`] runs each fire under the schedule's tenant, and the
//! [`FireWorker`]s run the fire queue through it.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::{ScheduleSpec, TenantRef};
use crate::{AuthContext, OrgId, WorkspaceId, Writer, with_writer};

mod claims;
mod delivery;
pub mod jitter;
mod worker;
pub use claims::{Claimed, Fire, FireClaims, FireKey, NativeFireClaims, Outcome};
pub use delivery::deliver;
pub use worker::{Ending, FireWorker};

/// A fire that did not run.
#[derive(Debug, Clone, thiserror::Error)]
pub enum FireError {
    /// No handler is registered for the target's kind.
    #[error("no fire handler for kind `{0}`")]
    NoHandler(String),
    /// A second handler was registered for a kind.
    #[error("fire handler for kind `{0}` registered twice")]
    DuplicateKind(String),
    /// The handler failed.
    #[error("fire failed: {0}")]
    Failed(String),
    /// The fire's claim could not be read or written.
    #[error("fire claim unavailable: {0}")]
    Unavailable(String),
}

/// Runs one scheduled tick of the resources of one kind.
#[async_trait]
pub trait FireHandler: Send + Sync {
    /// The [`crate::ports::FireTarget::kind`] this handler owns.
    fn kind(&self) -> &'static str;
    /// Fire resource `id` for the tick at `scheduled_at`, as `tenant`.
    async fn fire(
        &self,
        tenant: &AuthContext,
        id: &str,
        scheduled_at: DateTime<Utc>,
    ) -> Result<(), FireError>;
}

/// The fire handlers by kind, behind the fire queue. Handlers register after
/// the scheduler is built, because the services that own them need the
/// scheduler first.
pub struct FireRegistry {
    handlers: RwLock<BTreeMap<&'static str, Arc<dyn FireHandler>>>,
    claims: Arc<dyn FireClaims>,
    enqueued: Box<dyn Fn(usize) + Send + Sync>,
}

impl std::fmt::Debug for FireRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let handlers = self.handlers.read().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("FireRegistry").field("kinds", &handlers.keys().collect::<Vec<_>>()).finish()
    }
}

impl FireRegistry {
    /// No handlers yet; fires queue in `claims`.
    pub fn new(claims: Arc<dyn FireClaims>) -> Self {
        Self { handlers: RwLock::default(), claims, enqueued: Box::new(drop) }
    }

    /// Telling `count` how many rows each [`Self::enqueue`] added.
    #[must_use]
    pub fn counting_enqueued(mut self, count: impl Fn(usize) + Send + Sync + 'static) -> Self {
        self.enqueued = Box::new(count);
        self
    }

    /// Register `handler` for its kind; each kind has one handler.
    pub fn register(&self, handler: Arc<dyn FireHandler>) -> Result<(), FireError> {
        let mut handlers = self.handlers.write().unwrap_or_else(PoisonError::into_inner);
        if handlers.contains_key(handler.kind()) {
            return Err(FireError::DuplicateKind(handler.kind().to_owned()));
        }
        handlers.insert(handler.kind(), handler);
        Ok(())
    }

    /// Write the occurrence of `spec` at `scheduled_at` to the fire queue,
    /// due after its jitter; an occurrence already queued keeps its row.
    pub async fn enqueue(&self, spec: &ScheduleSpec, scheduled_at: DateTime<Utc>) -> Result<(), FireError> {
        let due_at = jitter::due_at(spec, scheduled_at);
        let added = self.claims.enqueue(&[Fire { spec: spec.clone(), scheduled_at, due_at }]).await?;
        (self.enqueued)(added);
        Ok(())
    }

    /// Run the fire of `spec` at `scheduled_at` through its kind's handler,
    /// as the schedule's key writing a request of its own.
    pub async fn run(&self, spec: &ScheduleSpec, scheduled_at: DateTime<Utc>) -> Result<(), FireError> {
        let handler = self.handler(spec)?;
        let writer = Writer::fresh(&spec.tenant.api_key_id);
        let tenant = spec.tenant.auth_context();
        with_writer(writer, handler.fire(&tenant, &spec.target.id, scheduled_at)).await
    }

    fn handler(&self, spec: &ScheduleSpec) -> Result<Arc<dyn FireHandler>, FireError> {
        self.handlers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(spec.target.kind.as_str())
            .cloned()
            .ok_or_else(|| FireError::NoHandler(spec.target.kind.clone()))
    }
}

impl TenantRef {
    /// The tenant of a schedule registered by `ctx`.
    pub fn of(ctx: &AuthContext) -> Self {
        Self {
            org_id: ctx.org.as_str().to_owned(),
            workspace_id: ctx.workspace.as_str().to_owned(),
            api_key_id: ctx.api_key_id.clone(),
        }
    }

    /// The identity a fire of this schedule runs as.
    pub fn auth_context(&self) -> AuthContext {
        AuthContext {
            org: OrgId::new(&self.org_id),
            workspace: WorkspaceId::new(&self.workspace_id),
            api_key_id: self.api_key_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
