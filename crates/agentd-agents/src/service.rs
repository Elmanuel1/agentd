//! Agent business logic: validation (exact live error strings),
//! canonicalization, integer versioning, and the archive lifecycle.

use std::sync::Arc;

use agentd_core::limits::{
    required, validate_array_max, validate_model_supported, validate_string_len, values,
    ModelCatalog,
};
use agentd_core::metadata::MetadataPolicy;
use agentd_core::{ApiError, AuthContext, Timestamp};

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
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Agent, ApiError> {
        let id = parse_id(raw_id)?;
        self.store
            .latest(&ctx.workspace, &id)
            .await
            .map_err(map_store)
    }

    /// `GET /v1/agents` — latest version of each agent.
    pub async fn list(&self, ctx: &AuthContext) -> Result<Vec<Agent>, ApiError> {
        self.store.list(&ctx.workspace).await.map_err(map_store)
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
        if let Some(v) = req.version {
            if v != current.version {
                return Err(ApiError::conflict_invalid_request(format!(
                    "version mismatch: expected {}, got {v}",
                    current.version
                )));
            }
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
        StoreError::Backend(m) => ApiError::internal(m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryAgentsStore;
    use crate::types::{ModelInput, SkillRef, Tool};
    use agentd_core::{OrgId, WorkspaceId};

    fn svc() -> AgentsService {
        AgentsService::new(
            Arc::new(InMemoryAgentsStore::new()),
            ModelCatalog::default_catalog(),
        )
    }

    fn ctx() -> AuthContext {
        AuthContext {
            org: OrgId("org_test".into()),
            workspace: WorkspaceId("wrkspc_01test".into()),
            api_key_id: "apikey_01test".into(),
        }
    }

    fn ctx2() -> AuthContext {
        AuthContext {
            org: OrgId("org_test".into()),
            workspace: WorkspaceId("wrkspc_02other".into()),
            api_key_id: "apikey_02test".into(),
        }
    }

    fn minimal() -> CreateAgentRequest {
        CreateAgentRequest {
            name: Some("compat-probe-agent".into()),
            model: Some(ModelInput::Shorthand("claude-sonnet-4-6".into())),
            system: Some("probe".into()),
            tools: serde_json::from_str(r#"[{"type":"agent_toolset_20260401"}]"#)
                .unwrap_or_default(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn create_canonicalizes_like_live_fixture() {
        let s = svc();
        let a = s.create(&ctx(), minimal()).await.unwrap();
        let v = serde_json::to_value(&a).unwrap();
        // Live fixture shape checks.
        assert_eq!(v["type"], "agent");
        assert_eq!(v["version"], 1);
        assert_eq!(v["model"], serde_json::json!({"id":"claude-sonnet-4-6","speed":"standard"}));
        assert_eq!(v["archived_at"], serde_json::Value::Null);
        assert_eq!(v["description"], serde_json::Value::Null);
        assert_eq!(v["mcp_servers"], serde_json::json!([]));
        assert_eq!(v["skills"], serde_json::json!([]));
        assert_eq!(v["multiagent"], serde_json::Value::Null);
        assert_eq!(v["metadata"], serde_json::json!({}));
        assert_eq!(v["system"], "probe");
        assert_eq!(v["tools"][0]["default_config"]["enabled"], true);
        assert!(a.id.as_str().starts_with("agent_01"));
    }

    #[tokio::test]
    async fn missing_model_exact_live_error() {
        let s = svc();
        let mut r = minimal();
        r.model = None;
        let e = s.create(&ctx(), r).await.unwrap_err();
        assert_eq!(e.message, "model: Field required");
        assert_eq!(e.status, 400);
    }

    #[tokio::test]
    async fn unsupported_model_exact_live_error() {
        let s = svc();
        let mut r = minimal();
        r.model = Some(ModelInput::Shorthand("gpt-4".into()));
        let e = s.create(&ctx(), r).await.unwrap_err();
        assert_eq!(e.message, "`model.id`: model \"gpt-4\": model is not supported");
    }

    #[tokio::test]
    async fn name_limits_exact_live_errors() {
        let s = svc();
        let mut r = minimal();
        r.name = Some("a".repeat(257));
        assert_eq!(
            s.create(&ctx(), r).await.unwrap_err().message,
            "name: maximum string length is 256"
        );
        let mut r = minimal();
        r.name = Some(String::new());
        assert_eq!(
            s.create(&ctx(), r).await.unwrap_err().message,
            "name: minimum string length is 1"
        );
    }

    #[tokio::test]
    async fn duplicate_skill_exact_live_error() {
        let s = svc();
        let mut r = minimal();
        r.skills = vec![
            SkillRef::Anthropic { skill_id: "pdf".into() },
            SkillRef::Anthropic { skill_id: "pdf".into() },
        ];
        let e = s.create(&ctx(), r).await.unwrap_err();
        assert_eq!(
            e.message,
            "Agent has invalid configuration: duplicate skill_id \"pdf\""
        );
    }

    #[tokio::test]
    async fn tools_cap_exact_live_error() {
        let s = svc();
        let mut r = minimal();
        let tool: Tool = serde_json::from_str(r#"{"type":"agent_toolset_20260401"}"#).unwrap();
        r.tools = vec![tool; 129];
        assert_eq!(
            s.create(&ctx(), r).await.unwrap_err().message,
            "tools: value must contain no more than 128 item(s)"
        );
    }

    #[tokio::test]
    async fn metadata_cap_applies_to_agents() {
        let s = svc();
        let mut r = minimal();
        r.metadata = agentd_core::Metadata(
            (0..17).map(|i| (format!("k{i}"), "v".into())).collect(),
        );
        let e = s.create(&ctx(), r).await.unwrap_err();
        assert_eq!(
            e.message,
            "invalid metadata: metadata map would have 17 entries, exceeding the maximum of 16"
        );
    }

    #[tokio::test]
    async fn get_malformed_id_is_400_invalid_agent_id() {
        // Live-verified: 400 "Invalid agent ID." — not 404.
        let s = svc();
        let e = s.get(&ctx(), "agent_00bogus").await.unwrap_err();
        assert_eq!(e.status, 400);
        assert_eq!(e.message, "Invalid agent ID.");
    }

    #[tokio::test]
    async fn get_missing_wellformed_id_is_404() {
        let s = svc();
        let ghost = AgentId::generate();
        let e = s.get(&ctx(), ghost.as_str()).await.unwrap_err();
        assert_eq!(e.status, 404);
    }

    #[tokio::test]
    async fn update_mints_next_integer_version() {
        let s = svc();
        let a = s.create(&ctx(), minimal()).await.unwrap();
        let updated = s
            .update(
                &ctx(),
                a.id.as_str(),
                UpdateAgentRequest {
                    system: Some("You are a helpful coding agent. Always write tests.".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.id, a.id);
        assert_eq!(
            updated.system.as_deref(),
            Some("You are a helpful coding agent. Always write tests.")
        );
        // Untouched fields carry over.
        assert_eq!(updated.name, a.name);
        // History preserved.
        let versions = s.versions(&ctx(), a.id.as_str()).await.unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].system.as_deref(), Some("probe"));
    }

    #[tokio::test]
    async fn update_with_stale_version_conflicts() {
        let s = svc();
        let a = s.create(&ctx(), minimal()).await.unwrap();
        s.update(&ctx(), a.id.as_str(), UpdateAgentRequest::default())
            .await
            .unwrap(); // now v2
        let e = s
            .update(
                &ctx(),
                a.id.as_str(),
                UpdateAgentRequest {
                    version: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(e.status, 409);
    }

    #[tokio::test]
    async fn archive_is_permanent_and_blocks_updates() {
        let s = svc();
        let a = s.create(&ctx(), minimal()).await.unwrap();
        let archived = s.archive(&ctx(), a.id.as_str()).await.unwrap();
        assert!(archived.archived_at.is_some());
        // Idempotent: second archive keeps the original stamp.
        let again = s.archive(&ctx(), a.id.as_str()).await.unwrap();
        assert_eq!(again.archived_at, archived.archived_at);
        // Updates now conflict.
        let e = s
            .update(&ctx(), a.id.as_str(), UpdateAgentRequest::default())
            .await
            .unwrap_err();
        assert_eq!(e.status, 409);
    }

    #[tokio::test]
    async fn workspace_isolation_end_to_end() {
        let s = svc();
        let a = s.create(&ctx(), minimal()).await.unwrap();
        // Other workspace: invisible.
        let e = s.get(&ctx2(), a.id.as_str()).await.unwrap_err();
        assert_eq!(e.status, 404);
        assert!(s.list(&ctx2()).await.unwrap().is_empty());
        assert_eq!(s.list(&ctx()).await.unwrap().len(), 1);
    }
}
