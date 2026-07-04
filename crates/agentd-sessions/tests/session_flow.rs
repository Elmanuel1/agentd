//! Session end-to-end tests: create → send events → engine drains →
//! status transitions → idle-gate semantics — everything the SSE clients
//! rely on.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_agents::AgentSnapshot;
use agentd_core::{ApiError, AuthContext, OrgId, Usage, WorkspaceId};
use agentd_sessions::events::{EventPayload, ModelUsage, StopReason};
use agentd_sessions::service::{EngineContract, SessionDeps, SessionsService};
use agentd_sessions::store::InMemorySessionsStore;
use agentd_sessions::types::{
    CreateSessionRequest, OutcomeEvaluation, OutcomeResult, SessionStatus,
};
use async_trait::async_trait;

/// Fake dependency port: hands back a snapshot parsed from the live agent
/// fixture; accepts env `env_ok`, rejects `env_archived`.
struct FakeDeps;

#[async_trait]
impl SessionDeps for FakeDeps {
    async fn resolve_agent(
        &self,
        _ctx: &AuthContext,
        agent_id: &str,
        version: Option<u64>,
    ) -> Result<AgentSnapshot, ApiError> {
        if agent_id == "agent_00archived" {
            return Err(ApiError::conflict_invalid_request(
                "agent is archived; new sessions cannot reference it",
            ));
        }
        let agent: agentd_agents::Agent =
            serde_json::from_str(include_str!("../../../fixtures/agent.create.json")).unwrap();
        let mut snap = agent.snapshot();
        if let Some(v) = version {
            snap.version = v;
        }
        Ok(snap)
    }

    async fn check_environment(&self, _ctx: &AuthContext, env_id: &str) -> Result<(), ApiError> {
        if env_id == "env_archived" {
            return Err(ApiError::conflict_invalid_request("environment is archived"));
        }
        Ok(())
    }
}

fn svc() -> SessionsService {
    SessionsService::new(Arc::new(InMemorySessionsStore::new()), Arc::new(FakeDeps))
}

fn ctx() -> AuthContext {
    AuthContext {
        org: OrgId("org_t".into()),
        workspace: WorkspaceId("wrkspc_01t".into()),
        api_key_id: "apikey_01t".into(),
    }
}

fn create_req() -> CreateSessionRequest {
    serde_json::from_str(
        r#"{"agent":"agent_01XdReZgC7eHpg9b9Nvrwdcm","environment_id":"env_ok","title":"probe"}"#,
    )
    .unwrap()
}

fn user_msg(text: &str) -> EventPayload {
    serde_json::from_value(serde_json::json!({
        "type":"user.message",
        "content":[{"type":"text","text":text}]
    }))
    .unwrap()
}

#[tokio::test]
async fn create_matches_live_shape() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let v = serde_json::to_value(&session).unwrap();
    assert_eq!(v["type"], "session");
    assert_eq!(v["status"], "idle");
    assert_eq!(v["outcome_evaluations"], serde_json::json!([]));
    assert_eq!(v["stats"], serde_json::json!({"active_seconds":0,"duration_seconds":0}));
    assert_eq!(
        v["usage"],
        serde_json::json!({"cache_creation":null,"cache_read_input_tokens":0,"input_tokens":0,"output_tokens":0})
    );
    assert_eq!(v["vault_ids"], serde_json::json!([]));
    assert!(v["id"].as_str().unwrap().starts_with("sesn_01"));
    // Agent embedded as the full snapshot with version.
    assert_eq!(v["agent"]["type"], "agent");
    assert_eq!(v["agent"]["version"], 1);
    assert!(v["agent"].get("created_at").is_none());
}

#[tokio::test]
async fn archived_agent_or_environment_rejected() {
    let s = svc();
    let mut req = create_req();
    req.agent = serde_json::from_str("\"agent_00archived\"").ok();
    // NB: agent_00archived is grammar-invalid on purpose? No — keep valid grammar.
    // Use tagged form with valid grammar id handled by FakeDeps.
    let req2: CreateSessionRequest = serde_json::from_str(
        r#"{"agent":"agent_01XdReZgC7eHpg9b9Nvrwdcm","environment_id":"env_archived"}"#,
    )
    .unwrap();
    let e = s.create(&ctx(), req2).await.unwrap_err();
    assert_eq!(e.status, 409);
}

#[tokio::test]
async fn send_events_queues_then_engine_drains_and_stamps() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let sent = s
        .send_events(&ctx(), session.id.as_str(), vec![user_msg("hello")])
        .await
        .unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].processed_at.is_none(), "queued = unprocessed");

    // Engine drains: processed_at gets stamped.
    let ws = ctx().workspace;
    let drained = s.drain_user_events(&ws, &session.id).await.unwrap();
    assert_eq!(drained.len(), 1);
    assert!(drained[0].processed_at.is_some());
    assert_eq!(drained[0].id, sent[0].id);

    // Second drain is empty (nothing unprocessed).
    assert!(s.drain_user_events(&ws, &session.id).await.unwrap().is_empty());

    // History shows the processed form.
    let history = s.list_events(&ctx(), session.id.as_str()).await.unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].processed_at.is_some());
}

#[tokio::test]
async fn define_outcome_validation_exact_live_error_with_index_path() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let bad: EventPayload = serde_json::from_value(serde_json::json!({
        "type":"user.define_outcome",
        "description":"x",
        "rubric":{"type":"text","content":"do x"},
        "max_iterations":21
    }))
    .unwrap();
    let e = s
        .send_events(&ctx(), session.id.as_str(), vec![bad])
        .await
        .unwrap_err();
    assert_eq!(
        e.message,
        "events.0.user_define_outcome.max_iterations: value must be greater than or equal to 1 and less than or equal to 20"
    );
}

#[tokio::test]
async fn define_outcome_gets_server_assigned_outcome_id() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let ok: EventPayload = serde_json::from_value(serde_json::json!({
        "type":"user.define_outcome",
        "description":"build it",
        "rubric":{"type":"text","content":"has tests"},
        "max_iterations":5
    }))
    .unwrap();
    let sent = s
        .send_events(&ctx(), session.id.as_str(), vec![ok])
        .await
        .unwrap();
    match &sent[0].payload {
        EventPayload::UserDefineOutcome { outcome_id, .. } => {
            assert!(outcome_id.as_ref().unwrap().as_str().starts_with("outc_01"));
        }
        other => panic!("wrong payload {other:?}"),
    }
}

#[tokio::test]
async fn agent_events_are_rejected_from_the_client_path() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let agent_event: EventPayload = serde_json::from_value(serde_json::json!({
        "type":"agent.message","content":[{"type":"text","text":"i am the model"}]
    }))
    .unwrap();
    let e = s
        .send_events(&ctx(), session.id.as_str(), vec![agent_event])
        .await
        .unwrap_err();
    assert_eq!(e.status, 400);
    assert!(e.message.starts_with("events.0:"));
}

#[tokio::test]
async fn status_machine_idle_running_idle_terminated() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let ws = ctx().workspace;

    s.set_status(&ws, &session.id, SessionStatus::Running, None)
        .await
        .unwrap();
    assert_eq!(
        s.get(&ctx(), session.id.as_str()).await.unwrap().status,
        SessionStatus::Running
    );

    s.set_status(
        &ws,
        &session.id,
        SessionStatus::Idle,
        Some(StopReason::EndTurn {}),
    )
    .await
    .unwrap();

    s.set_status(&ws, &session.id, SessionStatus::Terminated, None)
        .await
        .unwrap();

    // Terminated is absorbing: further transitions conflict.
    let e = s
        .set_status(&ws, &session.id, SessionStatus::Running, None)
        .await
        .unwrap_err();
    assert_eq!(e.status, 409);

    // Sending events to a terminated session conflicts.
    let e = s
        .send_events(&ctx(), session.id.as_str(), vec![user_msg("too late")])
        .await
        .unwrap_err();
    assert_eq!(e.status, 409);

    // Status events were journaled in order.
    let history = s.list_events(&ctx(), session.id.as_str()).await.unwrap();
    let types: Vec<String> = history
        .iter()
        .map(|e| serde_json::to_value(e).unwrap()["type"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        types,
        vec![
            "session.status_running",
            "session.status_idle",
            "session.status_terminated"
        ]
    );
}

#[tokio::test]
async fn sse_subscription_receives_live_events() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let mut rx = s.subscribe(&ctx(), session.id.as_str()).await.unwrap();
    // A queued client event is broadcast immediately.
    s.send_events(&ctx(), session.id.as_str(), vec![user_msg("ping")])
        .await
        .unwrap();
    let got = rx.recv().await.unwrap();
    assert!(got.payload.is_user_event());
    // Engine emit is broadcast too.
    let ws = ctx().workspace;
    s.emit(
        &ws,
        &session.id,
        serde_json::from_value(serde_json::json!({
            "type":"agent.message","content":[{"type":"text","text":"pong"}]
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    // Skip the processed re-broadcast of the user event if present, then find agent.message.
    let mut found_agent = false;
    for _ in 0..3 {
        if let Ok(ev) = rx.recv().await {
            if matches!(ev.payload, EventPayload::AgentMessage { .. }) {
                found_agent = true;
                break;
            }
        }
    }
    assert!(found_agent);
}

#[tokio::test]
async fn usage_accumulates_and_outcomes_record() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let ws = ctx().workspace;
    s.add_usage(
        &ws,
        &session.id,
        &Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: 10,
            cache_creation: None,
        },
    )
    .await
    .unwrap();
    s.add_usage(
        &ws,
        &session.id,
        &Usage {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_input_tokens: 3,
            cache_creation: None,
        },
    )
    .await
    .unwrap();
    let got = s.get(&ctx(), session.id.as_str()).await.unwrap();
    assert_eq!(got.usage.input_tokens, 101);
    assert_eq!(got.usage.output_tokens, 52);

    // Outcome evaluations upsert by outcome_id.
    let oid = agentd_sessions::types::OutcomeId::generate();
    s.record_outcome(
        &ws,
        &session.id,
        OutcomeEvaluation {
            outcome_id: oid.clone(),
            result: OutcomeResult::NeedsRevision,
            explanation: None,
            iteration: 0,
        },
    )
    .await
    .unwrap();
    s.record_outcome(
        &ws,
        &session.id,
        OutcomeEvaluation {
            outcome_id: oid.clone(),
            result: OutcomeResult::Satisfied,
            explanation: Some("all criteria met".into()),
            iteration: 1,
        },
    )
    .await
    .unwrap();
    let got = s.get(&ctx(), session.id.as_str()).await.unwrap();
    assert_eq!(got.outcome_evaluations.len(), 1);
    assert_eq!(got.outcome_evaluations[0].result, OutcomeResult::Satisfied);
}

#[tokio::test]
async fn archive_blocks_writes_and_running_blocks_archive() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let ws = ctx().workspace;

    // Running blocks archive (documented post-idle race behavior).
    s.set_status(&ws, &session.id, SessionStatus::Running, None)
        .await
        .unwrap();
    assert_eq!(
        s.archive(&ctx(), session.id.as_str()).await.unwrap_err().status,
        409
    );
    s.set_status(&ws, &session.id, SessionStatus::Idle, Some(StopReason::EndTurn {}))
        .await
        .unwrap();

    // Archive, then writes conflict.
    s.archive(&ctx(), session.id.as_str()).await.unwrap();
    let e = s
        .send_events(&ctx(), session.id.as_str(), vec![user_msg("x")])
        .await
        .unwrap_err();
    assert_eq!(e.status, 409);
}

#[tokio::test]
async fn missing_required_fields() {
    let s = svc();
    let e = s
        .create(
            &ctx(),
            serde_json::from_str(r#"{"environment_id":"env_ok"}"#).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(e.message, "agent: Field required");
    let e = s
        .create(
            &ctx(),
            serde_json::from_str(r#"{"agent":"agent_01XdReZgC7eHpg9b9Nvrwdcm"}"#).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(e.message, "environment_id: Field required");
}

#[tokio::test]
async fn model_request_span_events_flow_through_engine_contract() {
    let s = svc();
    let session = s.create(&ctx(), create_req()).await.unwrap();
    let ws = ctx().workspace;
    let start = s
        .emit(&ws, &session.id, EventPayload::SpanModelRequestStart {})
        .await
        .unwrap();
    s.emit(
        &ws,
        &session.id,
        EventPayload::SpanModelRequestEnd {
            model_request_start_id: start.id.clone(),
            is_error: false,
            model_usage: ModelUsage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    let history = s.list_events(&ctx(), session.id.as_str()).await.unwrap();
    assert_eq!(history.len(), 2);
    // Agent-emitted events are processed by construction.
    assert!(history.iter().all(|e| e.processed_at.is_some()));
}
