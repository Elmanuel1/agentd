//! Multiagent configuration as stored and served: every agent reference
//! resolved to a concrete `{type:"agent", id, version}`.

use serde::{Deserialize, Serialize};

use super::AgentRef;

/// A coordinator roster member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RosterMember {
    /// A stored agent at a concrete version.
    Agent(AgentRef),
    /// A model the coordinator may consult.
    Advisor(AdvisorMember),
}

/// `{"type":"advisor","model":"…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "advisor")]
pub struct AdvisorMember {
    /// The advisor model id.
    pub model: String,
}

/// Whether the primary thread can consult an advisor model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Advisor {
    /// An advisor model is available.
    Enabled {
        /// The advisor model id.
        model: String,
    },
    /// No advisor (the default).
    #[default]
    Disabled,
}

/// An on/off switch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Toggle {
    /// On (the default).
    #[default]
    Enabled,
    /// Off.
    Disabled,
}

/// Session threads or workflow runs: which agents they may run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Members {
    /// Allowed.
    Enabled {
        /// Whether inline (unsaved) agents may be defined.
        inline_agents: Toggle,
        /// Saved agents that may be run.
        predefined_agents: Vec<AgentRef>,
    },
    /// Not allowed.
    Disabled,
}

impl Default for Members {
    fn default() -> Self {
        Self::Enabled { inline_agents: Toggle::Enabled, predefined_agents: Vec::new() }
    }
}

/// An agent's multiagent configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Multiagent {
    /// This agent may delegate to a roster.
    #[serde(rename = "coordinator")]
    Coordinator {
        /// 1–20 roster members.
        agents: Vec<RosterMember>,
    },
    /// Advisor, subagents and workflows, each enabled or disabled on its own.
    #[serde(rename = "multiagent_20261001")]
    V20261001 {
        /// Advisor.
        advisor: Advisor,
        /// Session threads.
        subagents: Members,
        /// Workflow runs.
        workflows: Members,
    },
}
