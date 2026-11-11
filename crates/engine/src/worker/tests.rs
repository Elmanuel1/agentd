use serde_json::{json, Value};
use sessions::events::{Event, EventId, EventPayload, ModelUsage, Rubric, SpanEvent, UserEvent};
use sessions::types::{OutcomeId, Session, ThreadId};
use shared::Timestamp;

use super::grade::{self, active, VERDICT};
use super::reply::{led, Call};
use super::stall::stalled;
use super::turn::Turn;
use crate::model::{Block, ModelResponse, ModelTool, StopKind};
use crate::turn::policy::{Disposition, Toolbox, LIST_AGENTS};

const SESSION: &str = include_str!("../../../../conformance/fixtures/session.create.json");

fn session() -> Session {
    serde_json::from_str(SESSION).unwrap()
}

fn turn() -> Turn {
    Turn { thread: ThreadId::generate(), agent: "a".into(), scoped: true }
}

fn tool(name: &str) -> ModelTool {
    ModelTool { name: name.into(), description: "d".into(), input_schema: json!({"type": "object"}) }
}

fn reply(calls: &[&str]) -> ModelResponse {
    let mut content = vec![Block::text("on it")];
    let uses = calls.iter().enumerate().map(|(i, name)| Block::ToolUse {
        id: format!("toolu_{i}"),
        name: (*name).into(),
        input: json!({"result": "satisfied", "findings": []}),
    });
    content.extend(uses);
    ModelResponse {
        content,
        stop: StopKind::ToolUse,
        usage: ModelUsage::default(),
        thought: false,
        cost: None,
    }
}

fn wire(events: &[Event]) -> Vec<Value> {
    events.iter().map(|e| serde_json::to_value(e).unwrap()).collect()
}

fn types(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap()).collect()
}

/// `led` for `r`, with `tools`, grading `grading` when given.
fn lead(tools: &Toolbox, r: &ModelResponse, grading: Option<&grade::Outcome>) -> Vec<Value> {
    let (turn, start, session) = (turn(), EventId::generate(), session());
    let call = Call {
        turn: &turn,
        start: &start,
        tools,
        cloud: false,
        more: false,
        session: &session,
        log: &[],
        grading,
    };
    wire(&led(&call, r).events)
}

#[test]
fn a_reply_s_calls_come_before_its_end_and_what_they_lead_to_after_it() {
    // Live's poll replay, 2026-11-11: the message, the call, then the reply's end.
    let mut tools = Toolbox::default();
    tools.add(tool("bash"), Disposition::Ask { evaluation: json!({"type": "always_ask"}) });
    tools.add(tool(LIST_AGENTS), Disposition::Delegate);
    let events = lead(&tools, &reply(&["bash", "nope", LIST_AGENTS]), None);
    assert_eq!(
        types(&events),
        [
            "agent.message",
            "agent.tool_use",
            "agent.tool_use",
            "agent.tool_use",
            "span.model_request_end",
            "agent.tool_result",
            "agent.tool_result"
        ]
    );
    assert_eq!(
        (events[1]["evaluated_permission"].clone(), events[1]["evaluation"].clone()),
        (json!("ask"), json!({"type": "always_ask"}))
    );
    assert_eq!(events[4]["model_usage"]["speed"], "standard");
    let answered: Vec<&Value> = events[5..].iter().map(|e| &e["tool_use_id"]).collect();
    assert_eq!(answered, [&events[2]["id"], &events[3]["id"]], "the unknown tool's, then the roster's");
    assert_eq!(events[5]["is_error"], true);
}

#[test]
fn a_verdict_call_comes_before_the_grader_s_end_and_the_round_s_end_after_it() {
    let defined = Event {
        processed_at: Some(Timestamp::now()),
        ..Event::new(EventPayload::User(UserEvent::DefineOutcome {
            description: "Write a poem.".into(),
            rubric: Rubric::Text { content: "Warm.".into() },
            max_iterations: None,
            outcome_id: Some(OutcomeId::generate()),
        }))
    };
    let outcome = active(&[defined]).unwrap();
    let events = lead(&grade::tools(), &reply(&[VERDICT]), Some(&outcome));
    assert_eq!(
        types(&events),
        [
            "agent.message",
            "agent.tool_use",
            "span.model_request_end",
            "agent.tool_result",
            "span.outcome_evaluation_end"
        ]
    );
    assert_eq!(events[4]["result"], "satisfied");
}

#[test]
fn a_grader_stalls_after_three_calls_with_no_tool_call() {
    let (turn, start) = (turn(), EventId::generate());
    let end = || {
        turn.event(SpanEvent::ModelRequestEnd {
            model_request_start_id: start.clone(),
            is_error: false,
            model_usage: Default::default(),
        })
    };
    let call = || {
        turn.event(sessions::events::AgentEvent::ToolUse {
            name: "read".into(),
            input: json!({}),
            evaluated_permission: Some("allow".into()),
            evaluation: None,
        })
    };
    let mut log = vec![call(), end(), end(), end(), end()];
    assert!(!stalled(&log[..4], &turn.thread), "a call's tool call comes before its end");
    assert!(stalled(&log, &turn.thread));
    log.insert(3, call());
    assert!(!stalled(&log, &turn.thread));
}
