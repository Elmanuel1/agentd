//! Scheduled fires. Each resource that owns scheduled work registers one
//! [`FireHandler`] for its [`crate::ports::FireTarget::kind`]; the scheduler delivers every
//! tick to the [`FireRegistry`], which runs it under the schedule's tenant.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::{ScheduleSpec, TenantRef};
use crate::{with_writer, AuthContext, OrgId, WorkspaceId, Writer};

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

/// The fire handlers by kind. Handlers register after the scheduler is built,
/// because the services that own them need the scheduler first.
#[derive(Default)]
pub struct FireRegistry {
    handlers: RwLock<BTreeMap<&'static str, Arc<dyn FireHandler>>>,
}

impl std::fmt::Debug for FireRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let handlers = self.handlers.read().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("FireRegistry").field("kinds", &handlers.keys().collect::<Vec<_>>()).finish()
    }
}

impl FireRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
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

    /// Deliver one tick of `spec` to the handler of its target's kind, as
    /// the schedule's key writing a request of its own.
    pub async fn fire(&self, spec: &ScheduleSpec, scheduled_at: DateTime<Utc>) -> Result<(), FireError> {
        let handler = self
            .handlers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(spec.target.kind.as_str())
            .cloned()
            .ok_or_else(|| FireError::NoHandler(spec.target.kind.clone()))?;
        let writer = Writer::fresh(&spec.tenant.api_key_id);
        let tenant = spec.tenant.auth_context();
        with_writer(writer, handler.fire(&tenant, &spec.target.id, scheduled_at)).await
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
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::ports::FireTarget;

    #[derive(Default)]
    struct Recording {
        fires: Mutex<Vec<(AuthContext, String, DateTime<Utc>)>>,
        writers: Mutex<Vec<Writer>>,
    }

    #[async_trait]
    impl FireHandler for Recording {
        fn kind(&self) -> &'static str {
            "deployment"
        }
        async fn fire(&self, tenant: &AuthContext, id: &str, at: DateTime<Utc>) -> Result<(), FireError> {
            self.fires.lock().unwrap().push((tenant.clone(), id.to_owned(), at));
            self.writers.lock().unwrap().push(Writer::current());
            Ok(())
        }
    }

    fn spec(kind: &str) -> ScheduleSpec {
        ScheduleSpec {
            cron: "0 0 * * *".into(),
            timezone: "UTC".into(),
            target: FireTarget::new(kind, "id_1"),
            tenant: TenantRef {
                org_id: "org_a".into(),
                workspace_id: "wrkspc_a".into(),
                api_key_id: "apikey_a".into(),
            },
        }
    }

    #[tokio::test]
    async fn fire_runs_the_kinds_handler_under_the_schedules_tenant_as_its_key() {
        let registry = FireRegistry::new();
        let handler = Arc::new(Recording::default());
        registry.register(handler.clone()).unwrap();
        let at = Utc::now();
        registry.fire(&spec("deployment"), at).await.unwrap();
        let fires = handler.fires.lock().unwrap();
        let (tenant, id, fired_at) = fires.first().unwrap();
        assert_eq!(tenant.org.as_str(), "org_a");
        assert_eq!(tenant.workspace.as_str(), "wrkspc_a");
        assert_eq!(tenant.api_key_id, "apikey_a");
        assert_eq!((id.as_str(), *fired_at), ("id_1", at));
        let writer = handler.writers.lock().unwrap().first().cloned().unwrap();
        assert_eq!(writer.actor, "apikey_a");
        assert!(writer.request_id.starts_with("req_"), "{}", writer.request_id);
    }

    #[tokio::test]
    async fn fire_of_a_kind_with_no_handler_is_an_error() {
        let registry = FireRegistry::new();
        registry.register(Arc::new(Recording::default())).unwrap();
        let err = registry.fire(&spec("dream"), Utc::now()).await.unwrap_err();
        assert!(matches!(err, FireError::NoHandler(kind) if kind == "dream"));
    }

    #[test]
    fn a_kind_registers_one_handler() {
        let registry = FireRegistry::new();
        registry.register(Arc::new(Recording::default())).unwrap();
        let err = registry.register(Arc::new(Recording::default())).unwrap_err();
        assert!(matches!(err, FireError::DuplicateKind(kind) if kind == "deployment"));
    }

    #[test]
    fn fire_target_keys_and_json_are_stable() {
        let target = FireTarget::new("deployment", "depl_1");
        assert_eq!(target.key(), "deployment/depl_1");
        assert_eq!(
            serde_json::to_value(&target).unwrap(),
            serde_json::json!({"kind": "deployment", "id": "depl_1"})
        );
    }

    #[test]
    fn tenant_round_trips_through_the_auth_context() {
        let ctx = AuthContext {
            org: OrgId::new("org_b"),
            workspace: WorkspaceId::new("wrkspc_b"),
            api_key_id: "apikey_b".into(),
        };
        assert_eq!(TenantRef::of(&ctx).auth_context(), ctx);
    }
}
