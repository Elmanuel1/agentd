//! What a session's work token reaches, over live Postgres: its session's
//! mounted files and outputs, its memory stores as attached, its unanswered
//! calls; and what a session waiting on a tool takes from anyone. Gated by
//! `AGENTD_PG_TESTS=1` and `AGENTD_AWS_ENDPOINT`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use axum::http::StatusCode;
use common::{Auth, Rig, KEY};
use serde_json::{json, Value};
use sessions::events::{AgentEvent, Event, EventPayload, SessionEvent, StopReason};
use sessions::types::SessionId;
use shared::{Timestamp, WorkspaceId};

/// Append `payload` to `session`'s log, processed, and return its id.
async fn append(rig: &Rig, session: &str, payload: EventPayload) -> String {
    let mut event = Event::new(payload);
    event.processed_at = Some(Timestamp::now());
    let id = event.id.as_str().to_owned();
    let ws = WorkspaceId::new(&rig.ws);
    rig.stores
        .session_events
        .append_events(&ws, &SessionId::parse(session).unwrap(), &[event])
        .await
        .unwrap();
    id
}

fn tool_use() -> EventPayload {
    AgentEvent::ToolUse {
        name: "bash".into(),
        input: json!({"command": "ls"}),
        evaluated_permission: None,
        evaluation: None,
    }
    .into()
}

fn events(list: Value) -> Option<Value> {
    Some(json!({ "events": list }))
}

async fn store(rig: &Rig) -> String {
    let (_, s) = rig
        .call(
            "POST",
            "/v1/memory_stores",
            Auth::Key(KEY),
            Some(json!({"name": format!("m{}", common::stamp())})),
        )
        .await;
    s["id"].as_str().unwrap().to_owned()
}

/// A session with `file` mounted and stores `rw` and `ro` attached, its item queued.
async fn worker_session(rig: &Rig, env: &str, file: &str, rw: &str, ro: &str) -> String {
    let resources = json!([
        {"type": "file", "file_id": file, "mount_path": "/mnt/session/uploads/in.txt"},
        {"type": "memory_store", "memory_store_id": rw, "access": "read_write"},
        {"type": "memory_store", "memory_store_id": ro, "access": "read_only"},
    ]);
    let session = rig.session(env, json!({ "resources": resources })).await;
    rig.work.queue(&rig.ctx(), env, &session, env).await.unwrap();
    session
}

#[tokio::test]
async fn a_token_reads_its_mounted_files_and_saves_and_deletes_only_its_outputs() {
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let (_, input) = rig.upload(Auth::Key(KEY), "in.txt", "the input", None).await;
    let input = input["id"].as_str().unwrap().to_owned();
    let (rw, ro) = (store(&rig).await, store(&rig).await);
    let (mine, theirs) =
        (worker_session(&rig, &env, &input, &rw, &ro).await, rig.session(&env, json!({})).await);
    rig.work.queue(&rig.ctx(), &env, &theirs, &env).await.unwrap();
    let (token, other) = (rig.token(&mine), rig.token(&theirs));

    // FO-1: its mounted file, which no API key can download.
    let (st, body) = rig.call("GET", &format!("/v1/files/{input}/content"), Auth::Bearer(&token), None).await;
    assert_eq!((st, body.as_str()), (StatusCode::OK, Some("the input")));
    let (st, body) = rig.call("GET", &format!("/v1/files/{input}/content"), Auth::Key(KEY), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    // FO-2: an output, scoped to its session whatever scope_id says, listed by scope.
    let (st, out) = rig.upload(Auth::Bearer(&token), "out.txt", "the output", Some(&theirs)).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let out = out["id"].as_str().unwrap().to_owned();
    let (_, listed) = rig.call("GET", &format!("/v1/files?scope_id={mine}"), Auth::Key(KEY), None).await;
    assert!(listed["data"].as_array().unwrap().iter().any(|f| f["id"] == out.as_str()), "{listed}");
    let (_, others) = rig.call("GET", &format!("/v1/files?scope_id={theirs}"), Auth::Key(KEY), None).await;
    assert!(!others["data"].as_array().unwrap().iter().any(|f| f["id"] == out.as_str()));
    let (st, bytes) = rig.call("GET", &format!("/v1/files/{out}/content"), Auth::Key(KEY), None).await;
    assert_eq!((st, bytes.as_str()), (StatusCode::OK, Some("the output")), "an output is downloadable");
    // L-10: another session's file, and a file its session does not mount.
    let (_, unmounted) = rig.upload(Auth::Key(KEY), "x.txt", "x", None).await;
    for file in [out.as_str(), unmounted["id"].as_str().unwrap()] {
        let (st, _) = rig.call("GET", &format!("/v1/files/{file}/content"), Auth::Bearer(&other), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{file}");
        let (st, _) = rig.call("GET", &format!("/v1/files/{file}"), Auth::Bearer(&other), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{file}");
    }
    // FO-3: deleting another session's output or a mounted input is a 404; its own output goes.
    let (st, _) = rig.call("DELETE", &format!("/v1/files/{out}"), Auth::Bearer(&other), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = rig.call("DELETE", &format!("/v1/files/{input}"), Auth::Bearer(&token), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = rig.call("DELETE", &format!("/v1/files/{out}"), Auth::Bearer(&token), None).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn a_token_reaches_its_session_s_memory_stores_as_attached_and_no_other_skill_or_store() {
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let (_, input) = rig.upload(Auth::Key(KEY), "in.txt", "x", None).await;
    let (rw, ro, elsewhere) = (store(&rig).await, store(&rig).await, store(&rig).await);
    let session = worker_session(&rig, &env, input["id"].as_str().unwrap(), &rw, &ro).await;
    let token = rig.token(&session);
    let memory = |path: &str| Some(json!({"path": path, "content": "remember"}));
    let (st, made) = rig
        .call("POST", &format!("/v1/memory_stores/{rw}/memories"), Auth::Bearer(&token), memory("/a.md"))
        .await;
    assert_eq!(st, StatusCode::OK, "{made}");
    let (st, _) =
        rig.call("GET", &format!("/v1/memory_stores/{ro}/memories"), Auth::Bearer(&token), None).await;
    assert_eq!(st, StatusCode::OK);
    // L-11.
    let (st, refused) = rig
        .call("POST", &format!("/v1/memory_stores/{ro}/memories"), Auth::Bearer(&token), memory("/b.md"))
        .await;
    assert_eq!((st, refused["error"]["type"].as_str()), (StatusCode::FORBIDDEN, Some("permission_error")));
    // L-10.
    let (st, _) =
        rig.call("GET", &format!("/v1/memory_stores/{elsewhere}/memories"), Auth::Bearer(&token), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = rig.call("GET", "/v1/skills/skill_01notmine/versions/1", Auth::Bearer(&token), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) =
        rig.call("POST", &format!("/v1/memory_stores/{ro}/memories"), Auth::Key(KEY), memory("/c.md")).await;
    assert_eq!(st, StatusCode::OK, "an API key is unaffected");
}

#[tokio::test]
async fn a_token_sends_only_results_for_its_session_s_unanswered_calls() {
    // L-9.
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let session = rig.session(&env, json!({})).await;
    rig.work.queue(&rig.ctx(), &env, &session, &env).await.unwrap();
    let token = rig.token(&session);
    let call = append(&rig, &session, tool_use()).await;
    let path = format!("/v1/sessions/{session}/events");
    let message = json!([{"type": "user.message", "content": [{"type": "text", "text": "hi"}]}]);
    let custom = json!([{"type": "user.custom_tool_result", "custom_tool_use_id": call}]);
    let unknown = json!([{"type": "user.tool_result", "tool_use_id": "sevt_01notacall"}]);
    let twice = json!([{"type": "user.tool_result", "tool_use_id": call}, {"type": "user.tool_result", "tool_use_id": call}]);
    let agent = json!([{"type": "agent.tool_result", "tool_use_id": call, "content": []}]);
    for refused in [message, custom, unknown, twice] {
        let (st, body) = rig.call("POST", &path, Auth::Bearer(&token), events(refused.clone())).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }
    // The gate refuses an agent's event itself, before any read.
    let (st, body) = rig.call("POST", &path, Auth::Bearer(&token), events(agent)).await;
    let gate = "events.0: a session's work token sends only user.tool_result and user.custom_tool_result";
    assert_eq!(
        (st, body["error"].clone()),
        (StatusCode::BAD_REQUEST, json!({"type": "invalid_request_error", "message": gate}))
    );
    let result = json!([{"type": "user.tool_result", "tool_use_id": call, "content": [{"type": "text", "text": "ok"}]}]);
    let (st, body) = rig.call("POST", &path, Auth::Bearer(&token), events(result.clone())).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, _) = rig.call("POST", &path, Auth::Bearer(&token), events(result)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "already answered");
}

#[tokio::test]
async fn of_two_results_sent_at_once_for_one_call_only_one_is_saved() {
    // L-9: the check and the append hold the session's append lock.
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let session = rig.session(&env, json!({})).await;
    rig.work.queue(&rig.ctx(), &env, &session, &env).await.unwrap();
    let token = rig.token(&session);
    let path = format!("/v1/sessions/{session}/events");
    let (ws, id) = (WorkspaceId::new(&rig.ws), SessionId::parse(&session).unwrap());
    for _ in 0..10 {
        let call = append(&rig, &session, tool_use()).await;
        let result = json!([{"type": "user.tool_result", "tool_use_id": call, "content": [{"type": "text", "text": "ok"}]}]);
        let (a, b) = tokio::join!(
            rig.call("POST", &path, Auth::Bearer(&token), events(result.clone())),
            rig.call("POST", &path, Auth::Bearer(&token), events(result)),
        );
        let ((_, saved), (st, refused)) = match a.0 {
            StatusCode::OK => (a, b),
            _ => (b, a),
        };
        assert_eq!(st, StatusCode::BAD_REQUEST, "{refused}");
        let message = format!("events.0: {call} is not a call of this session waiting on its result");
        assert_eq!(refused["error"], json!({"type": "invalid_request_error", "message": message}));
        let expected = json!([{
            "type": "user.tool_result",
            "id": saved["data"][0]["id"],
            "tool_use_id": call,
            "content": [{"type": "text", "text": "ok"}],
            "is_error": false,
        }]);
        assert_eq!(saved["data"], expected);
        let log = rig.stores.session_events.events(&ws, &id).await.unwrap();
        let answers =
            log.iter().filter(|e| e.payload.answered_call().is_some_and(|(c, _)| c.as_str() == call));
        assert_eq!(answers.count(), 1, "one result saved for {call}");
    }
}

#[tokio::test]
async fn a_session_waiting_on_a_tool_takes_only_answers_and_interrupts() {
    // W-19.
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let session = rig.session(&env, json!({})).await;
    let call = append(&rig, &session, tool_use()).await;
    let blocking = vec![sessions::events::EventId::parse(&call).unwrap()];
    let idle = SessionEvent::StatusIdle { stop_reason: StopReason::RequiresAction { event_ids: blocking } };
    append(&rig, &session, idle.into()).await;
    let path = format!("/v1/sessions/{session}/events");
    let message = json!([{"type": "user.message", "content": [{"type": "text", "text": "hi"}]}]);
    let (st, body) = rig.call("POST", &path, Auth::Key(KEY), events(message)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let text = format!(
        "Invalid user.message event at events[0]: waiting on responses to events [{call}]; only \
         `user.tool_confirmation`, `user.custom_tool_result`, `user.tool_result`, or `user.interrupt` may be sent"
    );
    assert_eq!(body["error"], json!({"type": "invalid_request_error", "message": text}));
    for accepted in [
        json!({"type": "user.tool_confirmation", "tool_use_id": call, "result": "allow"}),
        json!({"type": "user.custom_tool_result", "custom_tool_use_id": call}),
        json!({"type": "user.tool_result", "tool_use_id": call}),
        json!({"type": "user.interrupt"}),
    ] {
        let (st, body) = rig.call("POST", &path, Auth::Key(KEY), events(json!([accepted]))).await;
        assert_eq!(st, StatusCode::OK, "{accepted}: {body}");
    }
}
