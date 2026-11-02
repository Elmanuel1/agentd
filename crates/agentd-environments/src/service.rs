//! Environment business logic.
//!
//! Live-verified behaviors enforced here:
//! - names are **not unique** (duplicate create succeeds with a new id),
//! - `description` canonicalizes to `""`,
//! - environments are org-scoped (every workspace of the org sees them).

use std::sync::Arc;

use agentd_core::limits::{required, validate_string_len, values};
use agentd_core::metadata::MetadataPolicy;
use agentd_core::{ApiError, AuthContext, Pagination, Scope, Timestamp};

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
        self.store.insert(&ctx.org, env.clone()).await.map_err(map_store)?;
        Ok(env)
    }

    /// `GET /v1/environments/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Environment, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.org, &id).await.map_err(map_store)
    }

    /// `GET /v1/environments`.
    pub async fn list(&self, ctx: &AuthContext, page: &Pagination) -> Result<Vec<Environment>, ApiError> {
        self.store.list(&ctx.org, page).await.map_err(map_store)
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
            return Err(ApiError::conflict_invalid_request("cannot update an archived environment"));
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
        self.store.replace(&ctx.org, env.clone()).await.map_err(map_store)?;
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
            self.store.replace(&ctx.org, env.clone()).await.map_err(map_store)?;
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
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
