//! Resolving a request's multiagent configuration: every agent reference to a
//! concrete version, and `multiagent_20261001` merged onto the stored one
//! level by level.

use shared::limits::validate_array_max;
use shared::{ApiError, AuthContext, Patch};

use super::{map_store, parse_id, AgentsService};
use crate::types::{
    AdvisorMember, AgentRef, AgentRefParams, Members, MembersParams, Multiagent, MultiagentParams,
    RosterMember, TaggedRefParams, Toggle,
};

/// The most roster members a coordinator may have.
const ROSTER_MAX: usize = 20;

impl AgentsService {
    /// The configuration after `patch`, given the `stored` one. `this` is the
    /// agent being written, which `{type:"self"}` names.
    pub(super) async fn resolve_multiagent(
        &self,
        ctx: &AuthContext,
        this: &AgentRef,
        stored: Option<&Multiagent>,
        patch: Patch<MultiagentParams>,
    ) -> Result<Option<Multiagent>, ApiError> {
        let params = match patch {
            Patch::Keep => return Ok(stored.cloned()),
            Patch::Clear => return Ok(None),
            Patch::Set(p) => p,
        };
        Ok(Some(match params {
            MultiagentParams::Coordinator { agents } => {
                if agents.is_empty() {
                    return Err(ApiError::invalid_request(
                        "multiagent.agents: List should have at least 1 item",
                    ));
                }
                validate_array_max("multiagent.agents", agents.len(), ROSTER_MAX)?;
                let mut roster = Vec::with_capacity(agents.len());
                for entry in agents {
                    roster.push(match entry {
                        AgentRefParams::Tagged(TaggedRefParams::Advisor { model }) => {
                            RosterMember::Advisor(AdvisorMember { model })
                        }
                        other => RosterMember::Agent(self.agent_ref(ctx, this, other).await?),
                    });
                }
                Multiagent::Coordinator { agents: roster }
            }
            MultiagentParams::V20261001 { advisor, subagents, workflows } => {
                let (s_advisor, s_subagents, s_workflows) = match stored {
                    Some(Multiagent::V20261001 { advisor, subagents, workflows }) => {
                        (Some(advisor), Some(subagents), Some(workflows))
                    }
                    _ => (None, None, None),
                };
                Multiagent::V20261001 {
                    advisor: advisor.apply(s_advisor.cloned().unwrap_or_default(), Default::default),
                    subagents: self.members(ctx, this, s_subagents, subagents).await?,
                    workflows: self.members(ctx, this, s_workflows, workflows).await?,
                }
            }
        }))
    }

    async fn members(
        &self,
        ctx: &AuthContext,
        this: &AgentRef,
        stored: Option<&Members>,
        patch: Patch<MembersParams>,
    ) -> Result<Members, ApiError> {
        let (inline, predefined) = match patch {
            Patch::Keep => return Ok(stored.cloned().unwrap_or_default()),
            Patch::Clear => return Ok(Members::default()),
            Patch::Set(MembersParams::Disabled {}) => return Ok(Members::Disabled),
            Patch::Set(MembersParams::Enabled { inline_agents, predefined_agents }) => {
                (inline_agents, predefined_agents)
            }
        };
        // The same type as stored merges onto it; any other starts from the defaults.
        let (base_inline, base_predefined) = match stored {
            Some(Members::Enabled { inline_agents, predefined_agents }) => {
                (*inline_agents, predefined_agents.clone())
            }
            _ => (Toggle::Enabled, Vec::new()),
        };
        let predefined_agents = match predefined {
            None => base_predefined,
            Some(list) => {
                let mut refs = Vec::with_capacity(list.len());
                for entry in list {
                    refs.push(self.agent_ref(ctx, this, entry).await?);
                }
                refs
            }
        };
        let inline_agents = inline.unwrap_or(base_inline);
        if inline_agents == Toggle::Disabled && predefined_agents.is_empty() {
            return Err(ApiError::invalid_request(
                "multiagent: predefined_agents must name at least one agent when inline_agents is disabled",
            ));
        }
        Ok(Members::Enabled { inline_agents, predefined_agents })
    }

    /// A reference at a concrete version: a bare id or an unpinned reference
    /// is the agent's latest version.
    async fn agent_ref(
        &self,
        ctx: &AuthContext,
        this: &AgentRef,
        entry: AgentRefParams,
    ) -> Result<AgentRef, ApiError> {
        let (raw, version) = match entry {
            AgentRefParams::Id(id) => (id, None),
            AgentRefParams::Tagged(TaggedRefParams::Agent { id, version }) => (id, version),
            AgentRefParams::Tagged(TaggedRefParams::SelfRef {}) => return Ok(this.clone()),
            AgentRefParams::Tagged(TaggedRefParams::Advisor { .. }) => {
                return Err(ApiError::invalid_request(
                    "multiagent: an advisor can only be a coordinator roster member",
                ))
            }
        };
        let id = parse_id(&raw)?;
        let agent = match version {
            Some(v) => self.store.version(&ctx.workspace, &id, v).await,
            None => self.store.latest(&ctx.workspace, &id).await,
        }
        .map_err(map_store)?;
        Ok(agent.reference())
    }
}
