//! One agent: tool steps, confirmations, custom tools, outcomes, interrupt,
//! the lease, provisioning and outputs.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use sessions::events::{
    AgentEvent, ConfirmationResult, EventPayload, Rubric, SpanEvent, StopReason, UserEvent,
};
use sessions::types::{OutcomeResult, SessionStatus};
use shared::ports::lease::Leases;
use shared::ContentBlock;

use super::*;
use crate::model::{ModelError, ModelRequest};

#[tokio::test]
async fn a_message_runs_a_tool_step_and_ends_the_turn() {
    let s = session(agent("worker", toolset("auto"), Value::Null));
    let (rig, mock) = Rig::scripted(
        s,
        vec![("*", vec![call("bash", json!({"command": "ls"})), say("two files")])],
        answering("a\nb"),
    );
    rig.send(message("list the files")).await;
    let types = rig.session.types();
    let expected = [
        "user.message",
        "session.status_running",
        "span.model_request_start",
        "agent.thinking",
        "span.model_request_end",
        "agent.tool_use",
        "agent.tool_result",
        "span.model_request_start",
        "agent.message",
        "span.model_request_end",
        "session.status_idle",
    ];
    assert_eq!(types, expected);
    assert_eq!(rig.last_stop(), StopReason::EndTurn {});
    assert_eq!(rig.session.get().status, SessionStatus::Idle);
    assert_eq!(rig.session.get().usage.input_tokens, 10);
    let tool_use = rig
        .session
        .log()
        .into_iter()
        .find(|e| matches!(e.payload, EventPayload::Agent(AgentEvent::ToolUse { .. })))
        .unwrap();
    let wire = serde_json::to_value(&tool_use).unwrap();
    assert_eq!(wire["evaluation"], json!({"type": "auto", "evaluated_permission": {"type": "allow"}}));
    let second = &mock.requests()[1];
    assert!(
        matches!(&second.messages[2].content[0], Block::ToolResult { content, .. } if content.starts_with("a\nb"))
    );
    assert!(second.system.contains("/workspace/skills/citing-sources/SKILL.md"));
}

#[tokio::test]
async fn an_always_ask_tool_waits_for_confirmation_then_runs() {
    let s = session(agent("worker", toolset("always_ask"), Value::Null));
    let (rig, _) = Rig::scripted(
        s,
        vec![("*", vec![call("bash", json!({"command": "rm x"})), say("removed")])],
        answering("ok"),
    );
    rig.send(message("remove x")).await;
    let StopReason::RequiresAction { event_ids } = rig.last_stop() else { panic!("{:?}", rig.last_stop()) };
    assert_eq!(event_ids.len(), 1);
    assert!(!rig.session.types().contains(&"agent.tool_result".to_owned()));
    rig.send(UserEvent::ToolConfirmation {
        tool_use_id: event_ids[0].clone(),
        result: ConfirmationResult::Allow,
        deny_message: None,
    })
    .await;
    assert!(rig.session.types().contains(&"agent.tool_result".to_owned()));
    assert_eq!(rig.messages(), ["removed"]);
    assert_eq!(rig.last_stop(), StopReason::EndTurn {});
}

#[tokio::test]
async fn a_denied_tool_tells_the_model_why() {
    let s = session(agent("worker", toolset("always_ask"), Value::Null));
    let (rig, mock) = Rig::scripted(
        s,
        vec![("*", vec![call("bash", json!({"command": "rm x"})), say("ok, kept it")])],
        answering(""),
    );
    rig.send(message("remove x")).await;
    let StopReason::RequiresAction { event_ids } = rig.last_stop() else { panic!() };
    rig.send(UserEvent::ToolConfirmation {
        tool_use_id: event_ids[0].clone(),
        result: ConfirmationResult::Deny,
        deny_message: Some("keep it".into()),
    })
    .await;
    let last = mock.requests().last().unwrap().messages.last().unwrap().clone();
    assert!(
        matches!(&last.content[0], Block::ToolResult { content, is_error: true, .. } if content == "keep it")
    );
    assert_eq!(rig.sandbox.execs().iter().filter(|e| e.argv[0] == "sh").count(), 0);
}

#[tokio::test]
async fn a_custom_tool_waits_for_the_clients_result() {
    let tools =
        json!([{"type": "custom", "name": "lookup", "description": "d", "input_schema": {"type": "object"}}]);
    let s = session(agent("worker", tools, Value::Null));
    let (rig, mock) =
        Rig::scripted(s, vec![("*", vec![call("lookup", json!({"q": 1})), say("it is 42")])], answering(""));
    rig.send(message("look it up")).await;
    let StopReason::RequiresAction { event_ids } = rig.last_stop() else { panic!() };
    assert!(rig.session.types().contains(&"agent.custom_tool_use".to_owned()));
    rig.send(UserEvent::CustomToolResult {
        custom_tool_use_id: event_ids[0].clone(),
        content: vec![ContentBlock::text("42")],
        is_error: false,
    })
    .await;
    assert_eq!(rig.messages(), ["it is 42"]);
    let last = mock.requests().last().unwrap().messages.last().unwrap().clone();
    assert!(matches!(&last.content[0], Block::ToolResult { content, .. } if content == "42"));
}

#[tokio::test]
async fn an_outcome_is_graded_and_revised_until_satisfied() {
    let s = session(agent("worker", toolset("auto"), Value::Null));
    let verdict = |r: &str| {
        call(
            "submit_verdict",
            json!({"result": r, "findings": [{"criterion": "1. name", "finding": "wrong on p.1"}]}),
        )
    };
    let scripts = vec![
        ("*", vec![say("v1"), say("v2")]),
        ("grader", vec![verdict("needs_revision"), verdict("satisfied")]),
    ];
    let (rig, mock) = Rig::scripted(s, scripts, answering(""));
    rig.send(UserEvent::DefineOutcome {
        description: "write result.json".into(),
        rubric: Rubric::Text { content: "### 1. name".into() },
        max_iterations: Some(3),
        outcome_id: Some(sessions::types::OutcomeId::generate()),
    })
    .await;
    let ends: Vec<OutcomeResult> = rig
        .session
        .log()
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::Span(SpanEvent::OutcomeEvaluationEnd { result, .. }) => Some(*result),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [OutcomeResult::NeedsRevision, OutcomeResult::Satisfied]);
    assert_eq!(rig.messages(), ["v1", "v2"]);
    assert_eq!(rig.session.get().outcome_evaluations[0].result, OutcomeResult::Satisfied);
    let revised = mock.requests().into_iter().filter(|r| r.agent == "worker").nth(1).unwrap();
    assert!(
        matches!(&revised.messages.last().unwrap().content[0], Block::Text { text } if text.contains("wrong on p.1"))
    );
    assert!(!rig.session.types().iter().any(|t| t == "agent.tool_use"));
}

#[tokio::test]
async fn the_last_round_without_a_pass_gets_one_acknowledgement_turn() {
    let s = session(agent("worker", toolset("auto"), Value::Null));
    let fail = call("submit_verdict", json!({"result": "needs_revision", "findings": []}));
    let scripts = vec![("*", vec![say("v1"), say("noted")]), ("grader", vec![fail])];
    let (rig, _) = Rig::scripted(s, scripts, answering(""));
    rig.send(UserEvent::DefineOutcome {
        description: "d".into(),
        rubric: Rubric::Text { content: "r".into() },
        max_iterations: Some(1),
        outcome_id: Some(sessions::types::OutcomeId::generate()),
    })
    .await;
    assert_eq!(rig.session.get().outcome_evaluations[0].result, OutcomeResult::MaxIterationsReached);
    assert_eq!(rig.messages(), ["v1", "noted"]);
}

/// A model that never answers, so a turn is mid-call when interrupted.
struct Hang;

#[async_trait]
impl ModelClient for Hang {
    async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, ModelError> {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        Err(ModelError::Fatal("hung".into()))
    }
}

#[tokio::test]
async fn an_interrupt_stops_the_turn_and_leaves_the_session_idle() {
    let s = session(agent("worker", toolset("auto"), Value::Null));
    let rig = Rig::new(s, Arc::new(Hang), answering(""));
    let id = rig.session.get().id;
    rig.session.send(message("think hard"));
    rig.dispatcher.wake(&rig.ctx, &id);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    rig.send(UserEvent::Interrupt {}).await;
    assert_eq!(rig.session.get().status, SessionStatus::Idle);
    assert_eq!(rig.last_stop(), StopReason::EndTurn {});
}

#[tokio::test]
async fn a_session_another_holder_leases_is_left_to_it() {
    let s = session(agent("worker", toolset("auto"), Value::Null));
    let (rig, _) = Rig::scripted(s, vec![("*", vec![say("hi")])], answering(""));
    let id = rig.session.get().id;
    let held = rig.leases.try_acquire(&format!("session:{id}")).await.unwrap();
    rig.send(message("hello")).await;
    assert!(rig.messages().is_empty());
    drop(held);
    rig.dispatcher.wake(&rig.ctx, &id);
    rig.idle().await;
    assert_eq!(rig.messages(), ["hi"]);
}

#[tokio::test]
async fn the_sandbox_is_provisioned_once_and_outputs_come_back_as_files() {
    let s = session(agent("worker", toolset("auto"), Value::Null));
    let (rig, _) = Rig::scripted(s, vec![("*", vec![say("one"), say("two")])], answering(""));
    rig.send(message("go")).await;
    let files = rig.sandbox.files();
    assert_eq!(files["/mnt/session/uploads/extraction/docs/a.json"], b"{\"pages\": 1}");
    assert!(files.contains_key("/workspace/skills/citing-sources/SKILL.md"));
    rig.sandbox.seed("/mnt/session/outputs/u0/result.json", b"{}");
    rig.send(message("again")).await;
    let outputs = rig.assets.outputs.lock().clone();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].filename, "result.json");
    assert!(rig.sandbox.dirs().iter().any(|(d, _)| d == "/mnt/session/outputs"));
}
