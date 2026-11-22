//! A coordinator's tools for its roster, as live gives them (probed
//! 2026-11-21): `create_agent` starts a roster agent on a task and returns
//! its thread id, `send_to_agent` sends one of those threads a follow-up,
//! and `list_agents` lists the threads started and the agents it may start.
//! The descriptions are live's, word for word.

use agents::AgentSnapshot;
use serde_json::json;
use sessions::types::ThreadId;

use crate::model::ModelTool;

/// Starts a roster agent on a task.
pub const CREATE_AGENT: &str = "create_agent";
/// Sends a follow-up to a thread the coordinator started.
pub const SEND_TO_AGENT: &str = "send_to_agent";
/// Lists the threads started and the roster.
pub const LIST_AGENTS: &str = "list_agents";

/// The reminder a result carries: the reply comes in a later turn.
const CREATED: &str = "The agent's response will arrive in a future turn as a new message. End your turn once you have spawned what you need.";
const SENT: &str =
    "The agent's response will arrive in a future turn as a new message. End your turn after you send.";

/// The three tools, `create_agent` listing `roster` as the agents it may start.
pub fn tools(roster: &[&AgentSnapshot]) -> Vec<ModelTool> {
    let listed: String = roster
        .iter()
        .map(|a| format!("  - {}: {}\n", a.config.name, a.config.description.as_deref().unwrap_or("")))
        .collect();
    let string = |description: &str| json!({"type": "string", "description": description});
    vec![
        ModelTool {
            name: CREATE_AGENT.into(),
            description: include_str!("delegation/create_agent.md").replace("{roster}", &listed),
            input_schema: json!({"type": "object", "properties": {
                "agent_name": string("Which subagent type to instantiate. Must be one of the names in the roster above."),
                "task": string("Complete, self-contained instructions. The agent has no access to your conversation history.")},
                "required": ["agent_name", "task"]}),
        },
        ModelTool {
            name: SEND_TO_AGENT.into(),
            description: include_str!("delegation/send_to_agent.md").into(),
            input_schema: json!({"type": "object", "properties": {
                "thread_id": string("The sthr_… thread_id from create_agent's result or the notification header. Addresses the specific instance, not the agent type."),
                "message": string("Your reply. The agent sees this as its next message.")},
                "required": ["thread_id", "message"]}),
        },
        ModelTool {
            name: LIST_AGENTS.into(),
            description: include_str!("delegation/list_agents.md").into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
    ]
}

/// `create_agent`'s result for the thread it opened.
pub fn created(thread: &ThreadId, agent: &str) -> String {
    json!({"thread_id": thread, "agent_name": agent, "note": CREATED}).to_string()
}

/// `send_to_agent`'s result for the thread it sent to.
pub fn sent(thread: &ThreadId) -> String {
    json!({"delivered": true, "thread_id": thread, "queued": false, "note": SENT}).to_string()
}

/// `list_agents`'s result: each thread started, with whether it is at work,
/// then the agents that may be started.
pub fn listing(threads: &[(ThreadId, String, bool)], roster: &[&AgentSnapshot]) -> String {
    let agents: Vec<_> = threads
        .iter()
        .map(|(id, name, busy)| {
            let status = if *busy { "RUNNING" } else { "IDLE" };
            json!({"thread_id": id, "agent_name": name, "status": status, "parked": false, "park_reason": null})
        })
        .collect();
    let available: Vec<_> = roster
        .iter()
        .map(|a| json!({"name": a.config.name, "description": a.config.description.as_deref().unwrap_or("")}))
        .collect();
    json!({"agents": agents, "available": available}).to_string()
}

/// A delegate's message as its coordinator reads it: tagged, with a header
/// carrying the thread to answer.
pub fn notification(agent: &str, thread: &ThreadId, text: &str) -> String {
    format!(
        "<agent-notification thread_id=\"{thread}\" agent_name=\"{agent}\">\n{text}\n</agent-notification>"
    )
}
