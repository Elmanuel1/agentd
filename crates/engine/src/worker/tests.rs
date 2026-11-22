use serde_json::{Value, json};
use sessions::events::{Event, EventId, EventPayload, ModelUsage, Rubric, SpanEvent, UserEvent};
use sessions::types::{OutcomeId, Session, ThreadId};
use shared::Timestamp;

use super::grade::{self, active};
use super::reply::{Call, led};
use super::stall::stalled;
use super::turn::Turn;
use crate::model::{Block, ModelResponse, ModelTool, StopKind};
use crate::turn::policy::{Disposition, LIST_AGENTS, Toolbox};

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
fn a_grader_grades_in_its_reply_and_the_round_ends_after_the_call_with_live_s_explanation() {
    // Live, 2026-11-12: no verdict tool; the round's end carries the explanation.
    let defined = Event {
        processed_at: Some(Timestamp::now()),
        ..Event::new(EventPayload::User(UserEvent::DefineOutcome {
            description: "Write a poem.".into(),
            rubric: Rubric::Text { content: "### Rain\nAbout rain.".into() },
            max_iterations: None,
            outcome_id: Some(OutcomeId::generate()),
        }))
    };
    let outcome = active(&[defined]).unwrap();
    let grade =
        json!({"result": "satisfied", "criteria": [{"criterion": "Rain", "met": true, "finding": "It is."}]});
    let graded = ModelResponse {
        content: vec![Block::text(grade.to_string())],
        stop: StopKind::EndTurn,
        ..reply(&[])
    };
    let events = lead(&Toolbox::grader(), &graded, Some(&outcome));
    assert_eq!(types(&events), ["agent.message", "span.model_request_end", "span.outcome_evaluation_end"]);
    assert_eq!(events[2]["result"], "satisfied");
    assert_eq!(
        events[2]["explanation"],
        "An independent grader found all criteria met:\n\n- Rain (met): It is."
    );
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

#[test]
fn a_reply_s_signed_thinking_is_kept_beside_its_agent_thinking() {
    let (turn, start, session) = (turn(), EventId::generate(), session());
    let thought = Block::Thinking { thinking: String::new(), signature: "sig".into() };
    let r = ModelResponse {
        content: vec![thought, Block::text("hi")],
        thought: true,
        stop: StopKind::EndTurn,
        ..reply(&[])
    };
    let tools = Toolbox::default();
    let call = Call {
        turn: &turn,
        start: &start,
        tools: &tools,
        cloud: false,
        more: false,
        session: &session,
        log: &[],
        grading: None,
    };
    let out = led(&call, &r);
    let thinking = out
        .events
        .iter()
        .find(|e| matches!(e.payload, EventPayload::Agent(sessions::events::AgentEvent::Thinking { .. })))
        .unwrap();
    assert_eq!(
        out.engine,
        vec![(
            thinking.id.clone(),
            json!({"thinking": [{"type": "thinking", "thinking": "", "signature": "sig"}]})
        )]
    );
    assert!(
        serde_json::to_value(thinking).unwrap().get("content").is_none(),
        "live's agent.thinking carries no content"
    );
}

#[test]
fn the_thread_that_waited_longest_takes_the_step() {
    // Two delegates both await the model: the one that just had a step
    // waits, so a thread whose calls come back fast cannot starve the other.
    use agents::types::{Multiagent, RosterMember};
    use sessions::events::{AgentEvent, SessionEvent};
    use shared::ContentBlock;
    let mut session = session();
    let member = |name: &str| {
        let mut a = session.agent.clone();
        a.config.name = name.into();
        RosterMember::Snapshot(Box::new(a))
    };
    let roster = vec![member("fast"), member("slow")];
    session.agent.config.multiagent = Some(Multiagent::Coordinator { agents: roster });
    let on = |thread: &ThreadId, payload: EventPayload| Event {
        processed_at: Some(Timestamp::now()),
        session_thread_id: Some(thread.clone()),
        ..Event::new(payload)
    };
    let message = |text: &str| AgentEvent::ThreadMessageReceived {
        content: vec![ContentBlock::text(text)],
        from_agent_name: "coord".into(),
        from_session_thread_id: ThreadId::generate(),
    };
    let (fast, slow) = (ThreadId::generate(), ThreadId::generate());
    let log = vec![
        on(&fast, SessionEvent::ThreadCreated { agent_name: "fast".into() }.into()),
        on(&slow, SessionEvent::ThreadCreated { agent_name: "slow".into() }.into()),
        on(&fast, message("go").into()),
        on(&slow, message("go").into()),
        on(&fast, SpanEvent::ModelRequestStart {}.into()),
        on(&fast, AgentEvent::Message { content: vec![ContentBlock::text("on it")] }.into()),
        on(&fast, message("more").into()),
    ];
    let taken = super::take::take(&log, &session, false, &Default::default(), &Default::default());
    assert_eq!(taken.thread.id, slow);
    assert!(taken.more, "the fast one still awaits the model, so the step is not the last");
}

#[test]
fn a_thread_that_waits_while_another_is_ready_idles_alone() {
    use sessions::events::StopReason;
    use sessions::steps::StepEnd;
    use sessions::types::SessionStatus;
    let led = || super::reply::Led {
        events: Vec::new(),
        end: StepEnd::Done,
        idle: Some(StopReason::RequiresAction { event_ids: Vec::new() }),
        evaluation: None,
        engine: Vec::new(),
    };
    let mut session = session();
    session.status = SessionStatus::Running;
    let thread = super::team::Thread::primary(&session);
    let turn = thread.turn();
    let r = reply(&[]);
    let mut out = led();
    super::settle::settle(&mut out, &turn, &thread, &mut session, &[], &r, true);
    assert!(matches!(out.end, StepEnd::Again), "another thread is ready, so the step runs again");
    assert_eq!(session.status, SessionStatus::Running);
    assert_eq!(types(&wire(&out.events)), ["session.thread_status_idle"]);
    let mut out = led();
    super::settle::settle(&mut out, &turn, &thread, &mut session, &[], &r, false);
    assert_eq!(session.status, SessionStatus::Idle, "with no other thread to run, the session idles");
}
