//! Dream + DreamRun types (agentd-original resource — wire shape follows the
//! house conventions of the imitated API).

use serde::{Deserialize, Serialize};
use shared::schedule::CronKind;
use shared::{Id, Metadata, Resource, Timestamp};

/// Marker for `dream_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct DreamResource_;
impl Resource for DreamResource_ {
    const PREFIX: &'static str = "dream";
    const LABEL: &'static str = "dream";
}
/// `Id<DreamResource_>` alias.
pub type DreamId = Id<DreamResource_>;

/// Marker for `dmrn_…` run ids.
#[derive(Debug, Clone, Copy)]
pub struct DreamRunResource_;
impl Resource for DreamRunResource_ {
    const PREFIX: &'static str = "dmrn";
    const LABEL: &'static str = "dream run";
}
/// `Id<DreamRunResource_>` alias.
pub type DreamRunId = Id<DreamRunResource_>;

/// Constant `"type":"dream"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamTag {
    /// Always `dream`.
    Dream,
}

/// Dream status (same lifecycle family as deployments).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamStatus {
    /// Firing on schedule.
    Active,
    /// Suppressed.
    Paused,
    /// Terminal.
    Archived,
}

/// The dream resource: reflect over a memory store on a cadence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dream {
    /// Created.
    pub created_at: Timestamp,
    /// `dream_…`.
    pub id: DreamId,
    /// The memory store this dream curates.
    pub memory_store_id: String,
    /// Metadata.
    pub metadata: Metadata,
    /// Name.
    pub name: String,
    /// Cron schedule.
    pub schedule: DreamSchedule,
    /// Status.
    pub status: DreamStatus,
    /// Always `"dream"`.
    #[serde(rename = "type")]
    pub tag: DreamTag,
    /// Updated.
    pub updated_at: Timestamp,
}

/// Cron schedule block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamSchedule {
    /// Always `"cron"`.
    #[serde(rename = "type")]
    pub kind: CronKind,
    /// Expression.
    pub expression: String,
    /// Timezone.
    pub timezone: String,
}

/// A candidate memory proposed by a dream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Target path in the memory store.
    pub path: String,
    /// Proposed content.
    pub content: String,
    /// What supports this candidate (session/outcome/event ids).
    pub provenance: Vec<String>,
    /// Proposer confidence 0.0–1.0.
    pub confidence: f64,
}

/// The review gate's verdict on one candidate — the three lenses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// Supported by the cited provenance?
    pub evidence_ok: bool,
    /// Consistent with (or a justified supersession of) live memory?
    pub conflict_ok: bool,
    /// Not a duplicate of what's already promoted?
    pub novelty_ok: bool,
    /// Reviewer reasoning.
    pub reasoning: String,
}

impl Verdict {
    /// Promotion requires all three lenses to pass.
    pub fn promoted(&self) -> bool {
        self.evidence_ok && self.conflict_ok && self.novelty_ok
    }
}

/// The journal of one candidate's fate within a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateOutcome {
    /// The candidate.
    pub candidate: Candidate,
    /// The gate's verdict.
    pub verdict: Verdict,
    /// The memory version written on promotion (null on rejection).
    pub memory_version_id: Option<String>,
}

/// Constant `"type":"dream_run"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamRunTag {
    /// Always `dream_run`.
    DreamRun,
}

/// One firing of a dream — the promotion audit record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DreamRun {
    /// The per-candidate journal.
    pub candidates: Vec<CandidateOutcome>,
    /// Created.
    pub created_at: Timestamp,
    /// Parent dream.
    pub dream_id: DreamId,
    /// `dmrn_…`.
    pub id: DreamRunId,
    /// Candidates promoted.
    pub promoted_count: u32,
    /// Candidates rejected.
    pub rejected_count: u32,
    /// Always `"dream_run"`.
    #[serde(rename = "type")]
    pub tag: DreamRunTag,
}

/// `POST /v1/dreams` body.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateDreamRequest {
    /// Required.
    pub name: Option<String>,
    /// Required — the curated store.
    pub memory_store_id: Option<String>,
    /// Required.
    pub schedule: Option<DreamSchedule>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
}
