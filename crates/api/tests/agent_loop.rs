//! The loop end to end: the composed app over live Postgres and LocalStack,
//! a loop worker over the same stores, and a scripted model in process.
//! Gated by `AGENTD_PG_TESTS=1` and `AGENTD_AWS_ENDPOINT`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use axum::Router;
use common::{request, send_to, Auth, Rig, Streaming, KEY};
use engine::model::budget::Unlimited;
use engine::model::retry::Retry;
use engine::model::{Block, ModelClient, ModelError, ModelRequest, ModelResponse, StopKind};
use engine::worker::{Loop, Models, Settings, Stores as LoopStores};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sessions::events::ModelUsage;
use sessions::signals::{LocalSignals, Signals};
use sessions::steps::LoopWorkspaces;
use shared::ports::RuntimeError;
use shared::{Timestamp, WorkspaceId};

/// One scripted answer.
enum Say {
    Text(&'static str),
    Tool(&'static str, Value),
    /// Pend until the call is cancelled.
    Hold,
    Fail(ModelError),
}

/// Plays its script in order, then says "done"; records each request.
#[derive(Default)]
struct Scripted {
    script: Mutex<VecDeque<Say>>,
    asked: Mutex<Vec<ModelRequest>>,
}

impl Scripted {
    fn new(script: Vec<Say>) -> Arc<Self> {
        Arc::new(Self { script: Mutex::new(script.into()), asked: Mutex::default() })
    }
}

#[async_trait]
impl ModelClient for Scripted {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.asked.lock().push(req);
        let say = self.script.lock().pop_front().unwrap_or(Say::Text("done"));
        let usage = ModelUsage { input_tokens: 10, output_tokens: 2, ..ModelUsage::default() };
        let (content, stop) = match say {
            Say::Text(t) => (vec![Block::text(t)], StopKind::EndTurn),
            Say::Tool(name, input) => {
                (vec![Block::ToolUse { id: "toolu_1".into(), name: name.into(), input }], StopKind::ToolUse)
            }
            Say::Hold => return std::future::pending().await,
            Say::Fail(e) => return Err(e),
        };
        Ok(ModelResponse { content, stop, usage, thought: false, cost: Some(0.004) })
    }
}

/// The queue is shared, so these tests take turns, each with its own worker.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A test's loop worker; it stops when the test ends, passed or not.
struct Worker {
    task: tokio::task::JoinHandle<()>,
    _turn: tokio::sync::MutexGuard<'static, ()>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn an_hour_ago() -> Timestamp {
    Timestamp::from_datetime(Timestamp::now().as_datetime() - chrono::Duration::hours(1))
}

/// A replica of the rig's app whose workspace joined the loop at `joined`,
/// and a loop worker calling `model`, sharing one set of signals. Steps
/// other tests left behind are cleared first.
async fn on_loop(rig: &Rig, model: Arc<Scripted>, joined: Timestamp) -> (Router, Worker) {
    let turn = SERIAL.lock().await;
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let pg = store_postgres::PgBackend::connect(&url).await.unwrap();
    let mut conn = pg.diesel.get().await.unwrap();
    diesel_async::RunQueryDsl::execute(diesel::sql_query("DELETE FROM loop_steps"), &mut *conn)
        .await
        .unwrap();
    let signals: Arc<dyn Signals> = Arc::new(LocalSignals::default());
    let on_loop = LoopWorkspaces::new([(WorkspaceId::new(&rig.ws), joined)]);
    let app =
        rig.replica(&rig.stores, Streaming { signals: signals.clone(), on_loop, ..Streaming::default() });
    let stores = LoopStores {
        steps: rig.stores.steps.clone(),
        sessions: rig.stores.sessions.clone(),
        log: rig.stores.session_events.clone(),
        work: rig.stores.work.clone(),
    };
    let retry = Retry { attempts: 3, first: Duration::from_millis(10), deadline: Duration::from_secs(20) };
    let models = Models { client: model, retry, budget: Arc::new(Unlimited), max_tokens: 1000 };
    let settings = Settings {
        owner: format!("test-{}", common::stamp()),
        lease: Duration::from_secs(60),
        per_workspace: 1000,
        steps: 8,
    };
    (app, Worker { task: Loop::new(stores, models, signals, settings).spawn(), _turn: turn })
}

async fn session(app: &Router, env: &str, tools: Value) -> String {
    let agent =
        json!({"name": format!("loop-{}", common::stamp()), "model": "claude-haiku-4-5", "tools": tools});
    let (st, agent) = send_to(app, request("POST", "/v1/agents", Auth::Key(KEY), Some(agent))).await;
    assert_eq!(st, StatusCode::OK, "{agent}");
    let body = json!({"agent": agent["id"], "environment_id": env});
    let (st, s) = send_to(app, request("POST", "/v1/sessions", Auth::Key(KEY), Some(body))).await;
    assert_eq!(st, StatusCode::OK, "{s}");
    s["id"].as_str().unwrap().to_owned()
}

fn message(text: &str) -> Value {
    json!({"events": [{"type": "user.message", "content": [{"type": "text", "text": text}]}]})
}

async fn send(app: &Router, session: &str, body: Value) -> (StatusCode, Value) {
    send_to(app, request("POST", &format!("/v1/sessions/{session}/events"), Auth::Key(KEY), Some(body))).await
}

/// The session's events, once one of type `until` (or any of them) is there.
async fn events_until(app: &Router, session: &str, until: &str, count: usize) -> Vec<Value> {
    for _ in 0..200 {
        let uri = format!("/v1/sessions/{session}/events?limit=1000");
        let (_, page) = send_to(app, request("GET", &uri, Auth::Key(KEY), None)).await;
        let events = page["data"].as_array().cloned().unwrap_or_default();
        if events.iter().filter(|e| e["type"] == until).count() >= count {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let uri = format!("/v1/sessions/{session}/events?limit=1000");
    let (_, page) = send_to(app, request("GET", &uri, Auth::Key(KEY), None)).await;
    panic!("no {count} `{until}` in {session}'s log: {page}");
}

fn types(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap_or_default()).collect()
}

const ALLOW: &str = "always_allow";

fn toolset(policy: &str) -> Value {
    json!([{"type": "agent_toolset_20260401", "default_config": {"permission_policy": {"type": policy}}}])
}

#[tokio::test]
async fn a_message_gets_one_answer_in_live_s_order() {
    // L-1, L-16, L-17, S-3, and live probe 1's order.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let model = Scripted::new(vec![Say::Text("hi there")]);
    let (app, _worker) = on_loop(&rig, model.clone(), an_hour_ago()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, json!([])).await;
    let (st, _) = send(&app, &s, message("hello")).await;
    assert_eq!(st, StatusCode::OK);
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    assert_eq!(
        types(&events),
        [
            "session.status_running",
            "session.thread_status_running",
            "user.message",
            "span.model_request_start",
            "agent.message",
            "span.model_request_end",
            "session.thread_status_idle",
            "session.usage",
            "session.status_idle"
        ]
    );
    assert!(events[2]["processed_at"].is_string(), "the step took the message");
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
    let (_, got) = send_to(&app, request("GET", &format!("/v1/sessions/{s}"), Auth::Key(KEY), None)).await;
    assert_eq!((got["status"].as_str(), got["usage"]["input_tokens"].as_u64()), (Some("idle"), Some(10)));
    assert_eq!(got["usage"]["list_cost"], json!({"amount": "1", "currency": "USD"}), "0.4 cents rounds up");
    let asked = model.asked.lock();
    assert_eq!(asked.len(), 1);
    assert!(matches!(&asked[0].messages[0].content[0], Block::Text { text } if text == "hello"));
}

#[tokio::test]
async fn a_self_hosted_built_in_call_idles_requires_action_and_its_result_continues_the_turn() {
    // L-2, L-3, L-8, L-13.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let model = Scripted::new(vec![Say::Tool("bash", json!({"command": "ls"})), Say::Text("listed")]);
    let (app, _worker) = on_loop(&rig, model.clone(), an_hour_ago()).await;
    let env = rig.environment("self_hosted").await;
    let s = session(&app, &env, toolset(ALLOW)).await;
    send(&app, &s, message("list files")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let call = events.iter().find(|e| e["type"] == "agent.tool_use").unwrap().clone();
    assert_eq!(call["evaluated_permission"], "allow");
    let tail: Vec<&str> = types(&events).into_iter().rev().take(3).collect();
    assert_eq!(tail, ["session.status_idle", "session.usage", "session.thread_status_idle"]);
    let idle = events.last().unwrap();
    assert_eq!(idle["stop_reason"], json!({"type": "requires_action", "event_ids": [call["id"]]}));
    let (_, item) = rig.call("GET", &format!("/v1/environments/{env}/work/{s}"), Auth::Key(KEY), None).await;
    assert_eq!(
        (item["state"].as_str(), item["environment_id"].as_str()),
        (Some("queued"), Some(env.as_str()))
    );

    let result = json!({"events": [{"type": "user.tool_result", "tool_use_id": call["id"], "content": [{"type": "text", "text": "a.txt"}]}]});
    let (st, body) = send_to(
        &app,
        request("POST", &format!("/v1/sessions/{s}/events"), Auth::Bearer(&rig.token(&s)), Some(result)),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let events = events_until(&app, &s, "session.status_idle", 2).await;
    let at = types(&events).iter().position(|t| *t == "user.tool_result").unwrap();
    assert_eq!(
        types(&events)[at - 2..=at + 1],
        [
            "session.status_running",
            "session.thread_status_running",
            "user.tool_result",
            "span.model_request_start"
        ]
    );
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
    assert!(model.asked.lock()[1].messages.iter().any(|m| m
        .content
        .iter()
        .any(|b| matches!(b, Block::ToolResult { content, .. } if content == "a.txt"))));
}

#[tokio::test]
async fn a_cloud_session_stays_running_while_its_worker_runs_a_built_in_call() {
    // L-4, L-8: the runtime queue, and the worker's result shown as the agent's.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let model = Scripted::new(vec![Say::Tool("bash", json!({"command": "ls"})), Say::Text("listed")]);
    let (app, _worker) = on_loop(&rig, model, an_hour_ago()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, toolset(ALLOW)).await;
    send(&app, &s, message("list files")).await;
    let events = events_until(&app, &s, "agent.tool_use", 1).await;
    let call = events.iter().find(|e| e["type"] == "agent.tool_use").unwrap().clone();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, got) = send_to(&app, request("GET", &format!("/v1/sessions/{s}"), Auth::Key(KEY), None)).await;
    assert_eq!(got["status"], "running");
    let (_, item) = rig.call("GET", &format!("/v1/environments/{env}/work/{s}"), Auth::Key(KEY), None).await;
    assert_eq!(item["state"], "queued", "on the runtime queue: {item}");
    let result = json!({"events": [{"type": "user.tool_result", "tool_use_id": call["id"], "content": [{"type": "text", "text": "a.txt"}]}]});
    send_to(
        &app,
        request("POST", &format!("/v1/sessions/{s}/events"), Auth::Bearer(&rig.token(&s)), Some(result)),
    )
    .await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let mirrored = events.iter().find(|e| e["type"] == "agent.tool_result").unwrap();
    assert_eq!(
        (mirrored["tool_use_id"].clone(), mirrored["content"][0]["text"].clone()),
        (call["id"].clone(), json!("a.txt"))
    );
}

#[tokio::test]
async fn asked_and_custom_tools_wait_for_the_client() {
    // L-13, L-14: a confirmation allows or denies; a custom result continues.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let model = Scripted::new(vec![
        Say::Tool("bash", json!({"command": "rm x"})),
        Say::Text("ok"),
        Say::Tool("lookup", json!({"k": 1})),
        Say::Text("found"),
    ]);
    let (app, _worker) = on_loop(&rig, model.clone(), an_hour_ago()).await;
    let env = rig.environment("self_hosted").await;
    let tools = json!([
        {"type": "agent_toolset_20260401", "default_config": {"permission_policy": {"type": "always_ask"}}},
        {"type": "custom", "name": "lookup", "description": "Look a value up.", "input_schema": {"type": "object"}}
    ]);
    let s = session(&app, &env, tools).await;
    send(&app, &s, message("clean up")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let ask = events.iter().find(|e| e["type"] == "agent.tool_use").unwrap().clone();
    assert_eq!(ask["evaluated_permission"], "ask");
    let deny = json!({"events": [{"type": "user.tool_confirmation", "tool_use_id": ask["id"], "result": "deny", "deny_message": "not that"}]});
    let (st, body) = send(&app, &s, deny).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let events = events_until(&app, &s, "session.status_idle", 2).await;
    let refused = events.iter().find(|e| e["type"] == "agent.tool_result").unwrap();
    assert_eq!(
        (refused["is_error"].clone(), refused["content"][0]["text"].clone()),
        (json!(true), json!("not that"))
    );

    let (st, body) = send(&app, &s, message("look it up")).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let events = events_until(&app, &s, "session.status_idle", 3).await;
    let custom = events.iter().find(|e| e["type"] == "agent.custom_tool_use").unwrap().clone();
    assert_eq!(
        events.last().unwrap()["stop_reason"],
        json!({"type": "requires_action", "event_ids": [custom["id"]]})
    );
    let answer = json!({"events": [{"type": "user.custom_tool_result", "custom_tool_use_id": custom["id"], "content": [{"type": "text", "text": "42"}]}]});
    send(&app, &s, answer).await;
    let events = events_until(&app, &s, "session.status_idle", 4).await;
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
}

#[tokio::test]
async fn an_interrupt_during_the_call_cancels_it_and_idles_the_session() {
    // L-6, P6 (live probe 3).
    let Some(rig) = Rig::with_sandbox().await else { return };
    let model = Scripted::new(vec![Say::Hold]);
    let (app, _worker) = on_loop(&rig, model, an_hour_ago()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, json!([])).await;
    send(&app, &s, message("write a long story")).await;
    events_until(&app, &s, "span.model_request_start", 1).await;
    send(&app, &s, json!({"events": [{"type": "user.interrupt"}]})).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let end = events.iter().find(|e| e["type"] == "span.model_request_end").unwrap();
    assert_eq!(end["is_error"], true);
    assert!(!types(&events).contains(&"agent.message"));
    let interrupt = events.iter().find(|e| e["type"] == "user.interrupt").unwrap();
    assert!(interrupt["processed_at"].is_string());
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
}

#[tokio::test]
async fn a_failing_model_is_retried_then_the_turn_stops_with_retries_exhausted() {
    // M-2.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let busy = || Say::Fail(ModelError::RetryAfter("529".into(), Duration::from_millis(20)));
    let model = Scripted::new(vec![busy(), busy(), busy()]);
    let (app, _worker) = on_loop(&rig, model.clone(), an_hour_ago()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, json!([])).await;
    send(&app, &s, message("hi")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    assert_eq!(model.asked.lock().len(), 3, "all three attempts");
    assert!(types(&events).contains(&"session.error"));
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "retries_exhausted"}));
}

#[tokio::test]
async fn a_spent_budget_stops_the_turn_before_the_next_call() {
    // L-18.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let model = Scripted::new(vec![Say::Text("one")]);
    let (app, _worker) = on_loop(&rig, model.clone(), an_hour_ago()).await;
    let env = rig.environment("cloud").await;
    let agent = json!({"name": format!("loop-{}", common::stamp()), "model": "claude-haiku-4-5"});
    let (_, agent) = send_to(&app, request("POST", "/v1/agents", Auth::Key(KEY), Some(agent))).await;
    let body = json!({"agent": agent["id"], "environment_id": env, "budget": {"type": "limit", "max_list_cost": {"amount": "1", "currency": "USD"}}});
    let (_, created) = send_to(&app, request("POST", "/v1/sessions", Auth::Key(KEY), Some(body))).await;
    let s = created["id"].as_str().unwrap().to_owned();
    send(&app, &s, message("one")).await;
    events_until(&app, &s, "session.status_idle", 1).await;
    send(&app, &s, message("two")).await;
    let events = events_until(&app, &s, "session.status_idle", 2).await;
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "budget_reached"}));
    assert_eq!(model.asked.lock().len(), 1, "no call once the budget is spent");
}

#[tokio::test]
async fn a_harness_session_moves_to_the_loop_at_its_next_turn_and_a_failed_stop_appends_nothing() {
    // L-19, L-20.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let env = rig.environment("cloud").await;
    let (old, fresh) = (rig.session(&env, json!({})).await, rig.session(&env, json!({})).await);
    let model = Scripted::new(vec![Say::Text("from the loop")]);
    let (app, _worker) = on_loop(&rig, model, Timestamp::now()).await;
    rig.sandboxes.fail_stops(Some(RuntimeError::Unavailable("daemon down".into())));
    let (st, body) = send(&app, &fresh, message("hi")).await;
    assert_eq!((st.as_u16(), body["error"]["type"].as_str()), (529, Some("overloaded_error")), "{body}");
    rig.sandboxes.fail_stops(Some(RuntimeError::Launch("broken".into())));
    let (st, body) = send(&app, &fresh, message("hi")).await;
    assert_eq!((st.as_u16(), body["error"]["type"].as_str()), (500, Some("api_error")), "{body}");
    let uri = format!("/v1/sessions/{fresh}/events");
    let (_, page) = send_to(&app, request("GET", &uri, Auth::Key(KEY), None)).await;
    assert_eq!(page["data"], json!([]), "a failed stop appends nothing");
    rig.sandboxes.fail_stops(None);
    send(&app, &old, message("hello again")).await;
    let events = events_until(&app, &old, "session.status_idle", 1).await;
    let answer = events.iter().find(|e| e["type"] == "agent.message").unwrap();
    assert_eq!(answer["content"][0]["text"], "from the loop");
    let (_, item) =
        rig.call("GET", &format!("/v1/environments/{env}/work/{old}"), Auth::Key(KEY), None).await;
    assert_eq!(item["state"], "queued", "its work moved to the runtime queue: {item}");
}

#[tokio::test]
async fn a_coordinator_delegates_to_a_roster_thread_and_hears_back() {
    // L-15.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let env = rig.environment("cloud").await;
    let name = format!("helper-{}", common::stamp());
    let model = Scripted::new(vec![
        Say::Tool("send_to_agent", json!({"agent_name": name, "message": "do it"})),
        Say::Text("waiting on the helper"),
        Say::Text("done by the helper"),
        Say::Text("all done"),
    ]);
    let (app, _worker) = on_loop(&rig, model.clone(), an_hour_ago()).await;
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
    send(&app, &s, message("run it")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let t = types(&events);
    for kind in ["session.thread_created", "agent.thread_message_sent", "agent.thread_message_received"] {
        assert!(t.contains(&kind), "{kind} in {t:?}");
    }
    assert_eq!(
        t.iter().filter(|k| **k == "session.status_idle").count(),
        1,
        "the session idles once, at the end: {t:?}"
    );
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
    let thread =
        events.iter().find(|e| e["type"] == "session.thread_created").unwrap()["session_thread_id"].clone();
    let uri = format!("/v1/sessions/{s}/threads/{}/events", thread.as_str().unwrap());
    let (st, page) = send_to(&app, request("GET", &uri, Auth::Key(KEY), None)).await;
    assert_eq!(st, StatusCode::OK, "{page}");
    let on_thread = page["data"].as_array().unwrap();
    assert!(
        on_thread
            .iter()
            .any(|e| e["type"] == "agent.message" && e["content"][0]["text"] == "done by the helper"),
        "the helper's events are on its thread: {page}"
    );
    let asked = model.asked.lock();
    assert_eq!(asked.len(), 4);
    assert_eq!(asked[2].agent, name, "the third call is the helper's");
    let heard = asked[3]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .any(|b| matches!(b, Block::Text { text } if text.starts_with(&format!("Message from {name}"))));
    assert!(heard, "the coordinator hears the helper's reply");
}
