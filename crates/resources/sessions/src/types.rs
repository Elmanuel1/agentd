//! Session wire types — session fixture captured live.

use agents::AgentSnapshot;
use serde::{Deserialize, Serialize};
use shared::{Id, Metadata, Resource, Timestamp, Usage};

mod budget;
mod outcomes;
mod requests;
mod resources;
mod threads;

pub use budget::{Budget, MonetaryAmount, check_budgetable};
pub use requests::{AgentOverrides, AgentRefInput, AgentUpdate, CreateSessionRequest, UpdateSessionRequest};

pub use threads::{SessionThread, ThreadId, ThreadStats, ThreadStatus, ThreadTag, primary_thread_id};

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
    /// The spending limit, `null` when unset.
    #[serde(default)]
    pub budget: Option<Budget>,
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
    /// The deployment whose fire created the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_id: Option<String>,
}

/// Live's 404 for a session the caller cannot reach: missing, foreign or deleted.
pub fn not_found(id: &str) -> shared::ApiError {
    shared::ApiError::not_found(format!("Session not found: {id}"))
}

impl Session {
    /// The session as an environment key of `environment` sees it: any
    /// session of another environment does not exist for that key.
    pub fn seen_from(self, environment: Option<&str>) -> Result<Self, shared::ApiError> {
        match environment {
            Some(env) if env != self.environment_id => Err(not_found(self.id.as_str())),
            _ => Ok(self),
        }
    }

    /// Returns a `409` when the session is archived.
    pub fn writable(&self) -> Result<(), shared::ApiError> {
        if self.archived_at.is_some() {
            return Err(shared::ApiError::conflict_invalid_request("session is archived and read-only"));
        }
        Ok(())
    }

    /// Returns a `409` when the session is archived or terminated.
    pub fn takes_events(&self) -> Result<(), shared::ApiError> {
        self.writable()?;
        if self.status == SessionStatus::Terminated {
            return Err(shared::ApiError::conflict_invalid_request(
                "cannot send events to a terminated session",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
