//! Integration tests against a live LocalStack-compatible emulator (Floci).
//!
//! Gated by `AGENTD_AWS_TESTS=1` + `AGENTD_AWS_ENDPOINT` so the hermetic
//! `cargo test` stays offline. Run with:
//! ```sh
//! AGENTD_AWS_TESTS=1 AGENTD_AWS_ENDPOINT=http://localhost:4566 \
//!   AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_DEFAULT_REGION=us-east-1 \
//!   cargo test -p agentd-store-aws --test aws_integration
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_agents::store::AgentsStore;
use agentd_agents::types::{AgentTag, ModelSpec, ModelSpeed};
use agentd_agents::{Agent, AgentsService};
use agentd_core::limits::ModelCatalog;
use agentd_core::{AuthContext, Metadata, OrgId, Timestamp, WorkspaceId};
use agentd_deployments::store::DeploymentsStore;
use agentd_files::store::{FilesStore, StoredFile};
use agentd_store_aws::{AwsBackend, DynamoAgentsStore, DynamoDeploymentsStore, DynamoFilesStore};

fn enabled() -> bool {
    std::env::var("AGENTD_AWS_TESTS").as_deref() == Ok("1")
}

/// Unique prefix per test run so reruns don't collide (Floci persists within
/// a container lifetime).
fn unique_prefix(tag: &str) -> String {
    let n = Timestamp::now().as_datetime().timestamp_micros();
    format!("agentdtest{tag}{n}")
}

async fn backend(tag: &str) -> AwsBackend {
    AwsBackend::connect(unique_prefix(tag)).await.unwrap()
}

fn ws(n: &str) -> WorkspaceId {
    WorkspaceId(n.to_owned())
}

fn ctx() -> AuthContext {
    AuthContext {
        org: OrgId("org_t".into()),
        workspace: ws("wrkspc_01aws"),
        api_key_id: "apikey_01aws".into(),
    }
}

fn agent(version: u64, id: agentd_agents::types::AgentId) -> Agent {
    Agent {
        archived_at: None,
        created_at: Timestamp::now(),
        description: None,
        id,
        mcp_servers: vec![],
        metadata: Metadata::new(),
        model: ModelSpec {
            id: "claude-sonnet-4-6".into(),
            speed: ModelSpeed::Standard,
        },
        multiagent: None,
        name: "aws-agent".into(),
        skills: vec![],
        system: None,
        tools: vec![],
        tag: AgentTag::Agent,
        updated_at: Timestamp::now(),
        version,
    }
}

// ── Agents: versioned entity round-trips through DynamoDB ────────────────

#[tokio::test]
async fn dynamo_agents_versioned_roundtrip() {
    if !enabled() {
        eprintln!("skipping (set AGENTD_AWS_TESTS=1)");
        return;
    }
    let store = DynamoAgentsStore::new(backend("agents").await).await.unwrap();
    let id = agentd_agents::types::AgentId::generate();

    store.insert(&ctx().workspace, agent(1, id.clone())).await.unwrap();
    assert_eq!(store.latest(&ctx().workspace, &id).await.unwrap().version, 1);

    store
        .append_version(&ctx().workspace, agent(2, id.clone()))
        .await
        .unwrap();
    assert_eq!(store.latest(&ctx().workspace, &id).await.unwrap().version, 2);
    assert_eq!(store.version(&ctx().workspace, &id, 1).await.unwrap().version, 1);

    let versions = store.versions(&ctx().workspace, &id).await.unwrap();
    assert_eq!(versions.iter().map(|a| a.version).collect::<Vec<_>>(), vec![1, 2]);

    // list returns latest of each; a second agent added.
    let id2 = agentd_agents::types::AgentId::generate();
    store.insert(&ctx().workspace, agent(1, id2.clone())).await.unwrap();
    let listed = store.list(&ctx().workspace).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|a| a.id == id && a.version == 2));

    // workspace isolation
    assert!(store.latest(&ws("wrkspc_02other"), &id).await.is_err());
}

/// The full service (validation + canonicalization) driving DynamoDB —
/// proves the AWS store is a true drop-in for the in-memory one.
#[tokio::test]
async fn agents_service_over_dynamo() {
    if !enabled() {
        return;
    }
    let store = Arc::new(DynamoAgentsStore::new(backend("svc").await).await.unwrap());
    let svc = AgentsService::new(store, ModelCatalog::default_catalog());
    let req: agentd_agents::types::CreateAgentRequest = serde_json::from_str(
        r#"{"name":"x","model":"claude-sonnet-4-6","tools":[{"type":"agent_toolset_20260401"}]}"#,
    )
    .unwrap();
    let a = svc.create(&ctx(), req).await.unwrap();
    assert_eq!(a.version, 1);
    let updated = svc
        .update(
            &ctx(),
            a.id.as_str(),
            serde_json::from_str(r#"{"system":"v2"}"#).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    // fetched back from Dynamo
    assert_eq!(svc.get(&ctx(), a.id.as_str(), None).await.unwrap().version, 2);
    // model canonicalization survived the round-trip
    assert_eq!(updated.model.id, "claude-sonnet-4-6");
}

// ── Deployments: the cross-replica exactly-once claim on DynamoDB ─────────

#[tokio::test]
async fn dynamo_claim_fire_is_exactly_once_across_concurrent_callers() {
    if !enabled() {
        return;
    }
    let store = Arc::new(
        DynamoDeploymentsStore::new(backend("claim").await)
            .await
            .unwrap(),
    );
    let did = agentd_deployments::types::DeploymentId::generate();
    let scheduled = "2026-07-10T00:00:00.000000Z";

    // 8 "replicas" race to claim the SAME (deployment, scheduled_at). The
    // DynamoDB conditional write must let exactly ONE win.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let s = store.clone();
        let w = ctx().workspace.clone();
        let d = did.clone();
        handles.push(tokio::spawn(async move {
            s.claim_fire(&w, &d, scheduled).await.unwrap()
        }));
    }
    let mut wins = 0;
    for h in handles {
        if h.await.unwrap() {
            wins += 1;
        }
    }
    assert_eq!(wins, 1, "exactly one replica claimed the tick");

    // A different scheduled_at is a fresh claim.
    assert!(store
        .claim_fire(&ctx().workspace, &did, "2026-07-17T00:00:00.000000Z")
        .await
        .unwrap());
}

// ── Files: DynamoDB metadata + S3 bytes ──────────────────────────────────

#[tokio::test]
async fn dynamo_s3_files_roundtrip() {
    if !enabled() {
        return;
    }
    let store = DynamoFilesStore::new(backend("files").await).await.unwrap();
    let meta = agentd_files::types::FileMetadata {
        created_at: Timestamp::now(),
        downloadable: true,
        filename: "data.csv".into(),
        id: agentd_files::types::FileId::generate(),
        mime_type: "text/csv".into(),
        size_bytes: 8,
        tag: agentd_files::types::FileTag::File,
    };
    let id = meta.id.clone();
    store
        .insert(
            &ctx().workspace,
            StoredFile {
                meta,
                bytes: bytes::Bytes::from_static(b"a,b\n1,2\n"),
                scope_id: Some("sesn_01A".into()),
            },
        )
        .await
        .unwrap();

    // bytes come back from S3
    let got = store.get(&ctx().workspace, &id).await.unwrap();
    assert_eq!(&got.bytes[..], b"a,b\n1,2\n");
    assert_eq!(got.meta.filename, "data.csv");

    // scope filter over DynamoDB
    let scoped = store.list(&ctx().workspace, Some("sesn_01A")).await.unwrap();
    assert_eq!(scoped.len(), 1);
    let other = store.list(&ctx().workspace, Some("sesn_01Z")).await.unwrap();
    assert!(other.is_empty());

    store.delete(&ctx().workspace, &id).await.unwrap();
    assert!(store.get(&ctx().workspace, &id).await.is_err());
}
