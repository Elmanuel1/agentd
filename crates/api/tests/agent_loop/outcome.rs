//! Outcomes on a coordinator whose delegates are still working.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::common::{self, Auth, KEY, Rig, request, send_to};
use super::{Say, Scripted, events_until, on_loop, send, types};

#[tokio::test]
async fn a_round_starts_only_once_the_delegates_are_done_and_ends_with_its_verdict() {
    // L-5 with L-15: the round starts on the coordinator's reply after the
    // helper's, and the session idles once, after the verdict.
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("cloud").await;
    let name = format!("helper-{}", common::stamp());
    let grade = json!({"result": "satisfied", "criteria": [{"criterion": "Poem", "met": true, "finding": "It is one."}]});
    let model = Scripted::new(vec![
        Say::Tool("create_agent", json!({"agent_name": name, "task": "write it"})),
        Say::Text("done by the helper"),
        Say::Text("the poem is written"),
        Say::Text(Box::leak(grade.to_string().into_boxed_str())),
        Say::Text("summary"),
    ]);
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let helper = json!({"name": name, "model": "claude-haiku-4-5"});
    let (_, helper) = send_to(&app, request("POST", "/v1/agents", Auth::Key(KEY), Some(helper))).await;
    let coord = json!({
        "name": format!("coord-{}", common::stamp()), "model": "claude-haiku-4-5",
        "multiagent": {"type": "coordinator", "agents": [{"type": "agent", "id": helper["id"], "version": helper["version"]}]},
    });
    let (st, coord) = send_to(&app, request("POST", "/v1/agents", Auth::Key(KEY), Some(coord))).await;
    assert_eq!(st, StatusCode::OK, "{coord}");
    let body = json!({"agent": coord["id"], "environment_id": env});
    let (_, created) = send_to(&app, request("POST", "/v1/sessions", Auth::Key(KEY), Some(body))).await;
    let s = created["id"].as_str().unwrap().to_owned();
    let outcome = json!({"events": [{"type": "user.define_outcome", "description": "Write a poem.", "rubric": {"type": "text", "content": "### Poem\nA poem."}, "max_iterations": 2}]});
    let (st, body) = send(&app, &s, outcome).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let t = types(&events);
    let at = |kind: &str| t.iter().position(|k| *k == kind).unwrap_or_else(|| panic!("{kind} in {t:?}"));
    assert_eq!(t.iter().filter(|k| **k == "span.outcome_evaluation_start").count(), 1, "{t:?}");
    assert!(at("agent.thread_message_received") < at("span.outcome_evaluation_start"), "{t:?}");
    let ends: Vec<&Value> = events.iter().filter(|e| e["type"] == "span.outcome_evaluation_end").collect();
    assert_eq!(ends.len(), 1, "{t:?}");
    assert_eq!(ends[0]["result"], "satisfied");
    assert_eq!(t.iter().filter(|k| **k == "session.status_idle").count(), 1, "{t:?}");
    let asked = model.asked.lock().clone();
    assert_eq!(asked.len(), 5);
    assert_eq!(asked[3].agent, "grader", "the grader grades the work the coordinator delivered");
    let (_, got) = send_to(&app, request("GET", &format!("/v1/sessions/{s}"), Auth::Key(KEY), None)).await;
    assert_eq!(got["outcome_evaluations"][0]["result"], "satisfied");
}

#[tokio::test]
async fn a_round_waits_for_a_delegate_that_waits_on_a_tool() {
    // A helper idle on its call is still mid-turn: the coordinator's reply
    // starts no round until the helper has replied.
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let name = format!("helper-{}", common::stamp());
    let grade = json!({"result": "satisfied", "criteria": [{"criterion": "Poem", "met": true, "finding": "It is one."}]});
    let model = Scripted::new(vec![
        // Each step goes to the thread that waited longest: the new helper,
        // then the coordinator.
        Say::Tool("create_agent", json!({"agent_name": name, "task": "write it"})),
        Say::Tool("lookup", json!({"k": 1})),
        Say::Text("waiting on the helper"),
        Say::Text("done by the helper"),
        Say::Text("the poem is written"),
        Say::Text(Box::leak(grade.to_string().into_boxed_str())),
        Say::Text("summary"),
    ]);
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let lookup = json!([{"type": "custom", "name": "lookup", "description": "Look a value up.", "input_schema": {"type": "object"}}]);
    let helper = json!({"name": name, "model": "claude-haiku-4-5", "tools": lookup});
    let (_, helper) = send_to(&app, request("POST", "/v1/agents", Auth::Key(KEY), Some(helper))).await;
    let coord = json!({
        "name": format!("coord-{}", common::stamp()), "model": "claude-haiku-4-5",
        "multiagent": {"type": "coordinator", "agents": [{"type": "agent", "id": helper["id"], "version": helper["version"]}]},
    });
    let (_, coord) = send_to(&app, request("POST", "/v1/agents", Auth::Key(KEY), Some(coord))).await;
    let body = json!({"agent": coord["id"], "environment_id": env});
    let (_, created) = send_to(&app, request("POST", "/v1/sessions", Auth::Key(KEY), Some(body))).await;
    let s = created["id"].as_str().unwrap().to_owned();
    let outcome = json!({"events": [{"type": "user.define_outcome", "description": "Write a poem.", "rubric": {"type": "text", "content": "### Poem\nA poem."}, "max_iterations": 2}]});
    send(&app, &s, outcome).await;
    let events = events_until(&app, &s, "agent.custom_tool_use", 1).await;
    let t = types(&events);
    assert!(!t.contains(&"span.outcome_evaluation_start"), "no round while the helper waits: {t:?}");
    let custom = events.iter().find(|e| e["type"] == "agent.custom_tool_use").unwrap().clone();
    let answer = json!({"events": [{"type": "user.custom_tool_result", "custom_tool_use_id": custom["id"], "content": [{"type": "text", "text": "42"}]}]});
    send(&app, &s, answer).await;
    let events = events_until(&app, &s, "span.outcome_evaluation_end", 1).await;
    let t = types(&events);
    let at = |kind: &str| t.iter().rposition(|k| *k == kind).unwrap_or_else(|| panic!("{kind} in {t:?}"));
    assert!(at("agent.thread_message_received") < at("span.outcome_evaluation_start"), "{t:?}");
}
