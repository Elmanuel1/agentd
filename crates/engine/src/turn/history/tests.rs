use serde_json::json;
use sessions::events::{EventId, ModelUsage};
use shared::Timestamp;

use super::*;

fn ev(payload: impl Into<EventPayload>) -> Event {
    let mut e = Event::new(payload.into());
    e.processed_at = Some(Timestamp::now());
    e
}

fn on(thread: &ThreadId, payload: impl Into<EventPayload>) -> Event {
    let mut e = ev(payload);
    e.session_thread_id = Some(thread.clone());
    e
}

fn user(text: &str) -> Event {
    ev(UserEvent::Message { content: vec![ContentBlock::text(text)] })
}

fn tool_use(name: &str, permission: &str) -> Event {
    ev(AgentEvent::ToolUse {
        name: name.into(),
        input: json!({"command": "ls"}),
        evaluated_permission: Some(permission.into()),
        evaluation: None,
    })
}

fn result(of: &Event, text: &str) -> Event {
    ev(AgentEvent::ToolResult {
        tool_use_id: of.id.clone(),
        content: vec![ContentBlock::text(text)],
        is_error: false,
    })
}

fn start() -> Event {
    ev(SpanEvent::ModelRequestStart {})
}

fn end(start: &Event) -> Event {
    ev(SpanEvent::ModelRequestEnd {
        model_request_start_id: start.id.clone(),
        is_error: false,
        model_usage: ModelUsage::default(),
    })
}

fn build(events: &[Event]) -> History {
    rebuild(&events.iter().collect::<Vec<_>>(), &HashMap::new())
}

#[test]
fn a_tool_step_replays_as_call_then_result_then_the_next_reply() {
    let s1 = start();
    let call = tool_use("bash", "allow");
    let s2 = start();
    let events = vec![
        user("list files"),
        s1.clone(),
        end(&s1),
        call.clone(),
        result(&call, "a b"),
        s2.clone(),
        ev(AgentEvent::Message { content: vec![ContentBlock::text("two files")] }),
        end(&s2),
    ];
    let h = build(&events);
    assert_eq!(h.messages.len(), 4);
    assert_eq!(h.messages[0], Message { role: Role::User, content: vec![Block::text("list files")] });
    assert!(matches!(&h.messages[1].content[0], Block::ToolUse { id, .. } if id == call.id.as_str()));
    assert!(matches!(&h.messages[2].content[0], Block::ToolResult { content, .. } if content == "a b"));
    assert_eq!(h.messages[3].role, Role::Assistant);
    assert!(!h.awaits_model());
}

#[test]
fn input_that_arrives_mid_step_follows_the_tool_results() {
    let s1 = start();
    let call = tool_use("bash", "allow");
    let events = vec![user("go"), s1.clone(), end(&s1), call.clone(), user("also this"), result(&call, "ok")];
    let h = build(&events);
    let last = h.messages.last().unwrap();
    assert!(matches!(last.content[0], Block::ToolResult { .. }));
    assert_eq!(last.content[1], Block::text("also this"));
    assert!(h.awaits_model());
}

#[test]
fn an_ask_without_a_result_is_owed_and_blocks_the_model() {
    let s1 = start();
    let ask = tool_use("bash", "ask");
    let h = build(&[user("go"), s1.clone(), end(&s1), ask.clone()]);
    assert_eq!(h.unanswered.len(), 1);
    assert_eq!(h.unanswered[0].id, ask.id.as_str());
    assert!(!h.unanswered[0].custom);
    assert!(!h.awaits_model());
}

#[test]
fn a_call_cut_off_by_a_crash_gets_an_error_result() {
    let s1 = start();
    let call = tool_use("bash", "allow");
    let h = build(&[user("go"), s1.clone(), end(&s1), call]);
    assert!(h.unanswered.is_empty());
    assert!(matches!(&h.messages.last().unwrap().content[0], Block::ToolResult { is_error: true, .. }));
    assert!(h.awaits_model());
}

#[test]
fn the_coordinator_replays_delegation_as_a_tool_call_and_the_reply_as_input() {
    let sub = ThreadId::generate();
    let s1 = start();
    let sent = ev(AgentEvent::ThreadMessageSent {
        content: vec![ContentBlock::text("extract it")],
        to_agent_name: Some("bid-extractor".into()),
        to_session_thread_id: sub.clone(),
    });
    let got = ev(AgentEvent::ThreadMessageReceived {
        content: vec![ContentBlock::text("done")],
        from_agent_name: "bid-extractor".into(),
        from_session_thread_id: sub.clone(),
    });
    let h = build(&[user("go"), s1.clone(), end(&s1), sent, got]);
    assert!(matches!(&h.messages[1].content[0], Block::ToolUse { name, .. } if name == SEND_TO_AGENT));
    let last = h.messages.last().unwrap();
    assert!(matches!(&last.content[0], Block::ToolResult { is_error: false, .. }));
    assert!(
        matches!(&last.content[1], Block::Text { text } if text.starts_with("Message from bid-extractor"))
    );
}

#[test]
fn a_secondary_thread_sees_its_own_events_and_the_answers_to_its_calls() {
    let sub = ThreadId::generate();
    let got = on(
        &sub,
        AgentEvent::ThreadMessageReceived {
            content: vec![ContentBlock::text("extract it")],
            from_agent_name: "orchestrator".into(),
            from_session_thread_id: ThreadId::generate(),
        },
    );
    let custom = on(&sub, AgentEvent::CustomToolUse { name: "lookup".into(), input: json!({}) });
    let answer = ev(UserEvent::CustomToolResult {
        custom_tool_use_id: EventId::parse(custom.id.as_str()).unwrap(),
        content: vec![ContentBlock::text("42")],
        is_error: false,
    });
    let step = on(&sub, SpanEvent::ModelRequestStart {});
    let all = vec![user("primary only"), got, step, custom, answer];
    let mine = thread_events(&all, Some(&sub));
    assert_eq!(mine.len(), 4);
    let h = rebuild(&mine, &HashMap::new());
    assert_eq!(h.messages[0], Message { role: Role::User, content: vec![Block::text("extract it")] });
    assert!(h.unanswered.is_empty());
    assert!(h.awaits_model());
}

#[test]
fn an_outcome_shows_its_rubric_and_a_failed_round_its_findings() {
    let define = ev(UserEvent::DefineOutcome {
        description: "write result.json".into(),
        rubric: Rubric::File { file_id: "file_1".into() },
        max_iterations: None,
        outcome_id: None,
    });
    let feedback = ev(SpanEvent::OutcomeEvaluationEnd {
        outcome_evaluation_start_id: EventId::generate(),
        outcome_id: sessions::types::OutcomeId::generate(),
        result: OutcomeResult::NeedsRevision,
        explanation: Some("name is wrong".into()),
        iteration: 0,
        usage: ModelUsage::default(),
    });
    let rubrics = HashMap::from([("file_1".to_owned(), "### 1. name".to_owned())]);
    let h = rebuild(&[&define], &rubrics);
    assert!(matches!(&h.messages[0].content[0], Block::Text { text } if text.contains("### 1. name")));
    let h = rebuild(&[&define, &feedback], &rubrics);
    assert!(matches!(&h.messages[0].content[1], Block::Text { text } if text.starts_with("name is wrong")));
}
