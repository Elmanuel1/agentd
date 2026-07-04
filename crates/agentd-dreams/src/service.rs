//! The dream loop: propose → review gate → promote (write version) or drop.
//!
//! The gate GATES VERSION CREATION — a rejected candidate writes nothing and
//! `latest` never moves. A promoted candidate writes a version through the
//! memory service (create at a fresh path, update at an existing one), so
//! the next session that resolves `latest` adopts it. That adoption is the
//! promotion; there is no second pointer.

use std::sync::Arc;

use agentd_core::limits::required;
use agentd_core::metadata::MetadataPolicy;
use agentd_core::ports::{FireTarget, ScheduleSpec, SchedulerPort};
use agentd_core::schedule::{CronKind, CronSchedule};
use agentd_core::{ApiError, AuthContext, Timestamp};
use agentd_memory::service::MemoriesService;
use agentd_memory::types::{CreateMemoryRequest, UpdateMemoryRequest};
use async_trait::async_trait;

use crate::store::{DreamsStore, StoreError};
use crate::types::{
    Candidate, CandidateOutcome, CreateDreamRequest, Dream, DreamId, DreamRun, DreamRunId,
    DreamRunTag, DreamStatus, DreamTag, Verdict,
};

/// Generates candidate memories from recent activity. The LLM implementation
/// lives in `agentd-engine`; tests use deterministic fakes.
#[async_trait]
pub trait Proposer: Send + Sync {
    /// Propose candidates for one dream firing.
    async fn propose(
        &self,
        ctx: &AuthContext,
        dream: &Dream,
    ) -> Result<Vec<Candidate>, ApiError>;
}

/// Judges one candidate against the live memory (evidence·conflict·novelty).
/// LLM implementation in `agentd-engine`; deterministic fakes in tests.
#[async_trait]
pub trait ReviewGate: Send + Sync {
    /// Review a candidate. `live_content` is the current `latest` content at
    /// the candidate's path (None when the path is empty).
    async fn review(
        &self,
        ctx: &AuthContext,
        candidate: &Candidate,
        live_content: Option<&str>,
    ) -> Result<Verdict, ApiError>;
}

/// Dream service.
pub struct DreamsService {
    store: Arc<dyn DreamsStore>,
    memories: Arc<MemoriesService>,
    proposer: Arc<dyn Proposer>,
    gate: Arc<dyn ReviewGate>,
    scheduler: Arc<dyn SchedulerPort>,
}

impl std::fmt::Debug for DreamsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DreamsService").finish_non_exhaustive()
    }
}

impl DreamsService {
    /// Build the service.
    pub fn new(
        store: Arc<dyn DreamsStore>,
        memories: Arc<MemoriesService>,
        proposer: Arc<dyn Proposer>,
        gate: Arc<dyn ReviewGate>,
        scheduler: Arc<dyn SchedulerPort>,
    ) -> Self {
        Self {
            store,
            memories,
            proposer,
            gate,
            scheduler,
        }
    }

    /// `POST /v1/dreams`.
    pub async fn create(&self, ctx: &AuthContext, req: CreateDreamRequest) -> Result<Dream, ApiError> {
        let name = required("name", req.name)?;
        let memory_store_id = required("memory_store_id", req.memory_store_id)?;
        let schedule = required("schedule", req.schedule)?;
        req.metadata.validate(MetadataPolicy::capped())?;
        // Validate cron + tz through the shared engine.
        CronSchedule {
            kind: CronKind::Cron,
            expression: schedule.expression.clone(),
            timezone: schedule.timezone.clone(),
        }
        .compile()
        .map_err(|e| ApiError::invalid_request(format!("schedule: {e}")))?;
        // The curated store must exist in this workspace.
        self.memories.get_store(ctx, &memory_store_id).await?;
        let now = Timestamp::now();
        let dream = Dream {
            created_at: now,
            id: DreamId::generate(),
            memory_store_id,
            metadata: req.metadata,
            name,
            schedule,
            status: DreamStatus::Active,
            tag: DreamTag::Dream,
            updated_at: now,
        };
        self.store
            .insert(&ctx.workspace, dream.clone())
            .await
            .map_err(map_store)?;
        self.scheduler
            .upsert(ScheduleSpec {
                cron: dream.schedule.expression.clone(),
                timezone: dream.schedule.timezone.clone(),
                target: FireTarget::Dream {
                    id: dream.id.as_str().to_owned(),
                },
            })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(dream)
    }

    /// `GET /v1/dreams/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Dream, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/dreams`.
    pub async fn list(&self, ctx: &AuthContext) -> Result<Vec<Dream>, ApiError> {
        self.store.list(&ctx.workspace).await.map_err(map_store)
    }

    /// `GET /v1/dreams/{id}/runs`.
    pub async fn runs(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vec<DreamRun>, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.runs(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// Fire one dream (scheduled via `/internal/dream/fire`, or manual).
    /// Paused/archived scheduled fires are dropped (None).
    pub async fn fire(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        manual: bool,
    ) -> Result<Option<DreamRun>, ApiError> {
        let id = parse_id(raw_id)?;
        let dream = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        match dream.status {
            DreamStatus::Archived => {
                if manual {
                    return Err(ApiError::conflict_invalid_request(
                        "cannot run an archived dream",
                    ));
                }
                return Ok(None);
            }
            DreamStatus::Paused if !manual => return Ok(None),
            _ => {}
        }

        // 1. Propose.
        let candidates = self.proposer.propose(ctx, &dream).await?;
        let mut outcomes = Vec::with_capacity(candidates.len());
        let mut promoted = 0u32;
        let mut rejected = 0u32;

        for candidate in candidates {
            // Live content at the candidate's path (the gate's comparison target).
            let live = self
                .memories
                .list_memories(
                    ctx,
                    &dream.memory_store_id,
                    Some(&candidate.path),
                    agentd_memory::service::View::Full,
                )
                .await?
                .into_iter()
                .find(|m| m.path == candidate.path);

            // 2. Review against live memory.
            let verdict = self
                .gate
                .review(ctx, &candidate, live.as_ref().and_then(|m| m.content.as_deref()))
                .await?;

            // 3. Pass → write the version (create or update). Fail → nothing.
            let memory_version_id = if verdict.promoted() {
                promoted += 1;
                let head = match live {
                    Some(existing) => {
                        self.memories
                            .update_memory(
                                ctx,
                                &dream.memory_store_id,
                                existing.id.as_str(),
                                UpdateMemoryRequest {
                                    content: Some(candidate.content.clone()),
                                    ..Default::default()
                                },
                            )
                            .await?
                    }
                    None => {
                        self.memories
                            .create_memory(
                                ctx,
                                &dream.memory_store_id,
                                CreateMemoryRequest {
                                    path: Some(candidate.path.clone()),
                                    content: Some(candidate.content.clone()),
                                },
                            )
                            .await?
                    }
                };
                Some(head.memory_version_id.as_str().to_owned())
            } else {
                rejected += 1;
                None
            };

            outcomes.push(CandidateOutcome {
                candidate,
                verdict,
                memory_version_id,
            });
        }

        let run = DreamRun {
            candidates: outcomes,
            created_at: Timestamp::now(),
            dream_id: dream.id.clone(),
            id: DreamRunId::generate(),
            promoted_count: promoted,
            rejected_count: rejected,
            tag: DreamRunTag::DreamRun,
        };
        self.store
            .push_run(&ctx.workspace, run.clone())
            .await
            .map_err(map_store)?;
        Ok(Some(run))
    }

    /// `POST /v1/dreams/{id}/pause`.
    pub async fn pause(&self, ctx: &AuthContext, raw_id: &str) -> Result<Dream, ApiError> {
        self.set_status(ctx, raw_id, DreamStatus::Paused, true).await
    }

    /// `POST /v1/dreams/{id}/unpause`.
    pub async fn unpause(&self, ctx: &AuthContext, raw_id: &str) -> Result<Dream, ApiError> {
        self.set_status(ctx, raw_id, DreamStatus::Active, true).await
    }

    /// `POST /v1/dreams/{id}/archive` — terminal.
    pub async fn archive(&self, ctx: &AuthContext, raw_id: &str) -> Result<Dream, ApiError> {
        let d = self.set_status(ctx, raw_id, DreamStatus::Archived, false).await?;
        let _ = self
            .scheduler
            .delete(&FireTarget::Dream {
                id: d.id.as_str().to_owned(),
            })
            .await;
        Ok(d)
    }

    async fn set_status(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        status: DreamStatus,
        guard_archived: bool,
    ) -> Result<Dream, ApiError> {
        let id = parse_id(raw_id)?;
        let mut d = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        if guard_archived && d.status == DreamStatus::Archived {
            return Err(ApiError::conflict_invalid_request(
                "dream is archived and immutable",
            ));
        }
        d.status = status;
        d.updated_at = Timestamp::now();
        self.store
            .replace(&ctx.workspace, d.clone())
            .await
            .map_err(map_store)?;
        let target = FireTarget::Dream {
            id: d.id.as_str().to_owned(),
        };
        match status {
            DreamStatus::Paused => {
                let _ = self.scheduler.pause(&target).await;
            }
            DreamStatus::Active => {
                let _ = self.scheduler.unpause(&target).await;
            }
            DreamStatus::Archived => {}
        }
        Ok(d)
    }
}

fn parse_id(raw: &str) -> Result<DreamId, ApiError> {
    DreamId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("dream not found"),
    }
}
