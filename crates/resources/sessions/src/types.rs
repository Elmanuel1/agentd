//! Session wire types — session fixture captured live.

use agents::AgentSnapshot;
use serde::{Deserialize, Serialize};
use shared::{Id, Metadata, Resource, Timestamp, Usage};

mod outcomes;
mod resources;

pub use outcomes::{OutcomeEvaluation, OutcomeId, OutcomeResource_, OutcomeResult};
pub use resources::{
    Checkout, MemoryAccess, SessionResource, SessionResourceEntry_, SessionResourceId, SessionResourceInput,
};

/// Marker for `sesn_…` ids (live-verified prefix — docs wrongly say
/// `session_`).
#[derive(Debug, Clone, Copy)]
pub struct SessionResource_;
impl Resource for SessionResource_ {
    const PREFIX: &'static str = "sesn";
    const LABEL: &'static str = "session";
}
/// `Id<SessionResource_>` alias.
pub type SessionId = Id<SessionResource_>;

/// Constant `"type":"session"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTag {
    /// Always `session`.
    Session,
}

/// Session lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Awaiting input (or blocked on a client action — see stop_reason).
    Idle,
    /// Actively working.
    Running,
    /// Re-scheduling after a retryable error.
    Rescheduling,
    /// Terminal, unusable.
    Terminated,
}

/// Wire stats block: `{"active_seconds":0,"duration_seconds":0}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// Seconds the agent was actively working.
    pub active_seconds: u64,
    /// Wall-clock session duration.
    pub duration_seconds: u64,
}

/// Full wire session object (field order per live fixture).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Frozen agent snapshot (full config, live-verified embedding).
    pub agent: AgentSnapshot,
    /// Archive stamp.
    pub archived_at: Option<Timestamp>,
    /// Created.
    pub created_at: Timestamp,
    /// The environment.
    pub environment_id: String,
    /// `sesn_…`.
    pub id: SessionId,
    /// User metadata (NO entry cap — live divergence from docs).
    pub metadata: Metadata,
    /// Outcome evaluations.
    pub outcome_evaluations: Vec<OutcomeEvaluation>,
    /// Attached resources.
    pub resources: Vec<SessionResource>,
    /// Stats.
    pub stats: Stats,
    /// Status.
    pub status: SessionStatus,
    /// Title (nullable).
    pub title: Option<String>,
    /// Always `"session"`.
    #[serde(rename = "type")]
    pub tag: SessionTag,
    /// Updated.
    pub updated_at: Timestamp,
    /// Token usage.
    pub usage: Usage,
    /// Attached vaults.
    pub vault_ids: Vec<String>,
}

/// Agent reference accepted on session create: bare id string or
/// `{type:"agent", id, version?}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AgentRefInput {
    /// `"agent_abc"` → latest version.
    Shorthand(String),
    /// Tagged form.
    Tagged {
        /// Must be `"agent"`.
        #[serde(rename = "type")]
        tag: String,
        /// Agent id.
        id: String,
        /// Pinned version (latest when absent).
        #[serde(default)]
        version: Option<u64>,
    },
}

impl AgentRefInput {
    /// The referenced agent id.
    pub fn id(&self) -> &str {
        match self {
            Self::Shorthand(id) => id,
            Self::Tagged { id, .. } => id,
        }
    }

    /// The pinned version, if any.
    pub fn version(&self) -> Option<u64> {
        match self {
            Self::Shorthand(_) => None,
            Self::Tagged { version, .. } => *version,
        }
    }
}

/// `POST /v1/sessions` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionRequest {
    /// Required agent reference.
    pub agent: Option<AgentRefInput>,
    /// Required environment.
    pub environment_id: Option<String>,
    /// Optional title.
    pub title: Option<String>,
    /// Resources.
    #[serde(default)]
    pub resources: Vec<SessionResourceInput>,
    /// Vaults.
    #[serde(default)]
    pub vault_ids: Vec<String>,
    /// Metadata (no entry cap — live divergence).
    #[serde(default)]
    pub metadata: Metadata,
}

/// `POST /v1/sessions/{id}` body — only `title`/`metadata` here (agent-config
/// overrides are a later surface).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSessionRequest {
    /// Replace title.
    pub title: Option<String>,
    /// Replace metadata.
    pub metadata: Option<Metadata>,
}

#[cfg(test)]
mod tests;
