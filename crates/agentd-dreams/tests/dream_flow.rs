//! Dream loop tests — the promote-don't-replace contract, end to end
//! against the real versioned memory service.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_core::ports::FakeScheduler;
use agentd_core::{ApiError, AuthContext, OrgId, WorkspaceId};
use agentd_dreams::service::{DreamsService, Proposer, ReviewGate};
use agentd_dreams::store::InMemoryDreamsStore;
use agentd_dreams::types::{Candidate, CreateDreamRequest, Dream, Verdict};
use agentd_memory::service::{MemoriesService, View};
use agentd_memory::store::InMemoryMemoryStore;
use agentd_memory::types::{CreateMemoryRequest, CreateStoreRequest};
use async_trait::async_trait;
use parking_lot::Mutex;

/// Scripted proposer: returns whatever the test arms.
#[derive(Default)]
struct ScriptedProposer {
    candidates: Mutex<Vec<Candidate>>,
}

#[async_trait]
impl Proposer for ScriptedProposer {
    async fn propose(&self, _ctx: &AuthContext, _dream: &Dream) -> Result<Vec<Candidate>, ApiError> {
        Ok(self.candidates.lock().clone())
    }
}

/// Deterministic gate:
/// - evidence fails when provenance is empty,
/// - novelty fails when the candidate equals live content,
/// - conflict fails when content contains "CONTRADICTION".
struct DeterministicGate;

#[async_trait]
impl ReviewGate for DeterministicGate {
    async fn review(
        &self,
        _ctx: &AuthContext,
        candidate: &Candidate,
        live_content: Option<&str>,
    ) -> Result<Verdict, ApiError> {
        let evidence_ok = !candidate.provenance.is_empty();
        let novelty_ok = live_content != Some(candidate.content.as_str());
        let conflict_ok = !candidate.content.contains("CONTRADICTION");
        Ok(Verdict {
            evidence_ok,
            conflict_ok,
            novelty_ok,
            reasoning: "deterministic test gate".into(),
        })
    }
}

struct Rig {
    dreams: DreamsService,
    memories: Arc<MemoriesService>,
    proposer: Arc<ScriptedProposer>,
    store_id: String,
}

fn ctx() -> AuthContext {
    AuthContext {
        org: OrgId("org_t".into()),
        workspace: WorkspaceId("wrkspc_01t".into()),
        api_key_id: "apikey_01t".into(),
    }
}

async fn rig() -> Rig {
    let memories = Arc::new(MemoriesService::new(Arc::new(InMemoryMemoryStore::new())));
    let store = memories
        .create_store(
            &ctx(),
            CreateStoreRequest {
                name: Some("curated".into()),
                description: Some("dream target".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let proposer = Arc::new(ScriptedProposer::default());
    let dreams = DreamsService::new(
        Arc::new(InMemoryDreamsStore::new()),
        memories.clone(),
        proposer.clone(),
        Arc::new(DeterministicGate),
        Arc::new(FakeScheduler::new()),
    );
    Rig {
        dreams,
        memories,
        proposer,
        store_id: store.id.as_str().to_owned(),
    }
}

fn dream_req(store_id: &str) -> CreateDreamRequest {
    serde_json::from_value(serde_json::json!({
        "name": "nightly-consolidation",
        "memory_store_id": store_id,
        "schedule": {"type":"cron","expression":"0 3 * * *","timezone":"UTC"}
    }))
    .unwrap()
}

fn candidate(path: &str, content: &str, provenance: &[&str]) -> Candidate {
    Candidate {
        path: path.into(),
        content: content.into(),
        provenance: provenance.iter().map(|s| s.to_string()).collect(),
        confidence: 0.9,
    }
}

#[tokio::test]
async fn promoted_candidate_writes_version_and_next_resolve_adopts() {
    let r = rig().await;
    let d = r.dreams.create(&ctx(), dream_req(&r.store_id)).await.unwrap();
    r.proposer.candidates.lock().push(candidate(
        "/lessons/pricing.md",
        "GC quotes must include bond costs.",
        &["sesn_01A", "outc_01B"],
    ));
    let run = r
        .dreams
        .fire(&ctx(), d.id.as_str(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.promoted_count, 1);
    assert_eq!(run.rejected_count, 0);
    assert!(run.candidates[0].memory_version_id.is_some());
    // The next session resolving latest sees the promoted content.
    let mems = r
        .memories
        .list_memories(&ctx(), &r.store_id, None, View::Full)
        .await
        .unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(
        mems[0].content.as_deref(),
        Some("GC quotes must include bond costs.")
    );
}

#[tokio::test]
async fn rejected_candidate_writes_nothing_latest_untouched() {
    let r = rig().await;
    // Seed live memory.
    let live = r
        .memories
        .create_memory(
            &ctx(),
            &r.store_id,
            CreateMemoryRequest {
                path: Some("/lessons/pricing.md".into()),
                content: Some("v1 truth".into()),
            },
        )
        .await
        .unwrap();
    let v1 = live.memory_version_id.clone();
    let d = r.dreams.create(&ctx(), dream_req(&r.store_id)).await.unwrap();
    // No provenance → evidence fails → rejected.
    r.proposer
        .candidates
        .lock()
        .push(candidate("/lessons/pricing.md", "unsupported claim", &[]));
    let run = r
        .dreams
        .fire(&ctx(), d.id.as_str(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.promoted_count, 0);
    assert_eq!(run.rejected_count, 1);
    assert!(run.candidates[0].memory_version_id.is_none());
    // latest never moved — THE core promise.
    let after = r
        .memories
        .get_memory(&ctx(), &r.store_id, live.id.as_str(), View::Full)
        .await
        .unwrap();
    assert_eq!(after.memory_version_id, v1);
    assert_eq!(after.content.as_deref(), Some("v1 truth"));
}

#[tokio::test]
async fn duplicate_candidate_fails_novelty() {
    let r = rig().await;
    r.memories
        .create_memory(
            &ctx(),
            &r.store_id,
            CreateMemoryRequest {
                path: Some("/l.md".into()),
                content: Some("already known".into()),
            },
        )
        .await
        .unwrap();
    let d = r.dreams.create(&ctx(), dream_req(&r.store_id)).await.unwrap();
    r.proposer
        .candidates
        .lock()
        .push(candidate("/l.md", "already known", &["sesn_01A"]));
    let run = r
        .dreams
        .fire(&ctx(), d.id.as_str(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.rejected_count, 1);
    assert!(!run.candidates[0].verdict.novelty_ok);
}

#[tokio::test]
async fn conflicting_candidate_rejected_by_conflict_lens() {
    let r = rig().await;
    let d = r.dreams.create(&ctx(), dream_req(&r.store_id)).await.unwrap();
    r.proposer.candidates.lock().push(candidate(
        "/l.md",
        "CONTRADICTION with promoted memory",
        &["sesn_01A"],
    ));
    let run = r
        .dreams
        .fire(&ctx(), d.id.as_str(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.rejected_count, 1);
    assert!(!run.candidates[0].verdict.conflict_ok);
}

#[tokio::test]
async fn promotion_updates_existing_memory_as_new_version() {
    let r = rig().await;
    let live = r
        .memories
        .create_memory(
            &ctx(),
            &r.store_id,
            CreateMemoryRequest {
                path: Some("/l.md".into()),
                content: Some("old lesson".into()),
            },
        )
        .await
        .unwrap();
    let v1 = live.memory_version_id.clone();
    let d = r.dreams.create(&ctx(), dream_req(&r.store_id)).await.unwrap();
    r.proposer
        .candidates
        .lock()
        .push(candidate("/l.md", "refined lesson", &["outc_01X"]));
    let run = r
        .dreams
        .fire(&ctx(), d.id.as_str(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.promoted_count, 1);
    // A NEW version — supersession, not replacement: v1 still in the chain.
    let after = r
        .memories
        .get_memory(&ctx(), &r.store_id, live.id.as_str(), View::Full)
        .await
        .unwrap();
    assert_ne!(after.memory_version_id, v1);
    assert_eq!(after.content.as_deref(), Some("refined lesson"));
    let versions = r
        .memories
        .list_versions(&ctx(), &r.store_id, Some(live.id.as_str()), View::Basic)
        .await
        .unwrap();
    assert_eq!(versions.len(), 2);
}

#[tokio::test]
async fn paused_dream_drops_scheduled_fire_manual_still_works() {
    let r = rig().await;
    let d = r.dreams.create(&ctx(), dream_req(&r.store_id)).await.unwrap();
    r.dreams.pause(&ctx(), d.id.as_str()).await.unwrap();
    // Scheduled fire dropped.
    assert!(r
        .dreams
        .fire(&ctx(), d.id.as_str(), false)
        .await
        .unwrap()
        .is_none());
    // Manual fire still allowed.
    r.proposer
        .candidates
        .lock()
        .push(candidate("/m.md", "manual insight", &["sesn_01Z"]));
    assert!(r
        .dreams
        .fire(&ctx(), d.id.as_str(), true)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn archive_is_terminal() {
    let r = rig().await;
    let d = r.dreams.create(&ctx(), dream_req(&r.store_id)).await.unwrap();
    r.dreams.archive(&ctx(), d.id.as_str()).await.unwrap();
    let e = r.dreams.fire(&ctx(), d.id.as_str(), true).await.unwrap_err();
    assert_eq!(e.status, 409);
    assert_eq!(
        r.dreams.pause(&ctx(), d.id.as_str()).await.unwrap_err().status,
        409
    );
}

#[tokio::test]
async fn run_journal_records_every_decision() {
    let r = rig().await;
    let d = r.dreams.create(&ctx(), dream_req(&r.store_id)).await.unwrap();
    {
        let mut c = r.proposer.candidates.lock();
        c.push(candidate("/a.md", "good", &["sesn_01A"]));
        c.push(candidate("/b.md", "no evidence", &[]));
    }
    r.dreams.fire(&ctx(), d.id.as_str(), false).await.unwrap();
    let runs = r.dreams.runs(&ctx(), d.id.as_str()).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].candidates.len(), 2);
    assert_eq!(runs[0].promoted_count, 1);
    assert_eq!(runs[0].rejected_count, 1);
    // Provenance + verdict + resulting version are all journaled.
    let promoted = runs[0]
        .candidates
        .iter()
        .find(|c| c.verdict.promoted())
        .unwrap();
    assert_eq!(promoted.candidate.provenance, vec!["sesn_01A"]);
    assert!(promoted.memory_version_id.as_ref().unwrap().starts_with("memver_01"));
}

#[tokio::test]
async fn create_validates_store_and_schedule() {
    let r = rig().await;
    let mut req = dream_req(&r.store_id);
    req.memory_store_id = Some("memstore_00bogus".into());
    assert_eq!(
        r.dreams.create(&ctx(), req).await.unwrap_err().message,
        "Invalid memory store ID."
    );
    let mut req = dream_req(&r.store_id);
    req.schedule = serde_json::from_value(serde_json::json!(
        {"type":"cron","expression":"99 * * * *","timezone":"UTC"}
    ))
    .ok();
    assert!(r
        .dreams
        .create(&ctx(), req)
        .await
        .unwrap_err()
        .message
        .starts_with("schedule:"));
}
