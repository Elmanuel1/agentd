//! Environment business logic.
//!
//! Live-verified behaviors enforced here:
//! - names are **not unique** (duplicate create succeeds with a new id),
//! - a create without `description` stores `""`,
//! - environments are org-scoped (every workspace of the org sees them).

use std::sync::Arc;

use shared::limits::{required, validate_string_len, values};
use shared::metadata::MetadataPolicy;
use shared::{ApiError, AuthContext, Pagination, Scope, Timestamp};

use crate::store::{EnvironmentsStore, StoreError};
use crate::types::{
    CreateEnvironmentRequest, Environment, EnvironmentConfig, EnvironmentId, EnvironmentState,
    EnvironmentTag, ScopeParam, UpdateEnvironmentRequest,
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
        let config = match req.config {
            Some(params) => params.apply(None)?,
            None => EnvironmentConfig::default(),
        };
        req.metadata.validate(MetadataPolicy::capped())?;
        let scope = scope(req.scope)?;
        let now = Timestamp::now();
        let env = Environment {
            id: EnvironmentId::generate(),
            tag: EnvironmentTag::Environment,
            name,
            description: req.description.apply_to_option(Some(String::new())),
            created_at: now,
            updated_at: now,
            archived_at: None,
            state: EnvironmentState::Active,
            config,
            metadata: req.metadata,
            scope,
        };
        self.store.insert(&ctx.org, env.clone()).await.map_err(map_store)?;
        Ok(env)
    }

    /// `GET /v1/environments/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Environment, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.org, &id).await.map_err(map_store)
    }

    /// `GET /v1/environments`: active ones, newest first; archived ones too
    /// with `include_archived`.
    pub async fn list(
        &self,
        ctx: &AuthContext,
        include_archived: bool,
        page: &Pagination,
    ) -> Result<Vec<Environment>, ApiError> {
        self.store.list(&ctx.org, include_archived, page).await.map_err(map_store)
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
        env.description = req.description.apply_to_option(env.description);
        if let Some(c) = req.config {
            env.config = c.apply(Some(&env.config))?;
        }
        env.metadata = env.metadata.merged(req.metadata);
        env.metadata.validate(MetadataPolicy::capped())?;
        if req.scope.is_some() {
            env.scope = scope(req.scope)?;
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

/// API organizations support only the `organization` scope (SDK docs).
fn scope(requested: Option<ScopeParam>) -> Result<Scope, ApiError> {
    match requested {
        None | Some(ScopeParam::Organization) => Ok(Scope::Organization),
        Some(ScopeParam::Account) => {
            Err(ApiError::invalid_request("scope: 'account' is not supported for API organizations"))
        }
    }
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
