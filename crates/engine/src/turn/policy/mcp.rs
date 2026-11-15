//! MCP tools in the toolbox: each offered under its server's `mcp_toolset`,
//! with that toolset's policy.

use agents::types::Tool;
use agents::AgentSnapshot;

use super::{from_policy, Disposition, Toolbox};
use crate::mcp::McpTool;
use crate::model::ModelTool;

impl Toolbox {
    /// Offer each of `tools` that its server's `mcp_toolset` enables and
    /// whose name no earlier tool took.
    pub fn with_mcp(&mut self, agent: &AgentSnapshot, tools: &[McpTool]) {
        for t in tools {
            let toolset = agent.config.tools.iter().find_map(|x| match x {
                Tool::McpToolset { configs, default_config, mcp_server_name }
                    if *mcp_server_name == t.server =>
                {
                    Some((configs, default_config))
                }
                _ => None,
            });
            let Some((configs, default)) = toolset else { continue };
            let config = configs.iter().find(|c| c.name == t.name);
            let enabled = config.map_or(default.enabled, |c| c.enabled);
            if !enabled || self.disposition(&t.name).is_some() {
                continue;
            }
            let (ask, evaluation) =
                match from_policy(config.map_or(default.permission_policy, |c| c.permission_policy)) {
                    Disposition::Ask { evaluation } => (true, Some(evaluation)),
                    Disposition::Allow { evaluation } => (false, evaluation),
                    _ => continue,
                };
            let tool = ModelTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            };
            self.add(tool, Disposition::Mcp { server: t.server.clone(), ask, evaluation });
        }
    }
}
