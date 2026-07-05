//! Session wire types — session fixture captured live.

use agentd_agents::AgentSnapshot;
use agentd_core::{Id, Metadata, Resource, Timestamp, Usage};
use serde::{Deserialize, Serialize};

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

/// Marker for `sesrsc_…` session-resource ids.
#[derive(Debug, Clone, Copy)]
pub struct SessionResourceEntry_;
impl Resource for SessionResourceEntry_ {
    const PREFIX: &'static str = "sesrsc";
    const LABEL: &'static str = "session resource";
}
/// `Id<SessionResourceEntry_>` alias.
pub type SessionResourceId = Id<SessionResourceEntry_>;

/// Marker for `outc_…` outcome ids.
#[derive(Debug, Clone, Copy)]
pub struct OutcomeResource_;
impl Resource for OutcomeResource_ {
    const PREFIX: &'static str = "outc";
    const LABEL: &'static str = "outcome";
}
/// `Id<OutcomeResource_>` alias.
pub type OutcomeId = Id<OutcomeResource_>;

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

/// Repo checkout selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Checkout {
    /// A branch by name.
    Branch {
        /// Branch name.
        name: String,
    },
    /// A commit by sha.
    Commit {
        /// Commit sha.
        sha: String,
    },
}

/// Memory-store access mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAccess {
    /// Read/write (default).
    ReadWrite,
    /// Read-only mount.
    ReadOnly,
}

/// A session resource — file / repo / memory store. Cross-entity links are
/// by ID string (no crate dependency on files/memory).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionResource {
    /// An uploaded file mounted read-only.
    File {
        /// `sesrsc_…` (assigned at attach).
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<SessionResourceId>,
        /// The uploaded file (session-scoped copy).
        file_id: String,
        /// Absolute mount path.
        mount_path: String,
    },
    /// A GitHub repository cloned at startup.
    GithubRepository {
        /// `sesrsc_…`.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<SessionResourceId>,
        /// Repo URL.
        url: String,
        /// Clone path (defaults to /workspace/<repo>).
        #[serde(skip_serializing_if = "Option::is_none")]
        mount_path: Option<String>,
        /// Checkout selector.
        #[serde(skip_serializing_if = "Option::is_none")]
        checkout: Option<Checkout>,
        // NOTE: authorization_token is accepted on input but NEVER echoed —
        // it is stripped before this type is constructed (see requests).
    },
    /// A memory store mount (session-create only).
    MemoryStore {
        /// `sesrsc_…`.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<SessionResourceId>,
        /// The store.
        memory_store_id: String,
        /// Access mode.
        #[serde(skip_serializing_if = "Option::is_none")]
        access: Option<MemoryAccess>,
        /// Session-specific guidance (≤4096).
        #[serde(skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
    },
}

/// Outcome evaluation record surfaced on the session object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutcomeEvaluation {
    /// The outcome.
    pub outcome_id: OutcomeId,
    /// Result of the latest grader iteration.
    pub result: OutcomeResult,
    /// Grader explanation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// 0-indexed iteration.
    pub iteration: u32,
}

/// Grader verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeResult {
    /// Rubric met — terminal.
    Satisfied,
    /// Iterate again.
    NeedsRevision,
    /// Iteration cap reached — terminal.
    MaxIterationsReached,
    /// Rubric fundamentally mismatched — terminal.
    Failed,
    /// Interrupted mid-grade — terminal.
    Interrupted,
}

impl OutcomeResult {
    /// Whether this verdict ends the outcome loop.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::NeedsRevision)
    }
}

/// Full wire session object (field order per live fixture).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// Input resource shape — carries the write-only `authorization_token` for
/// repos, which is stripped before the wire type is built.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionResourceInput {
    /// File mount.
    File {
        /// Uploaded file id.
        file_id: String,
        /// Required absolute mount path.
        mount_path: Option<String>,
    },
    /// Repo mount.
    GithubRepository {
        /// Repo URL.
        url: String,
        /// Clone token (write-only — never echoed).
        #[allow(dead_code)]
        authorization_token: Option<String>,
        /// Clone path.
        mount_path: Option<String>,
        /// Checkout.
        checkout: Option<Checkout>,
    },
    /// Memory store mount (session-create only).
    MemoryStore {
        /// Store id.
        memory_store_id: String,
        /// Access.
        access: Option<MemoryAccess>,
        /// Instructions.
        instructions: Option<String>,
    },
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
mod tests {
    use super::*;

    const SESSION_FIXTURE: &str = include_str!("../../../fixtures/session.create.json");

    #[test]
    fn fixture_roundtrip_is_lossless() {
        let original: serde_json::Value = serde_json::from_str(SESSION_FIXTURE).unwrap();
        let s: Session = serde_json::from_str(SESSION_FIXTURE).unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap(), original);
    }

    #[test]
    fn agent_ref_input_forms() {
        let a: AgentRefInput = serde_json::from_str("\"agent_01XdReZgC7eHpg9b9Nvrwdcm\"").unwrap();
        assert_eq!(a.id(), "agent_01XdReZgC7eHpg9b9Nvrwdcm");
        assert_eq!(a.version(), None);
        let b: AgentRefInput = serde_json::from_str(
            r#"{"type":"agent","id":"agent_01XdReZgC7eHpg9b9Nvrwdcm","version":3}"#,
        )
        .unwrap();
        assert_eq!(b.version(), Some(3));
    }

    #[test]
    fn repo_resource_token_is_never_serialized() {
        let input: SessionResourceInput = serde_json::from_str(
            r#"{"type":"github_repository","url":"https://github.com/o/r","authorization_token":"ghp_secret"}"#,
        )
        .unwrap();
        // The wire type has no token field at all — constructing it drops it.
        let wire = match input {
            SessionResourceInput::GithubRepository { url, mount_path, checkout, .. } => {
                SessionResource::GithubRepository { id: None, url, mount_path, checkout }
            }
            _ => panic!("wrong variant"),
        };
        let s = serde_json::to_string(&wire).unwrap();
        assert!(!s.contains("ghp_secret"));
        assert!(!s.contains("authorization_token"));
    }

    #[test]
    fn outcome_result_terminality() {
        assert!(OutcomeResult::Satisfied.is_terminal());
        assert!(!OutcomeResult::NeedsRevision.is_terminal());
        assert!(OutcomeResult::MaxIterationsReached.is_terminal());
        assert!(OutcomeResult::Failed.is_terminal());
        assert!(OutcomeResult::Interrupted.is_terminal());
    }
}
