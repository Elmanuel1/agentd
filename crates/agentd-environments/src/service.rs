//! Environment business logic.
//!
//! Live-verified behaviors enforced here:
//! - names are **not unique** (duplicate create succeeds with a new id),
//! - `description` canonicalizes to `""`,
//! - environments are org-scoped (every workspace of the org sees them).

use std::sync::Arc;

use agentd_core::limits::{required, validate_string_len, values};
use agentd_core::metadata::MetadataPolicy;
use agentd_core::{ApiError, AuthContext, Scope, Timestamp};

use crate::store::{EnvironmentsStore, StoreError};
use crate::types::{
    CreateEnvironmentRequest, Environment, EnvironmentConfig, EnvironmentId, EnvironmentState,
    EnvironmentTag, UpdateEnvironmentRequest,
};

/// Environment service.
pub struct EnvironmentsService {
    store: Arc<dyn EnvironmentsStore>,
}

impl std::fmt::Debug for EnvironmentsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentsService").finish_non_exhaustive()
    }
}

impl EnvironmentsService {
    /// Build with a store.
    pub fn new(store: Arc<dyn EnvironmentsStore>) -> Self {
        Self { store }
    }

    /// `POST /v1/environments` — duplicate names allowed (live-verified).
    pub async fn create(
        &self,
        ctx: &AuthContext,
        req: CreateEnvironmentRequest,
    ) -> Result<Environment, ApiError> {
        let name = required("name", req.name)?;
        validate_string_len("name", &name, values::NAME_MIN, values::NAME_MAX)?;
        let config = required("config", req.config)?;
        req.metadata.validate(MetadataPolicy::capped())?;
        let now = Timestamp::now();
        let env = Environment {
            id: EnvironmentId::generate(),
            tag: EnvironmentTag::Environment,
            name,
            description: req.description.unwrap_or_default(),
            created_at: now,
            updated_at: now,
            archived_at: None,
            state: EnvironmentState::Active,
            config: canonicalize(config),
            metadata: req.metadata,
            scope: Scope::Organization,
        };
        self.store
            .insert(&ctx.org, env.clone())
            .await
            .map_err(map_store)?;
        Ok(env)
    }

    /// `GET /v1/environments/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Environment, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.org, &id).await.map_err(map_store)
    }

    /// `GET /v1/environments`.
    pub async fn list(&self, ctx: &AuthContext) -> Result<Vec<Environment>, ApiError> {
        self.store.list(&ctx.org).await.map_err(map_store)
    }

    /// `POST /v1/environments/{id}` — changes apply to new containers only.
    pub async fn update(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        req: UpdateEnvironmentRequest,
    ) -> Result<Environment, ApiError> {
        let id = parse_id(raw_id)?;
        let mut env = self.store.get(&ctx.org, &id).await.map_err(map_store)?;
        if env.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request(
                "cannot update an archived environment",
            ));
        }
        if let Some(name) = req.name {
            validate_string_len("name", &name, values::NAME_MIN, values::NAME_MAX)?;
            env.name = name;
        }
        if let Some(d) = req.description {
            env.description = d;
        }
        if let Some(c) = req.config {
            env.config = canonicalize(c);
        }
        if let Some(m) = req.metadata {
            m.validate(MetadataPolicy::capped())?;
            env.metadata = m;
        }
        env.updated_at = Timestamp::now();
        self.store
            .replace(&ctx.org, env.clone())
            .await
            .map_err(map_store)?;
        Ok(env)
    }

    /// `POST /v1/environments/{id}/archive` — terminal, read-only after.
    pub async fn archive(&self, ctx: &AuthContext, raw_id: &str) -> Result<Environment, ApiError> {
        let id = parse_id(raw_id)?;
        let mut env = self.store.get(&ctx.org, &id).await.map_err(map_store)?;
        if env.archived_at.is_none() {
            env.archived_at = Some(Timestamp::now());
            env.state = EnvironmentState::Archived;
            env.updated_at = Timestamp::now();
            self.store
                .replace(&ctx.org, env.clone())
                .await
                .map_err(map_store)?;
        }
        Ok(env)
    }

    /// `DELETE /v1/environments/{id}` — live returned 200 (docs say 204).
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<Environment, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.delete(&ctx.org, &id).await.map_err(map_store)
    }
}

/// Round-trip through the canonical enum guarantees defaults are populated —
/// deserialization already expands them, so serialization is canonical.
fn canonicalize(c: EnvironmentConfig) -> EnvironmentConfig {
    c
}

fn parse_id(raw: &str) -> Result<EnvironmentId, ApiError> {
    EnvironmentId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("environment not found"),
        StoreError::Backend(m) => ApiError::internal(m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryEnvironmentsStore;
    use agentd_core::{OrgId, WorkspaceId};

    fn svc() -> EnvironmentsService {
        EnvironmentsService::new(Arc::new(InMemoryEnvironmentsStore::new()))
    }

    fn ctx() -> AuthContext {
        AuthContext {
            org: OrgId("org_t".into()),
            workspace: WorkspaceId("wrkspc_01a".into()),
            api_key_id: "apikey_01".into(),
        }
    }

    /// Same org, different workspace — env must still be visible (org scope).
    fn ctx_other_ws() -> AuthContext {
        AuthContext {
            org: OrgId("org_t".into()),
            workspace: WorkspaceId("wrkspc_02b".into()),
            api_key_id: "apikey_02".into(),
        }
    }

    fn minimal() -> CreateEnvironmentRequest {
        CreateEnvironmentRequest {
            name: Some("compat-probe-env".into()),
            config: serde_json::from_str(
                r#"{"type":"cloud","networking":{"type":"unrestricted"}}"#,
            )
            .ok(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn create_matches_live_canonical_shape() {
        let s = svc();
        let e = s.create(&ctx(), minimal()).await.unwrap();
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "environment");
        assert_eq!(v["description"], ""); // live: empty string, NOT null
        assert_eq!(v["state"], "active");
        assert_eq!(v["scope"], "organization"); // only environments carry scope
        assert_eq!(v["config"]["packages"]["type"], "packages");
        assert_eq!(v["config"]["init_script"], "");
        assert!(e.id.as_str().starts_with("env_01"));
    }

    #[tokio::test]
    async fn duplicate_names_allowed_like_live_api() {
        // Live probe: second create with the same name → 200 with a new id.
        let s = svc();
        let a = s.create(&ctx(), minimal()).await.unwrap();
        let b = s.create(&ctx(), minimal()).await.unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(a.name, b.name);
        assert_eq!(s.list(&ctx()).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn org_scoped_visible_across_workspaces() {
        let s = svc();
        let e = s.create(&ctx(), minimal()).await.unwrap();
        // Different workspace, same org → visible.
        let got = s.get(&ctx_other_ws(), e.id.as_str()).await.unwrap();
        assert_eq!(got.id, e.id);
    }

    #[tokio::test]
    async fn missing_fields_use_required_error() {
        let s = svc();
        let e = s
            .create(&ctx(), CreateEnvironmentRequest::default())
            .await
            .unwrap_err();
        assert_eq!(e.message, "name: Field required");
        let e = s
            .create(
                &ctx(),
                CreateEnvironmentRequest {
                    name: Some("x".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(e.message, "config: Field required");
    }

    #[tokio::test]
    async fn malformed_id_message_uses_environment_label() {
        let s = svc();
        let e = s.get(&ctx(), "env_00bogus").await.unwrap_err();
        assert_eq!(e.status, 400);
        assert_eq!(e.message, "Invalid environment ID.");
    }

    #[tokio::test]
    async fn archive_then_update_conflicts() {
        let s = svc();
        let e = s.create(&ctx(), minimal()).await.unwrap();
        let archived = s.archive(&ctx(), e.id.as_str()).await.unwrap();
        assert!(archived.archived_at.is_some());
        assert_eq!(archived.state, EnvironmentState::Archived);
        let err = s
            .update(&ctx(), e.id.as_str(), UpdateEnvironmentRequest::default())
            .await
            .unwrap_err();
        assert_eq!(err.status, 409);
    }

    #[tokio::test]
    async fn delete_removes() {
        let s = svc();
        let e = s.create(&ctx(), minimal()).await.unwrap();
        s.delete(&ctx(), e.id.as_str()).await.unwrap();
        assert_eq!(
            s.get(&ctx(), e.id.as_str()).await.unwrap_err().status,
            404
        );
    }

    #[tokio::test]
    async fn update_replaces_config_and_bumps_updated_at() {
        let s = svc();
        let e = s.create(&ctx(), minimal()).await.unwrap();
        let updated = s
            .update(
                &ctx(),
                e.id.as_str(),
                UpdateEnvironmentRequest {
                    config: serde_json::from_str(r#"{"type":"self_hosted"}"#).ok(),
                    description: Some("now self hosted".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            updated.config,
            EnvironmentConfig::SelfHosted {}
        ));
        assert_eq!(updated.description, "now self hosted");
    }
}
