//! Agent business logic: validation (exact live error strings),
//! canonicalization, integer versioning, and the archive lifecycle.

use std::sync::Arc;

use agentd_core::limits::{
    required, validate_array_max, validate_model_supported, validate_string_len, values,
    ModelCatalog,
};
use agentd_core::metadata::MetadataPolicy;
use agentd_core::{ApiError, AuthContext, Pagination, Timestamp};

use crate::store::{AgentsStore, StoreError};
use crate::types::{
    Agent, AgentConfig, AgentId, AgentTag, CreateAgentRequest, UpdateAgentRequest,
};

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

impl AgentsService {
    /// Build with a store and model catalog.
    pub fn new(store: Arc<dyn AgentsStore>, catalog: ModelCatalog) -> Self {
        Self { store, catalog }
    }

    /// `POST /v1/agents`.
    pub async fn create(
        &self,
        ctx: &AuthContext,
        req: CreateAgentRequest,
    ) -> Result<Agent, ApiError> {
        let metadata = req.metadata.clone();
        let config = self.validate_config(req)?;
        let now = Timestamp::now();
        let agent = Agent {
            archived_at: None,
            created_at: now,
            description: config.description,
            id: AgentId::generate(),
            mcp_servers: config.mcp_servers,
            metadata,
            model: config.model,
            multiagent: config.multiagent,
            name: config.name,
            skills: config.skills,
            system: config.system,
            tools: config.tools,
            tag: AgentTag::Agent,
            updated_at: now,
            version: 1,
        };
        self.store
            .insert(&ctx.workspace, agent.clone())
            .await
            .map_err(map_store)?;
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

    /// `GET /v1/agents` — latest version of each agent.
    pub async fn list(&self, ctx: &AuthContext, page: &Pagination) -> Result<Vec<Agent>, ApiError> {
        self.store
            .list(&ctx.workspace, page)
            .await
            .map_err(map_store)
    }

    /// `GET /v1/agents/{id}/versions` — ascending.
    pub async fn versions(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vec<Agent>, ApiError> {
        let id = parse_id(raw_id)?;
        self.store
            .versions(&ctx.workspace, &id)
            .await
            .map_err(map_store)
    }

    /// `POST /v1/agents/{id}` — every update mints version latest+1.
    pub async fn update(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        req: UpdateAgentRequest,
    ) -> Result<Agent, ApiError> {
        let id = parse_id(raw_id)?;
        // Live: `version` is required (optimistic lock) — a missing one fails
        // body validation with "version: Field required".
        let expected = required("version", req.version)?;
        let current = self
            .store
            .latest(&ctx.workspace, &id)
            .await
            .map_err(map_store)?;
        if current.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request(
                "cannot update an archived agent",
            ));
        }
        if expected != current.version {
            // Live 400 message, verbatim.
            return Err(ApiError::invalid_request(
                "Concurrent modification detected. Please fetch the latest version and retry.",
            ));
        }
        // Merge: present fields replace, absent fields carry over.
        let merged = CreateAgentRequest {
            name: Some(req.name.unwrap_or_else(|| current.name.clone())),
            model: Some(match req.model {
                Some(m) => m,
                None => crate::types::ModelInput::Full {
                    id: current.model.id.clone(),
                    speed: Some(current.model.speed),
                },
            }),
            description: req.description.or_else(|| current.description.clone()),
            system: req.system.or_else(|| current.system.clone()),
            tools: req.tools.unwrap_or_else(|| current.tools.clone()),
            mcp_servers: req
                .mcp_servers
                .unwrap_or_else(|| current.mcp_servers.clone()),
            skills: req.skills.unwrap_or_else(|| current.skills.clone()),
            multiagent: req.multiagent.or_else(|| current.multiagent.clone()),
            metadata: req.metadata.unwrap_or_else(|| current.metadata.clone()),
        };
        let metadata = merged.metadata.clone();
        let config = self.validate_config(merged)?;
        let next = Agent {
            archived_at: None,
            created_at: current.created_at,
            description: config.description,
            id: current.id.clone(),
            mcp_servers: config.mcp_servers,
            metadata,
            model: config.model,
            multiagent: config.multiagent,
            name: config.name,
            skills: config.skills,
            system: config.system,
            tools: config.tools,
            tag: AgentTag::Agent,
            updated_at: Timestamp::now(),
            version: current.version + 1,
        };
        self.store
            .append_version(&ctx.workspace, next.clone())
            .await
            .map_err(map_store)?;
        Ok(next)
    }

    /// `POST /v1/agents/{id}/archive` — permanent, idempotent stamp.
    pub async fn archive(&self, ctx: &AuthContext, raw_id: &str) -> Result<Agent, ApiError> {
        let id = parse_id(raw_id)?;
        let mut current = self
            .store
            .latest(&ctx.workspace, &id)
            .await
            .map_err(map_store)?;
        if current.archived_at.is_none() {
            current.archived_at = Some(Timestamp::now());
            current.updated_at = Timestamp::now();
            self.store
                .replace_latest(&ctx.workspace, current.clone())
                .await
                .map_err(map_store)?;
        }
        Ok(current)
    }

    /// Shared validation + canonicalization with exact live error strings.
    fn validate_config(&self, req: CreateAgentRequest) -> Result<AgentConfig, ApiError> {
        let name = required("name", req.name)?;
        validate_string_len("name", &name, values::NAME_MIN, values::NAME_MAX)?;
        if let Some(d) = &req.description {
            validate_string_len("description", d, 0, values::DESCRIPTION_MAX)?;
        }
        if let Some(s) = &req.system {
            validate_string_len("system", s, 0, values::SYSTEM_MAX)?;
        }
        validate_array_max("tools", req.tools.len(), values::TOOLS_MAX)?;
        validate_array_max(
            "mcp_servers",
            req.mcp_servers.len(),
            values::MCP_SERVERS_MAX,
        )?;
        // Live: duplicate skill_id → "Agent has invalid configuration: duplicate skill_id "pdf""
        let mut seen = std::collections::BTreeSet::new();
        for s in &req.skills {
            if !seen.insert(s.skill_id().to_owned()) {
                return Err(ApiError::invalid_request(format!(
                    "Agent has invalid configuration: duplicate skill_id \"{}\"",
                    s.skill_id()
                )));
            }
        }
        validate_array_max("skills", req.skills.len(), values::SKILLS_MAX)?;
        req.metadata.validate(MetadataPolicy::capped())?;
        let model_input = required("model", req.model)?;
        let model = model_input.canonicalize();
        validate_model_supported(&model.id, &self.catalog)?;
        Ok(AgentConfig {
            name,
            model,
            description: req.description,
            system: req.system,
            tools: req.tools,
            mcp_servers: req.mcp_servers,
            skills: req.skills,
            multiagent: req.multiagent,
        })
    }
}

fn parse_id(raw: &str) -> Result<AgentId, ApiError> {
    AgentId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("agent not found"),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
