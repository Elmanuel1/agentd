//! Agent configuration rules, on the request types that carry the data: the
//! exact live error strings, in the live order.

use shared::limits::{
    required, validate_array_max, validate_model_supported, validate_string_len, values, ModelCatalog,
};
use shared::metadata::MetadataPolicy;
use shared::{ApiError, Metadata, Patch};

use super::{
    Agent, AgentConfig, CreateAgentRequest, McpServer, ModelInput, ModelSpec, MultiagentParams, SkillParams,
    SkillRef, ToolParams, UpdateAgentRequest,
};

/// A validated configuration whose `multiagent` the service still resolves
/// (its agent references are looked up in the store).
#[derive(Debug, Clone)]
pub struct ConfigDraft {
    /// The configuration; `multiagent` is the stored value until resolved.
    pub config: AgentConfig,
    /// The metadata after the request.
    pub metadata: Metadata,
    /// The multiagent change to resolve.
    pub multiagent: Patch<MultiagentParams>,
}

/// The fields every agent write validates.
struct Fields {
    name: Option<String>,
    model: Option<ModelInput>,
    description: Option<String>,
    system: Option<String>,
    tools: Vec<ToolParams>,
    mcp_servers: Vec<McpServer>,
    skills: Vec<SkillParams>,
    metadata: Metadata,
}

impl CreateAgentRequest {
    /// Validate and resolve a new agent's configuration.
    pub fn into_draft(self, catalog: &ModelCatalog) -> Result<ConfigDraft, ApiError> {
        let fields = Fields {
            name: self.name,
            model: self.model,
            description: self.description,
            system: self.system,
            tools: self.tools,
            mcp_servers: self.mcp_servers,
            skills: self.skills,
            metadata: self.metadata,
        };
        let identity = self.execution_identity.unwrap_or_default();
        let multiagent = self.multiagent.map_or(Patch::Keep, Patch::Set);
        fields.into_draft(catalog, identity, None, multiagent)
    }
}

impl UpdateAgentRequest {
    /// Validate and resolve `current` after this update.
    pub fn apply_to(self, current: &Agent, catalog: &ModelCatalog) -> Result<ConfigDraft, ApiError> {
        let c = &current.config;
        // Stored tools and skills are already resolved; they pass through as is.
        let (tools, keep_tools) = match self.tools {
            Patch::Keep => (Vec::new(), true),
            Patch::Clear => (Vec::new(), false),
            Patch::Set(t) => (t, false),
        };
        let (skills, keep_skills) = match self.skills {
            Patch::Keep => (Vec::new(), true),
            Patch::Clear => (Vec::new(), false),
            Patch::Set(s) => (s, false),
        };
        let fields = Fields {
            name: Some(self.name.unwrap_or_else(|| c.name.clone())),
            model: Some(self.model.unwrap_or_else(|| stored_model(&c.model))),
            description: self.description.apply_to_option(c.description.clone()),
            system: self.system.apply_to_option(c.system.clone()),
            tools,
            mcp_servers: self.mcp_servers.apply(c.mcp_servers.clone(), Vec::new),
            skills,
            metadata: current.metadata.clone().merged(self.metadata),
        };
        let identity = self.execution_identity.apply(c.execution_identity.clone(), Default::default);
        let mut draft = fields.into_draft(catalog, identity, Some(c.multiagent.clone()), self.multiagent)?;
        if keep_tools {
            draft.config.tools = c.tools.clone();
        }
        if keep_skills {
            draft.config.skills = c.skills.clone();
        }
        Ok(draft)
    }
}

fn stored_model(m: &ModelSpec) -> ModelInput {
    ModelInput::Full {
        id: m.id.clone(),
        speed: Some(m.speed),
        effort: m.effort.map(super::model::EffortInput::Object),
        inference_geo: m.inference_geo.clone(),
    }
}

impl Fields {
    fn into_draft(
        self,
        catalog: &ModelCatalog,
        execution_identity: super::ExecutionIdentity,
        stored_multiagent: Option<Option<super::Multiagent>>,
        multiagent: Patch<MultiagentParams>,
    ) -> Result<ConfigDraft, ApiError> {
        let name = required("name", self.name)?;
        validate_string_len("name", &name, values::NAME_MIN, values::NAME_MAX)?;
        if let Some(d) = &self.description {
            validate_string_len("description", d, 0, values::DESCRIPTION_MAX)?;
        }
        if let Some(s) = &self.system {
            validate_string_len("system", s, 0, values::SYSTEM_MAX)?;
        }
        validate_array_max("tools", self.tools.len(), values::TOOLS_MAX)?;
        validate_array_max("mcp_servers", self.mcp_servers.len(), values::MCP_SERVERS_MAX)?;
        let skills: Vec<SkillRef> = self.skills.into_iter().map(SkillParams::resolve).collect();
        // Live: duplicate skill_id → "Agent has invalid configuration: duplicate skill_id "pdf""
        let mut seen = std::collections::BTreeSet::new();
        if let Some(dup) = skills.iter().find(|s| !seen.insert(s.skill_id().to_owned())) {
            return Err(ApiError::invalid_request(format!(
                "Agent has invalid configuration: duplicate skill_id \"{}\"",
                dup.skill_id()
            )));
        }
        validate_array_max("skills", skills.len(), values::SKILLS_MAX)?;
        self.metadata.validate(MetadataPolicy::capped())?;
        let model = required("model", self.model)?.canonicalize();
        validate_model_supported(&model.id, catalog)?;
        let tools = self.tools.into_iter().map(ToolParams::resolve).collect::<Result<_, _>>()?;
        Ok(ConfigDraft {
            config: AgentConfig {
                name,
                model,
                description: self.description,
                system: self.system,
                tools,
                mcp_servers: self.mcp_servers,
                skills,
                multiagent: stored_multiagent.flatten(),
                execution_identity,
            },
            metadata: self.metadata,
            multiagent,
        })
    }
}
