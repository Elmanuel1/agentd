//! The model call a step makes: the thread's tools, its MCP servers' tools
//! among them, the system prompt with its roster, and the conversation.

use sessions::types::Session;

use super::Loop;
use super::team::{Thread, roster};
use crate::mcp::McpTool;
use crate::model::{Message, ModelRequest};
use crate::turn::policy::Toolbox;
use crate::turn::prompt::system_prompt;

impl Loop {
    /// The request `thread` sends with `messages`, and the tools it offers.
    pub(super) fn request(
        &self,
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
            Some(_) => Toolbox::read_only(),
            None => {
                let mut tools = Toolbox::for_agent(agent, coordinator);
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
        let request = ModelRequest {
            model: agent.config.model.id.clone(),
            agent: agent.config.name.clone(),
            system: system_prompt(agent, &[], delegates),
            messages,
            tools: tools.tools.clone(),
            max_tokens: self.models.max_tokens,
            effort,
            compact: None,
        };
        (tools, request)
    }
}
