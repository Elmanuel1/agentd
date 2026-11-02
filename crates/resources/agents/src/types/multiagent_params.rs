//! Multiagent configuration as a request sends it. Agent references may be a
//! bare id, `{type:"agent"}` without a version, or `{type:"self"}`; the
//! service resolves them before storing.

use serde::Deserialize;
use shared::Patch;

use super::multiagent::{Advisor, Toggle};

/// A reference to a saved agent.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AgentRefParams {
    /// A bare agent id: its latest version.
    Id(String),
    /// A tagged reference.
    Tagged(TaggedRefParams),
}

/// Tagged reference forms.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaggedRefParams {
    /// A saved agent, optionally pinned.
    Agent {
        /// Agent id.
        id: String,
        /// Pinned version; latest when left out.
        #[serde(default)]
        version: Option<u64>,
    },
    /// The agent being configured.
    #[serde(rename = "self")]
    SelfRef {},
    /// A model the coordinator may consult (coordinator rosters only).
    Advisor {
        /// The advisor model id.
        model: String,
    },
}

/// Session threads or workflow runs, as sent.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MembersParams {
    /// Allowed.
    Enabled {
        /// Inline agents; enabled when left out or `null`.
        #[serde(default)]
        inline_agents: Option<Toggle>,
        /// Saved agents; when sent, replaces the stored list.
        #[serde(default)]
        predefined_agents: Option<Vec<AgentRefParams>>,
    },
    /// Not allowed.
    Disabled {},
}

/// A request's `multiagent`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum MultiagentParams {
    /// A coordinator roster.
    #[serde(rename = "coordinator")]
    Coordinator {
        /// 1–20 roster entries.
        agents: Vec<AgentRefParams>,
    },
    /// Advisor, subagents and workflows. A member left out keeps its stored
    /// value on update; `null` takes its default.
    #[serde(rename = "multiagent_20261001")]
    V20261001 {
        /// Advisor.
        #[serde(default)]
        advisor: Patch<Advisor>,
        /// Session threads.
        #[serde(default)]
        subagents: Patch<MembersParams>,
        /// Workflow runs.
        #[serde(default)]
        workflows: Patch<MembersParams>,
    },
}
