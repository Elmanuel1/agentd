//! agentd-memory service tests: live-wire compatibility + the versioned
//! `latest`-ref extension (promote-don't-replace substrate).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_core::hash::content_sha256;
use agentd_core::{AuthContext, OrgId, WorkspaceId};
use agentd_memory::service::{MemoriesService, View};
use agentd_memory::store::{InMemoryMemoryStore, VersionSelector};
use agentd_memory::types::{
    CreateMemoryRequest, CreateStoreRequest, Precondition, UpdateMemoryRequest,
};

fn svc() -> MemoriesService {
    MemoriesService::new(Arc::new(InMemoryMemoryStore::new()))
}

fn ctx() -> AuthContext {
    AuthContext {
        org: OrgId("org_t".into()),
        workspace: WorkspaceId("wrkspc_01t".into()),
        api_key_id: "apikey_01Wnw5ximjHVpuyEXdDxFe5x".into(),
    }
}

async fn store(s: &MemoriesService) -> String {
    s.create_store(
        &ctx(),
        CreateStoreRequest {
            name: Some("compat-probe-mem".into()),
            description: Some("probe".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id
    .as_str()
    .to_owned()
}

fn create_req(path: &str, content: &str) -> CreateMemoryRequest {
    CreateMemoryRequest {
        path: Some(path.into()),
        content: Some(content.into()),
    }
}

#[tokio::test]
async fn create_memory_matches_live_fixture_shape() {
    let s = svc();
    let sid = store(&s).await;
    let m = s
        .create_memory(&ctx(), &sid, create_req("/rules.md", "v1 content"))
        .await
        .unwrap();
    let v = serde_json::to_value(&m).unwrap();
    // Live fixture: sha of "v1 content", no content field echoed, head pointer set.
    assert_eq!(
        v["content_sha256"],
        "60329bc85480d3c6a566d6101ce5f9fbee836b89e1952cf16326fb392cb59c55"
    );
    assert_eq!(v["content_size_bytes"], 10);
    assert!(v.get("content").is_none(), "create must NOT echo content");
    assert!(v["memory_version_id"].as_str().unwrap().starts_with("memver_01"));
    assert_eq!(v["type"], "memory");
    assert_eq!(v["path"], "/rules.md");
}

#[tokio::test]
async fn occupied_path_conflict_matches_live_409() {
    let s = svc();
    let sid = store(&s).await;
    let first = s
        .create_memory(&ctx(), &sid, create_req("/a.md", "one"))
        .await
        .unwrap();
    let e = s
        .create_memory(&ctx(), &sid, create_req("/a.md", "dup"))
        .await
        .unwrap_err();
    // Live: 409 memory_path_conflict_error with exact message + extra fields.
    assert_eq!(e.status, 409);
    assert_eq!(e.error_type.wire_name(), "memory_path_conflict_error");
    assert_eq!(
        e.message,
        format!(
            "path `/a.md` is already used by `{}`; use update to modify it",
            first.id.as_str()
        )
    );
    assert_eq!(
        e.extra.get("conflicting_memory_id").and_then(|v| v.as_str()),
        Some(first.id.as_str())
    );
}

#[tokio::test]
async fn update_appends_version_and_advances_latest() {
    let s = svc();
    let sid = store(&s).await;
    let m = s
        .create_memory(&ctx(), &sid, create_req("/rules.md", "v1 content"))
        .await
        .unwrap();
    let v1_head = m.memory_version_id.clone();
    let updated = s
        .update_memory(
            &ctx(),
            &sid,
            m.id.as_str(),
            UpdateMemoryRequest {
                content: Some("v2 via POST".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    // Live probe: update returned NEW memory_version_id, same memory id,
    // created_at unchanged, updated_at bumped.
    assert_ne!(updated.memory_version_id, v1_head);
    assert_eq!(updated.id, m.id);
    assert_eq!(updated.created_at, m.created_at);
    assert_eq!(
        updated.content_sha256,
        content_sha256(b"v2 via POST")
    );
    // Chain now has 2 versions, newest first in list_versions.
    let versions = s
        .list_versions(&ctx(), &sid, Some(m.id.as_str()), View::Basic)
        .await
        .unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].id, updated.memory_version_id);
}

#[tokio::test]
async fn full_view_returns_content_of_latest() {
    let s = svc();
    let sid = store(&s).await;
    let m = s
        .create_memory(&ctx(), &sid, create_req("/n.md", "hello"))
        .await
        .unwrap();
    let full = s
        .get_memory(&ctx(), &sid, m.id.as_str(), View::Full)
        .await
        .unwrap();
    assert_eq!(full.content.as_deref(), Some("hello"));
    let basic = s
        .get_memory(&ctx(), &sid, m.id.as_str(), View::Basic)
        .await
        .unwrap();
    assert!(basic.content.is_none());
}

#[tokio::test]
async fn rollback_repoints_latest_without_new_version() {
    // The promote-don't-replace core: rollback = O(1) ref move.
    let s = svc();
    let sid = store(&s).await;
    let m = s
        .create_memory(&ctx(), &sid, create_req("/r.md", "v1"))
        .await
        .unwrap();
    let v1 = m.memory_version_id.clone();
    s.update_memory(
        &ctx(),
        &sid,
        m.id.as_str(),
        UpdateMemoryRequest {
            content: Some("v2".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Roll back to v1.
    let rolled = s
        .rollback(&ctx(), &sid, m.id.as_str(), v1.as_str())
        .await
        .unwrap();
    assert_eq!(rolled.memory_version_id, v1);
    assert_eq!(rolled.content_sha256, content_sha256(b"v1"));
    // No third version was written — the chain still has exactly 2.
    let versions = s
        .list_versions(&ctx(), &sid, Some(m.id.as_str()), View::Basic)
        .await
        .unwrap();
    assert_eq!(versions.len(), 2);
    // Full view now serves v1 content: the next session adopting `latest`
    // sees the rolled-back state.
    let full = s
        .get_memory(&ctx(), &sid, m.id.as_str(), View::Full)
        .await
        .unwrap();
    assert_eq!(full.content.as_deref(), Some("v1"));
}

#[tokio::test]
async fn resolve_latest_vs_pinned() {
    let s = svc();
    let sid_raw = store(&s).await;
    let m = s
        .create_memory(&ctx(), &sid_raw, create_req("/p.md", "v1"))
        .await
        .unwrap();
    let v1 = m.memory_version_id.clone();
    let updated = s
        .update_memory(
            &ctx(),
            &sid_raw,
            m.id.as_str(),
            UpdateMemoryRequest {
                content: Some("v2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let sid = m.memory_store_id.clone();
    // latest → v2
    let latest = s
        .resolve(&ctx(), &sid, &m.id, &VersionSelector::Latest)
        .await
        .unwrap();
    assert_eq!(latest.id, updated.memory_version_id);
    assert_eq!(latest.content.as_deref(), Some("v2"));
    // pinned → v1 forever
    let pinned = s
        .resolve(&ctx(), &sid, &m.id, &VersionSelector::Pinned(v1.clone()))
        .await
        .unwrap();
    assert_eq!(pinned.id, v1);
    assert_eq!(pinned.content.as_deref(), Some("v1"));
}

#[tokio::test]
async fn precondition_mismatch_is_409_typed() {
    let s = svc();
    let sid = store(&s).await;
    let m = s
        .create_memory(&ctx(), &sid, create_req("/c.md", "current"))
        .await
        .unwrap();
    let e = s
        .update_memory(
            &ctx(),
            &sid,
            m.id.as_str(),
            UpdateMemoryRequest {
                content: Some("new".into()),
                precondition: Some(Precondition::ContentSha256 {
                    content_sha256: "a".repeat(64),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.status, 409);
    assert_eq!(e.error_type.wire_name(), "memory_precondition_failed_error");
    // Matching precondition succeeds.
    s.update_memory(
        &ctx(),
        &sid,
        m.id.as_str(),
        UpdateMemoryRequest {
            content: Some("new".into()),
            precondition: Some(Precondition::ContentSha256 {
                content_sha256: content_sha256(b"current"),
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn bad_precondition_format_is_400_with_live_message() {
    let s = svc();
    let sid = store(&s).await;
    let m = s
        .create_memory(&ctx(), &sid, create_req("/c.md", "x"))
        .await
        .unwrap();
    let e = s
        .update_memory(
            &ctx(),
            &sid,
            m.id.as_str(),
            UpdateMemoryRequest {
                content: Some("y".into()),
                precondition: Some(Precondition::ContentSha256 {
                    content_sha256: "deadbeef".into(),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.status, 400);
    assert_eq!(
        e.message,
        "precondition.content_sha256: value length must be 64 characters (and 1 more validation errors)"
    );
}

#[tokio::test]
async fn content_size_cap_exact_live_error() {
    let s = svc();
    let sid = store(&s).await;
    let e = s
        .create_memory(&ctx(), &sid, create_req("/big.md", &"x".repeat(102_401)))
        .await
        .unwrap_err();
    assert_eq!(e.message, "content: value length must be at most 102400 bytes");
}

#[tokio::test]
async fn delete_journals_deleted_version_and_frees_path() {
    let s = svc();
    let sid = store(&s).await;
    let m = s
        .create_memory(&ctx(), &sid, create_req("/d.md", "x"))
        .await
        .unwrap();
    s.delete_memory(&ctx(), &sid, m.id.as_str(), None)
        .await
        .unwrap();
    // Memory is gone from reads…
    assert_eq!(
        s.get_memory(&ctx(), &sid, m.id.as_str(), View::Basic)
            .await
            .unwrap_err()
            .status,
        404
    );
    // …but the audit chain survives, ending in a `deleted` op.
    let versions = s
        .list_versions(&ctx(), &sid, Some(m.id.as_str()), View::Basic)
        .await
        .unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(
        serde_json::to_value(&versions[0]).unwrap()["operation"],
        "deleted"
    );
    // The path is free again.
    s.create_memory(&ctx(), &sid, create_req("/d.md", "fresh"))
        .await
        .unwrap();
}

#[tokio::test]
async fn redact_scrubs_content_preserves_audit_spine() {
    let s = svc();
    let sid = store(&s).await;
    let m = s
        .create_memory(&ctx(), &sid, create_req("/secret.md", "leaked-key"))
        .await
        .unwrap();
    let vid = m.memory_version_id.clone();
    let redacted = s
        .redact_version(&ctx(), &sid, vid.as_str())
        .await
        .unwrap();
    let v = serde_json::to_value(&redacted).unwrap();
    assert_eq!(v["content_sha256"], serde_json::Value::Null);
    assert_eq!(v["content_size_bytes"], serde_json::Value::Null);
    assert_eq!(v["path"], serde_json::Value::Null);
    assert_eq!(v["redacted_by"]["type"], "api_actor");
    // Audit spine preserved.
    assert_eq!(v["operation"], "created");
    assert_eq!(v["created_by"]["type"], "api_actor");
    // Content unrecoverable even on full view.
    let full = s.get_version(&ctx(), &sid, vid.as_str()).await.unwrap();
    assert!(full.content.is_none());
}

#[tokio::test]
async fn rename_onto_occupied_path_conflicts() {
    let s = svc();
    let sid = store(&s).await;
    s.create_memory(&ctx(), &sid, create_req("/a.md", "a"))
        .await
        .unwrap();
    let b = s
        .create_memory(&ctx(), &sid, create_req("/b.md", "b"))
        .await
        .unwrap();
    let e = s
        .update_memory(
            &ctx(),
            &sid,
            b.id.as_str(),
            UpdateMemoryRequest {
                path: Some("/a.md".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.status, 409);
    assert_eq!(e.error_type.wire_name(), "memory_path_conflict_error");
}

#[tokio::test]
async fn store_lifecycle_and_archived_writes_conflict() {
    let s = svc();
    let sid = store(&s).await;
    s.archive_store(&ctx(), &sid).await.unwrap();
    let e = s
        .create_memory(&ctx(), &sid, create_req("/x.md", "x"))
        .await
        .unwrap_err();
    assert_eq!(e.status, 409);
    s.delete_store(&ctx(), &sid).await.unwrap();
    assert_eq!(s.get_store(&ctx(), &sid).await.unwrap_err().status, 404);
}

#[tokio::test]
async fn list_memories_path_prefix_filter() {
    let s = svc();
    let sid = store(&s).await;
    for p in ["/notes/a.md", "/notes/b.md", "/other/c.md"] {
        s.create_memory(&ctx(), &sid, create_req(p, "x"))
            .await
            .unwrap();
    }
    let notes = s
        .list_memories(&ctx(), &sid, Some("/notes/"), View::Basic)
        .await
        .unwrap();
    assert_eq!(notes.len(), 2);
    let all = s.list_memories(&ctx(), &sid, None, View::Basic).await.unwrap();
    assert_eq!(all.len(), 3);
}
