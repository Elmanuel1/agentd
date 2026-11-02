use serde::{Deserialize, Serialize};

/// Multiagent roster entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RosterEntry {
    /// Bare agent id string → latest version.
    Shorthand(String),
    /// Tagged reference.
    Tagged(TaggedRosterEntry),
}

/// Tagged roster entry forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaggedRosterEntry {
    /// A stored agent, optionally pinned.
    Agent {
        /// Agent id.
        id: String,
        /// Pinned version.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u64>,
    },
    /// The coordinator itself.
    #[serde(rename = "self")]
    SelfEntry {},
}

/// Coordinator config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Multiagent {
    /// This agent may delegate to a roster.
    Coordinator {
        /// 1–20 roster entries.
        agents: Vec<RosterEntry>,
    },
}
