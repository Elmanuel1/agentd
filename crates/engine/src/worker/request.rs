//! The model call a step makes: the thread's tools, its MCP servers' tools
//! among them, the system prompt with the agent's skills, and the
//! conversation.

use agents::AgentSnapshot;
use agents::types::SkillRef;
use sessions::types::Session;
use shared::WorkspaceId;

use super::Loop;
use super::team::{Thread, roster};
use crate::mcp::McpTool;
use crate::model::{Message, ModelRequest};
use crate::turn::policy::Toolbox;
use crate::turn::prompt::{SkillInfo, system_prompt};

impl Loop {
    /// The request `thread` sends with `messages`, and the tools it offers.
    pub(super) async fn request(
        &self,
        ws: &WorkspaceId,
        session: &Session,
        thread: &Thread,
        messages: Vec<Message>,
        mcp: &[McpTool],
    ) -> (Toolbox, ModelRequest) {
        let agent = &thread.agent;
        let roster = roster(session);
        let coordinator = thread.primary && !roster.is_empty();
        let delegates = match coordinator {
            true => roster.as_slice(),
            false => &[],
        };
        let tools = match &thread.grading {
            Some(_) => Toolbox::grader(),
            None => {
                let mut tools = Toolbox::for_agent(agent, delegates);
                tools.with_mcp(agent, mcp);
                tools
            }
        };
        let effort = agent
            .config
            .model
            .effort
            .as_ref()
            .and_then(|e| serde_json::to_value(e).ok()?.as_str().map(str::to_owned));
        let skills = self.skills(ws, agent).await;
        let request = ModelRequest {
            model: agent.config.model.id.clone(),
            agent: agent.config.name.clone(),
            system: system_prompt(agent, &skills),
            messages,
            tools: tools.tools.clone(),
            max_tokens: self.models.max_tokens,
            effort,
            compact: None,
        };
        (tools, request)
    }

    /// The skills `agent` names, as its prompt lists them; one that cannot be
    /// read (deleted since, or an Anthropic skill agentd does not have) is
    /// left out.
    async fn skills(&self, ws: &WorkspaceId, agent: &AgentSnapshot) -> Vec<SkillInfo> {
        let refs: Vec<(&str, &str)> = agent
            .config
            .skills
            .iter()
            .map(|skill| {
                let (SkillRef::Anthropic { version, .. } | SkillRef::Custom { version, .. }) = skill;
                (skill.skill_id(), version.as_str())
            })
            .collect();
        if refs.is_empty() {
            return Vec::new();
        }
        match self.stores.skills.listed(ws, &refs).await {
            Ok(versions) => versions
                .into_iter()
                .map(|v| SkillInfo { name: v.directory, description: v.description })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e.message, "loop: the agent's skills could not be read; its prompt lists none");
                Vec::new()
            }
        }
    }
}
