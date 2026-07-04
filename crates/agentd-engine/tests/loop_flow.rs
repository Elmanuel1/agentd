//! Engine loop tests: scripted model + real jailed tools + the REAL
//! sessions service as the engine contract — the full task-side stack.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_agents::AgentSnapshot;
use agentd_core::{ApiError, AuthContext, OrgId, WorkspaceId};
use agentd_engine::model::{MockModel, ModelOutput, ModelResponse};
use agentd_engine::outcome::{GradeVerdict, Grader, OutcomeLoop};
use agentd_engine::tools::WorkspaceTools;
use agentd_engine::turn::{TurnEnd, TurnRunner};
use agentd_sessions::events::{EventPayload, ModelUsage, Rubric};
use agentd_sessions::service::{SessionDeps, SessionsService};
use agentd_sessions::store::InMemorySessionsStore;
use agentd_sessions::types::{OutcomeResult, Session, SessionStatus};
use async_trait::async_trait;

struct FakeDeps;

#[async_trait]
impl SessionDeps for FakeDeps {
    async fn resolve_agent(
        &self,
        _ctx: &AuthContext,
        _id: &str,
        _version: Option<u64>,
    ) -> Result<AgentSnapshot, ApiError> {
        let agent: agentd_agents::Agent =
            serde_json::from_str(include_str!("../../../fixtures/agent.create.json")).unwrap();
        Ok(agent.snapshot())
    }
    async fn check_environment(&self, _ctx: &AuthContext, _env: &str) -> Result<(), ApiError> {
        Ok(())
    }
}

fn ctx() -> AuthContext {
    AuthContext {
        org: OrgId("org_t".into()),
        workspace: WorkspaceId("wrkspc_01t".into()),
        api_key_id: "apikey_01t".into(),
    }
}

struct Rig {
    sessions: Arc<SessionsService>,
    session: Session,
    workspace: tempfile::TempDir,
}

async fn rig() -> Rig {
    let sessions = Arc::new(SessionsService::new(
        Arc::new(InMemorySessionsStore::new()),
        Arc::new(FakeDeps),
    ));
    let session = sessions
        .create(
            &ctx(),
            serde_json::from_str(
                r#"{"agent":"agent_01XdReZgC7eHpg9b9Nvrwdcm","environment_id":"env_01ok"}"#,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("notes.md"), "the answer is 42\n").unwrap();
    Rig {
        sessions,
        session,
        workspace,
    }
}

fn runner(r: &Rig, model: Arc<MockModel>) -> TurnRunner {
    TurnRunner::new(
        r.sessions.clone(),
        model,
        Arc::new(WorkspaceTools::new(r.workspace.path())),
        ctx().workspace,
        r.session.id.clone(),
        r.session.agent.clone(),
    )
}

fn text(t: &str) -> ModelResponse {
    ModelResponse {
        outputs: vec![ModelOutput::Text { text: t.into() }],
        usage: ModelUsage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
    }
}

fn tool_use(name: &str, input: serde_json::Value) -> ModelResponse {
    ModelResponse {
        outputs: vec![ModelOutput::ToolUse {
            name: name.into(),
            input,
        }],
        usage: ModelUsage {
            input_tokens: 20,
            output_tokens: 8,
            ..Default::default()
        },
    }
}

async fn send_user(r: &Rig, text: &str) {
    r.sessions
        .send_events(
            &ctx(),
            r.session.id.as_str(),
            vec![serde_json::from_value(serde_json::json!({
                "type":"user.message","content":[{"type":"text","text":text}]
            }))
            .unwrap()],
        )
        .await
        .unwrap();
}

fn event_types(events: &[agentd_sessions::events::Event]) -> Vec<String> {
    events
        .iter()
        .map(|e| {
            serde_json::to_value(e).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn text_only_turn_full_event_sequence() {
    let r = rig().await;
    send_user(&r, "hello").await;
    let model = Arc::new(MockModel::scripted(vec![text("Hi! How can I help?")]));
    let mut runner = runner(&r, model.clone());
    let end = runner.run_turn().await.unwrap();
    assert_eq!(end, TurnEnd::Completed);

    let events = r.sessions.list_events(&ctx(), r.session.id.as_str()).await.unwrap();
    let types = event_types(&events);
    assert_eq!(
        types,
        vec![
            "user.message",           // queued (stamped processed on drain)
            "session.status_running",
            "span.model_request_start",
            "span.model_request_end",
            "agent.message",
            "session.status_idle",
        ]
    );
    // The model saw the drained user message.
    assert_eq!(model.requests()[0].messages[0].content, "hello");
    // Usage accumulated onto the session.
    let s = r.sessions.get(&ctx(), r.session.id.as_str()).await.unwrap();
    assert_eq!(s.usage.input_tokens, 10);
    assert_eq!(s.usage.output_tokens, 5);
    assert_eq!(s.status, SessionStatus::Idle);
}

#[tokio::test]
async fn tool_loop_executes_and_feeds_result_back() {
    let r = rig().await;
    send_user(&r, "read my notes").await;
    let model = Arc::new(MockModel::scripted(vec![
        tool_use("read", serde_json::json!({"path":"notes.md"})),
        text("Your notes say the answer is 42."),
    ]));
    let mut runner = runner(&r, model.clone());
    assert_eq!(runner.run_turn().await.unwrap(), TurnEnd::Completed);

    let events = r.sessions.list_events(&ctx(), r.session.id.as_str()).await.unwrap();
    let types = event_types(&events);
    assert_eq!(
        types,
        vec![
            "user.message",
            "session.status_running",
            "span.model_request_start",
            "span.model_request_end",
            "agent.tool_use",
            "agent.tool_result",
            "span.model_request_start",
            "span.model_request_end",
            "agent.message",
            "session.status_idle",
        ]
    );
    // Second model call carried the tool result.
    let second = &model.requests()[1];
    assert!(second
        .messages
        .iter()
        .any(|m| m.content.contains("the answer is 42")));
    // Usage accumulated across BOTH calls.
    let s = r.sessions.get(&ctx(), r.session.id.as_str()).await.unwrap();
    assert_eq!(s.usage.input_tokens, 30);
}

#[tokio::test]
async fn always_ask_parks_then_confirmation_resumes() {
    let r = rig().await;
    // Arm the agent with an always_ask policy on `write`.
    let mut agent = r.session.agent.clone();
    agent.tools = serde_json::from_value(serde_json::json!([{
        "type":"agent_toolset_20260401",
        "default_config":{"enabled":true,"permission_policy":{"type":"always_allow"}},
        "configs":[{"name":"write","permission_policy":{"type":"always_ask"}}]
    }]))
    .unwrap();
    let model = Arc::new(MockModel::scripted(vec![
        tool_use("write", serde_json::json!({"path":"out.txt","content":"data"})),
        text("Written."),
    ]));
    let mut runner = TurnRunner::new(
        r.sessions.clone(),
        model,
        Arc::new(WorkspaceTools::new(r.workspace.path())),
        ctx().workspace,
        r.session.id.clone(),
        agent,
    );
    send_user(&r, "write the file").await;
    // Turn parks awaiting confirmation.
    let end = runner.run_turn().await.unwrap();
    let TurnEnd::AwaitingAction { blocking_event } = end else {
        panic!("expected AwaitingAction, got {end:?}");
    };
    // The idle event carries requires_action with the blocking id.
    let events = r.sessions.list_events(&ctx(), r.session.id.as_str()).await.unwrap();
    let idle = events
        .iter()
        .find_map(|e| match &e.payload {
            EventPayload::SessionStatusIdle { stop_reason } => Some(stop_reason.clone()),
            _ => None,
        })
        .unwrap();
    assert!(!idle.is_terminal(), "requires_action is NOT terminal");
    // File was NOT written yet.
    assert!(!r.workspace.path().join("out.txt").exists());

    // Client allows → resume → tool executes → completes.
    r.sessions
        .send_events(
            &ctx(),
            r.session.id.as_str(),
            vec![serde_json::from_value(serde_json::json!({
                "type":"user.tool_confirmation",
                "tool_use_id": blocking_event.as_str(),
                "result":"allow"
            }))
            .unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(runner.run_turn().await.unwrap(), TurnEnd::Completed);
    assert!(r.workspace.path().join("out.txt").exists(), "tool ran after allow");
}

#[tokio::test]
async fn deny_feeds_denial_to_model_and_tool_never_runs() {
    let r = rig().await;
    let mut agent = r.session.agent.clone();
    agent.tools = serde_json::from_value(serde_json::json!([{
        "type":"agent_toolset_20260401",
        "default_config":{"enabled":true,"permission_policy":{"type":"always_ask"}}
    }]))
    .unwrap();
    let model = Arc::new(MockModel::scripted(vec![
        tool_use("write", serde_json::json!({"path":"secret.txt","content":"x"})),
        text("Understood, not writing."),
    ]));
    let mut runner = TurnRunner::new(
        r.sessions.clone(),
        model.clone(),
        Arc::new(WorkspaceTools::new(r.workspace.path())),
        ctx().workspace,
        r.session.id.clone(),
        agent,
    );
    send_user(&r, "write it").await;
    let TurnEnd::AwaitingAction { blocking_event } = runner.run_turn().await.unwrap() else {
        panic!("expected park");
    };
    r.sessions
        .send_events(
            &ctx(),
            r.session.id.as_str(),
            vec![serde_json::from_value(serde_json::json!({
                "type":"user.tool_confirmation",
                "tool_use_id": blocking_event.as_str(),
                "result":"deny",
                "deny_message":"write to public.txt instead"
            }))
            .unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(runner.run_turn().await.unwrap(), TurnEnd::Completed);
    assert!(!r.workspace.path().join("secret.txt").exists());
    // Model saw the deny message.
    let last = model.requests().last().unwrap().clone();
    assert!(last
        .messages
        .iter()
        .any(|m| m.content.contains("write to public.txt instead")));
}

#[tokio::test]
async fn custom_tool_parks_then_result_resumes() {
    let r = rig().await;
    let mut agent = r.session.agent.clone();
    agent.tools = serde_json::from_value(serde_json::json!([
        {"type":"agent_toolset_20260401"},
        {"type":"custom","name":"linear_graphql","description":"query linear",
         "input_schema":{"type":"object","properties":{"query":{"type":"string"}}}}
    ]))
    .unwrap();
    let model = Arc::new(MockModel::scripted(vec![
        tool_use("linear_graphql", serde_json::json!({"query":"{ issues }"})),
        text("There are 3 open issues."),
    ]));
    let mut runner = TurnRunner::new(
        r.sessions.clone(),
        model.clone(),
        Arc::new(WorkspaceTools::new(r.workspace.path())),
        ctx().workspace,
        r.session.id.clone(),
        agent,
    );
    send_user(&r, "how many issues?").await;
    let TurnEnd::AwaitingAction { blocking_event } = runner.run_turn().await.unwrap() else {
        panic!("expected park on custom tool");
    };
    // Host executes with its own credentials, returns the result.
    r.sessions
        .send_events(
            &ctx(),
            r.session.id.as_str(),
            vec![serde_json::from_value(serde_json::json!({
                "type":"user.custom_tool_result",
                "custom_tool_use_id": blocking_event.as_str(),
                "content":[{"type":"text","text":"{\"issues\":3}"}],
                "is_error":false
            }))
            .unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(runner.run_turn().await.unwrap(), TurnEnd::Completed);
    let last = model.requests().last().unwrap().clone();
    assert!(last.messages.iter().any(|m| m.content.contains("{\"issues\":3}")));
}

#[tokio::test]
async fn interrupt_halts_without_model_call() {
    let r = rig().await;
    r.sessions
        .send_events(
            &ctx(),
            r.session.id.as_str(),
            vec![serde_json::from_value(serde_json::json!({"type":"user.interrupt"})).unwrap()],
        )
        .await
        .unwrap();
    let model = Arc::new(MockModel::scripted(vec![])); // ZERO responses armed
    let mut runner = runner(&r, model.clone());
    assert_eq!(runner.run_turn().await.unwrap(), TurnEnd::Interrupted);
    assert!(model.requests().is_empty(), "no model call on interrupt");
}

// ── outcome loop ─────────────────────────────────────────────────────────

/// Grader that needs N revisions before satisfaction.
struct CountdownGrader {
    revisions: std::sync::atomic::AtomicU32,
}

#[async_trait]
impl Grader for CountdownGrader {
    async fn grade(
        &self,
        _description: &str,
        _rubric: &Rubric,
        _iteration: u32,
    ) -> Result<GradeVerdict, ApiError> {
        let left = self.revisions.load(std::sync::atomic::Ordering::SeqCst);
        if left == 0 {
            Ok(GradeVerdict {
                result: OutcomeResult::Satisfied,
                explanation: "all criteria met".into(),
                usage: ModelUsage::default(),
            })
        } else {
            self.revisions
                .store(left - 1, std::sync::atomic::Ordering::SeqCst);
            Ok(GradeVerdict {
                result: OutcomeResult::NeedsRevision,
                explanation: "missing tests".into(),
                usage: ModelUsage::default(),
            })
        }
    }
}

#[tokio::test]
async fn outcome_loop_iterates_until_satisfied() {
    let r = rig().await;
    send_user(&r, "build it").await;
    // 2 revisions then satisfied → 3 work turns.
    let model = Arc::new(MockModel::scripted(vec![
        text("attempt 1"),
        text("attempt 2"),
        text("attempt 3"),
    ]));
    let mut work = runner(&r, model);
    let outcome_loop = OutcomeLoop::new(
        r.sessions.clone(),
        Arc::new(CountdownGrader {
            revisions: std::sync::atomic::AtomicU32::new(2),
        }),
        ctx().workspace,
        r.session.id.clone(),
    );
    let oid = agentd_sessions::types::OutcomeId::generate();
    let result = outcome_loop
        .run(
            &mut work,
            oid.clone(),
            "build the model",
            &Rubric::Text {
                content: "has tests".into(),
            },
            Some(5),
        )
        .await
        .unwrap();
    assert_eq!(result, OutcomeResult::Satisfied);
    // Session records the final verdict.
    let s = r.sessions.get(&ctx(), r.session.id.as_str()).await.unwrap();
    assert_eq!(s.outcome_evaluations.len(), 1);
    assert_eq!(s.outcome_evaluations[0].result, OutcomeResult::Satisfied);
    assert_eq!(s.outcome_evaluations[0].iteration, 2);
    // Grader span events journaled (start+end per iteration).
    let events = r.sessions.list_events(&ctx(), r.session.id.as_str()).await.unwrap();
    let starts = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::SpanOutcomeEvaluationStart { .. }))
        .count();
    assert_eq!(starts, 3);
}

#[tokio::test]
async fn outcome_loop_caps_at_max_iterations() {
    let r = rig().await;
    send_user(&r, "build it").await;
    let model = Arc::new(MockModel::scripted(vec![text("a1"), text("a2")]));
    let mut work = runner(&r, model);
    // Grader never satisfied.
    let outcome_loop = OutcomeLoop::new(
        r.sessions.clone(),
        Arc::new(CountdownGrader {
            revisions: std::sync::atomic::AtomicU32::new(u32::MAX),
        }),
        ctx().workspace,
        r.session.id.clone(),
    );
    let result = outcome_loop
        .run(
            &mut work,
            agentd_sessions::types::OutcomeId::generate(),
            "impossible task",
            &Rubric::Text {
                content: "perfection".into(),
            },
            Some(2),
        )
        .await
        .unwrap();
    assert_eq!(result, OutcomeResult::MaxIterationsReached);
}
