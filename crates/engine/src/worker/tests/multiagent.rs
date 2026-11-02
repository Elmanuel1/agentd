//! A coordinator and its roster: threads, their events, and replies.

use serde_json::json;
use sessions::events::{AgentEvent, EventPayload, SessionEvent};
use sessions::service::threads_projection as project;
use sessions::types::primary_thread_id;

use super::*;

fn coordinator() -> Session {
    let worker = agent("bid-extractor", toolset("auto"), Value::Null);
    session(agent("orchestrator", toolset("auto"), json!({"type": "coordinator", "agents": [worker]})))
}

#[tokio::test]
async fn a_coordinator_delegates_to_a_thread_and_hears_back() {
    let send = call("send_to_agent", json!({"agent_name": "bid-extractor", "message": "extract doc 1"}));
    let scripts = vec![
        ("orchestrator", vec![send, say("waiting for the extractor"), say("all done")]),
        (
            "bid-extractor",
            vec![call("bash", json!({"command": "python3 pages.py"})), say("extracted 12 fields")],
        ),
    ];
    let (rig, mock) = Rig::scripted(coordinator(), scripts, answering("page text"));
    rig.send(message("run the pipeline")).await;
    let log = rig.session.log();
    let created = log
        .iter()
        .find(|e| matches!(e.payload, EventPayload::Session(SessionEvent::ThreadCreated { .. })))
        .unwrap();
    let sub = created.session_thread_id.clone().unwrap();
    let on_primary = |e: &&sessions::Event| e.session_thread_id.is_none();
    let sent = log.iter().filter(on_primary).find_map(|e| match &e.payload {
        EventPayload::Agent(AgentEvent::ThreadMessageSent {
            to_session_thread_id, to_agent_name, ..
        }) => Some((to_session_thread_id.clone(), to_agent_name.clone())),
        _ => None,
    });
    assert_eq!(sent, Some((sub.clone(), Some("bid-extractor".into()))));
    let sub_events: Vec<&sessions::Event> =
        log.iter().filter(|e| e.session_thread_id.as_ref() == Some(&sub)).collect();
    assert!(sub_events.iter().any(
        |e| matches!(&e.payload, EventPayload::Agent(AgentEvent::ToolUse { name, .. }) if name == "bash")
    ));
    assert!(sub_events.iter().any(|e| matches!(&e.payload, EventPayload::Agent(AgentEvent::ThreadMessageSent { to_session_thread_id, .. }) if *to_session_thread_id == primary_thread_id(&rig.session.get().id))));
    assert!(!sub_events
        .iter()
        .filter(|e| !matches!(e.payload, EventPayload::Session(_)))
        .any(|e| e.on_session_stream()));
    let replies: Vec<_> = log
        .iter()
        .filter(on_primary)
        .filter(|e| matches!(e.payload, EventPayload::Agent(AgentEvent::ThreadMessageReceived { .. })))
        .collect();
    assert_eq!(replies.len(), 1);
    assert_eq!(rig.messages().last().map(String::as_str), Some("all done"));
    let last = mock.requests().into_iter().rfind(|r| r.agent == "orchestrator").unwrap();
    let text = serde_json::to_string(&last.messages).unwrap();
    assert!(text.contains("Message from bid-extractor"));
    assert!(text.contains("extracted 12 fields"));
    let threads = project(&rig.session.get(), &log);
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[1].agent.config.name, "bid-extractor");
    assert_eq!(threads[1].usage.input_tokens, 10);
}

/// The orchestrator sends a task, then a follow-up to the thread id its
/// first reply names.
struct FollowUp(parking_lot::Mutex<usize>);

#[async_trait::async_trait]
impl ModelClient for FollowUp {
    async fn complete(
        &self,
        req: crate::model::ModelRequest,
    ) -> Result<ModelResponse, crate::model::ModelError> {
        if req.agent != "orchestrator" {
            return Ok(say("done"));
        }
        let mut n = self.0.lock();
        *n += 1;
        let seen = serde_json::to_string(&req.messages).unwrap();
        let thread =
            seen.split("thread ").nth(1).and_then(|t| t.split(['.', ')']).next()).unwrap_or("").to_owned();
        Ok(match *n {
            1 => call("send_to_agent", json!({"agent_name": "bid-extractor", "message": "extract"})),
            3 => call("send_to_agent", json!({"session_thread_id": thread, "message": "fix field 3"})),
            5 => say("finished"),
            _ => say("waiting"),
        })
    }
}

#[tokio::test]
async fn a_follow_up_goes_to_the_same_thread() {
    let rig =
        Rig::new(coordinator(), std::sync::Arc::new(FollowUp(parking_lot::Mutex::new(0))), answering(""));
    rig.send(message("go")).await;
    let types = rig.session.types();
    assert_eq!(types.iter().filter(|t| *t == "session.thread_created").count(), 1);
    let sent = rig
        .session
        .log()
        .iter()
        .filter(|e| {
            e.session_thread_id.is_none()
                && matches!(e.payload, EventPayload::Agent(AgentEvent::ThreadMessageSent { .. }))
        })
        .count();
    assert_eq!(sent, 2);
    assert_eq!(rig.messages().last().map(String::as_str), Some("finished"));
}

#[tokio::test]
async fn an_unknown_roster_agent_is_an_error_for_the_model() {
    let bad = call("send_to_agent", json!({"agent_name": "nobody", "message": "x"}));
    let (rig, mock) =
        Rig::scripted(coordinator(), vec![("orchestrator", vec![bad, say("ok")])], answering(""));
    rig.send(message("go")).await;
    let second = &mock.requests()[1];
    assert!(
        matches!(&second.messages.last().unwrap().content[0], Block::ToolResult { is_error: true, content, .. } if content.contains("not on your roster"))
    );
}
