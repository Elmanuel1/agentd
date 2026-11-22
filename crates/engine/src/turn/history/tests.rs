use serde_json::json;
use sessions::events::{EventId, ModelUsage, SpanModelUsage};
use shared::Timestamp;

use super::*;
use sessions::types::OutcomeResult;

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
        model_usage: SpanModelUsage::default(),
    })
}

fn build(events: &[Event]) -> History {
    rebuild(&events.iter().collect::<Vec<_>>(), &HashMap::new(), &HashMap::new())
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
fn an_allowed_built_in_call_waits_for_the_session_s_worker() {
    let s1 = start();
    let call = tool_use("bash", "allow");
    let h = build(&[user("go"), s1.clone(), end(&s1), call]);
    assert_eq!(h.unanswered.len(), 1);
    assert!(h.unanswered[0].for_worker());
    assert!(!h.awaits_model());
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
    let kept = HashMap::from([(sent.id.clone(), json!({"opened": true}))]);
    let log = [user("go"), s1.clone(), end(&s1), sent, got];
    let h = rebuild(&log.iter().collect::<Vec<_>>(), &HashMap::new(), &kept);
    assert!(
        matches!(&h.messages[1].content[0], Block::ToolUse { name, .. } if name == delegation::CREATE_AGENT)
    );
    let last = h.messages.last().unwrap();
    assert!(matches!(&last.content[0], Block::ToolResult { is_error: false, .. }));
    assert!(
        matches!(&last.content[1], Block::Text { text } if text.starts_with("<agent-notification thread_id="))
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
    let h = rebuild(&mine, &HashMap::new(), &HashMap::new());
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
    let h = rebuild(&[&define], &rubrics, &HashMap::new());
    assert!(matches!(&h.messages[0].content[0], Block::Text { text } if text.contains("### 1. name")));
    let h = rebuild(&[&define, &feedback], &rubrics, &HashMap::new());
    assert!(matches!(&h.messages[0].content[1], Block::Text { text } if text.starts_with("name is wrong")));
}

#[test]
fn the_primary_thread_leaves_out_answers_to_another_threads_calls() {
    let sub = ThreadId::generate();
    let custom = on(&sub, AgentEvent::CustomToolUse { name: "lookup".into(), input: json!({}) });
    let answer = ev(UserEvent::CustomToolResult {
        custom_tool_use_id: EventId::parse(custom.id.as_str()).unwrap(),
        content: vec![ContentBlock::text("42")],
        is_error: false,
    });
    let all = vec![user("go"), custom, answer];
    let primary = thread_events(&all, None);
    assert_eq!(primary.len(), 1);
}

mod inputs;
mod interrupts;
mod thinking;

#[test]
fn a_coordinator_s_sends_replay_as_live_s_tools_with_their_results() {
    // Live, 2026-11-21: `create_agent` opens a thread and returns its id;
    // `send_to_agent` takes that id.
    let thread = ThreadId::generate();
    let sent = |text: &str| {
        ev(AgentEvent::ThreadMessageSent {
            content: vec![ContentBlock::text(text)],
            to_agent_name: Some("helper".into()),
            to_session_thread_id: thread.clone(),
        })
    };
    let (first, again) = (sent("do it"), sent("fix it"));
    let kept = HashMap::from([
        (first.id.clone(), json!({"opened": true})),
        (again.id.clone(), json!({"opened": false})),
    ]);
    let h = rebuild(&[&first, &again], &HashMap::new(), &kept);
    let blocks: Vec<&Block> = h.messages.iter().flat_map(|m| &m.content).collect();
    let calls: Vec<(&str, &serde_json::Value)> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::ToolUse { name, input, .. } => Some((name.as_str(), input)),
            _ => None,
        })
        .collect();
    assert_eq!(calls[0], ("create_agent", &json!({"agent_name": "helper", "task": "do it"})));
    assert_eq!(calls[1], ("send_to_agent", &json!({"thread_id": thread, "message": "fix it"})));
    let results: Vec<serde_json::Value> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::ToolResult { content, .. } => serde_json::from_str(content).ok(),
            _ => None,
        })
        .collect();
    assert_eq!((&results[0]["thread_id"], &results[0]["agent_name"]), (&json!(thread), &json!("helper")));
    assert_eq!((&results[1]["delivered"], &results[1]["queued"]), (&json!(true), &json!(false)));
}

#[test]
fn a_follow_up_whose_opening_send_was_compacted_replays_as_send_to_agent() {
    // The loop keeps beside each send whether it opened its thread, so a
    // follow-up still replays as one with its thread's first send gone.
    let thread = ThreadId::generate();
    let again = ev(AgentEvent::ThreadMessageSent {
        content: vec![ContentBlock::text("fix it")],
        to_agent_name: Some("helper".into()),
        to_session_thread_id: thread.clone(),
    });
    let kept = HashMap::from([(again.id.clone(), json!({"opened": false}))]);
    let h = rebuild(&[&again], &HashMap::new(), &kept);
    let call = h.messages.iter().flat_map(|m| &m.content).find_map(|b| match b {
        Block::ToolUse { name, input, .. } => Some((name.as_str(), input)),
        _ => None,
    });
    assert_eq!(call, Some(("send_to_agent", &json!({"thread_id": thread, "message": "fix it"}))));
}
