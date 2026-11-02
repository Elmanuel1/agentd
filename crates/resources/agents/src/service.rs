//! Agent business logic: integer versioning and the archive lifecycle. The
//! configuration rules live on the request types (`types::validate`).

use std::sync::Arc;

use shared::limits::ModelCatalog;
use shared::{ApiError, AuthContext, Pagination, Timestamp};

use crate::store::{AgentFilter, AgentsStore, StoreError};
use crate::types::{Agent, AgentConfig, AgentId, AgentRef, AgentTag, CreateAgentRequest, UpdateAgentRequest};

mod multiagent;

/// Agent service — all handlers delegate here.
pub struct AgentsService {
    store: Arc<dyn AgentsStore>,
    catalog: ModelCatalog,
}

impl std::fmt::Debug for AgentsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentsService").finish_non_exhaustive()
    }
}

/// Live 400 for a stale `version`, verbatim; also the answer to a lost race.
const CONCURRENT: &str = "Concurrent modification detected. Please fetch the latest version and retry.";

impl AgentsService {
    /// Build with a store and model catalog.
    pub fn new(store: Arc<dyn AgentsStore>, catalog: ModelCatalog) -> Self {
        Self { store, catalog }
    }

    /// `POST /v1/agents`.
    pub async fn create(&self, ctx: &AuthContext, req: CreateAgentRequest) -> Result<Agent, ApiError> {
        let draft = req.into_draft(&self.catalog)?;
        let id = AgentId::generate();
        let this = AgentRef { id: id.clone(), tag: AgentTag::Agent, version: 1 };
        let multiagent = self.resolve_multiagent(ctx, &this, None, draft.multiagent).await?;
        let now = Timestamp::now();
        let agent = Agent {
            id,
            config: AgentConfig { multiagent, ..draft.config },
            archived_at: None,
            created_at: now,
            metadata: draft.metadata,
            tag: AgentTag::Agent,
            updated_at: now,
            version: 1,
        };
        self.store.insert(&ctx.workspace, agent.clone()).await.map_err(map_store)?;
        Ok(agent)
    }

    /// `GET /v1/agents/{id}` — malformed id → 400 "Invalid agent ID."
    /// (live-verified: not 404).
    pub async fn get(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        version: Option<u64>,
    ) -> Result<Agent, ApiError> {
        let id = parse_id(raw_id)?;
        match version {
            // Live: `?version=N` returns that specific version.
            Some(v) => self.store.version(&ctx.workspace, &id, v).await,
            None => self.store.latest(&ctx.workspace, &id).await,
        }
        .map_err(map_store)
    }

    /// `GET /v1/agents` — the latest version of each agent, newest first.
    pub async fn list(
        &self,
        ctx: &AuthContext,
        filter: &AgentFilter,
        page: &Pagination,
    ) -> Result<Vec<Agent>, ApiError> {
        self.store.list(&ctx.workspace, filter, page).await.map_err(map_store)
    }

    /// `GET /v1/agents/{id}/versions` — ascending.
    pub async fn versions(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vec<Agent>, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.versions(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `POST /v1/agents/{id}`. A change mints version latest+1; an update that
    /// changes nothing returns the agent as it is. `version`, when sent, must
    /// be the latest.
    pub async fn update(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        req: UpdateAgentRequest,
    ) -> Result<Agent, ApiError> {
        let id = parse_id(raw_id)?;
        let current = self.store.latest(&ctx.workspace, &id).await.map_err(map_store)?;
        if current.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("cannot update an archived agent"));
        }
        if req.version.is_some_and(|v| v != current.version) {
            return Err(ApiError::invalid_request(CONCURRENT));
        }
        let draft = req.apply_to(&current, &self.catalog)?;
        let this = AgentRef { id: current.id.clone(), tag: AgentTag::Agent, version: current.version + 1 };
        let stored = current.config.multiagent.as_ref();
        let multiagent = self.resolve_multiagent(ctx, &this, stored, draft.multiagent).await?;
        let config = AgentConfig { multiagent, ..draft.config };
        if config == current.config && draft.metadata == current.metadata {
            return Ok(current);
        }
        let next = Agent {
            config,
            metadata: draft.metadata,
            updated_at: Timestamp::now(),
            version: this.version,
            ..current
        };
        match self.store.append_version(&ctx.workspace, next.clone()).await {
            Ok(()) => Ok(next),
            Err(StoreError::Conflict) => Err(ApiError::invalid_request(CONCURRENT)),
            Err(e) => Err(map_store(e)),
        }
    }

    /// `POST /v1/agents/{id}/archive` — permanent, idempotent stamp.
    pub async fn archive(&self, ctx: &AuthContext, raw_id: &str) -> Result<Agent, ApiError> {
        let id = parse_id(raw_id)?;
        let mut current = self.store.latest(&ctx.workspace, &id).await.map_err(map_store)?;
        if current.archived_at.is_none() {
            current.archived_at = Some(Timestamp::now());
            current.updated_at = Timestamp::now();
            self.store.replace_latest(&ctx.workspace, current.clone()).await.map_err(map_store)?;
        }
        Ok(current)
    }
}

fn parse_id(raw: &str) -> Result<AgentId, ApiError> {
    AgentId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("agent not found"),
        StoreError::Conflict => ApiError::invalid_request(CONCURRENT),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
