//! MCP servers end to end: a scripted streamable-HTTP server in process, the
//! loop connecting to it from agentd (MC-1 to MC-26).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sessions::steps::{Claim, StepCounts, StepEnd, StepWrite, Steps};
use sessions::store::StoreError;
use shared::net::Guard;

use super::common::{request, send_to, Auth, Rig, KEY};
use super::{db, events_until, message, send, types, worker, Say, Scripted, Tune};

/// One request the server got: its method, session id, bearer, user agent,
/// `clientInfo`, the call's arguments, and when it came.
#[derive(Debug, Clone)]
struct Got {
    method: String,
    session: Option<String>,
    bearer: Option<String>,
    agent: Option<String>,
    client: Value,
    arguments: Value,
    at: chrono::DateTime<chrono::Utc>,
}

/// A scripted MCP server: `lookup` answers, `flaky` fails with a 502,
/// `locked` answers 401, `hang` never answers, `forget` makes it end every
/// session it gave, and `refuse` answers 401 to everything.
#[derive(Default)]
struct Server {
    got: Mutex<Vec<Got>>,
    sessions: AtomicUsize,
    forget: AtomicBool,
    refuse: AtomicBool,
    tokens: AtomicUsize,
    /// `grow` was called: the list has gained `extra`.
    grown: AtomicBool,
}

impl Server {
    fn methods(&self) -> Vec<String> {
        self.got.lock().iter().map(|g| g.method.clone()).collect()
    }

    fn count(&self, method: &str) -> usize {
        self.got.lock().iter().filter(|g| g.method == method).count()
    }

    fn calls(&self) -> Vec<Got> {
        self.got.lock().iter().filter(|g| g.method == "tools/call").cloned().collect()
    }
}

async fn rpc(State(s): State<Arc<Server>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let header = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let method = body["method"].as_str().unwrap_or_default().to_owned();
    let session = header("mcp-session-id");
    let (bearer, agent, client) =
        (header("authorization"), header("user-agent"), body["params"]["clientInfo"].clone());
    let (arguments, at) = (body["params"]["arguments"].clone(), chrono::Utc::now());
    let got = Got { method: method.clone(), session: session.clone(), bearer, agent, client, arguments, at };
    s.got.lock().push(got);
    if s.refuse.load(Ordering::SeqCst) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if method == "tools/call" && session.is_some() && s.forget.swap(false, Ordering::SeqCst) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let id = body["id"].clone();
    let result = match method.as_str() {
        "initialize" => {
            let n = s.sessions.fetch_add(1, Ordering::SeqCst);
            let mut r = Json(json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-11-25",
                "capabilities": {"tools": {}}, "serverInfo": {"name": "docs", "version": "1"}}}))
            .into_response();
            r.headers_mut().insert("mcp-session-id", format!("s-{n}").parse().unwrap());
            return r;
        }
        "tools/list" if s.grown.load(Ordering::SeqCst) => json!({"tools": [
            {"name": "lookup", "description": "Look a word up.", "inputSchema": {"type": "object"}},
            {"name": "extra", "description": "Added later.", "inputSchema": {"type": "object"}}]}),
        "tools/call" if body["params"]["name"] == "grow" => {
            s.grown.store(true, Ordering::SeqCst);
            let stream = format!(
                "event: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
                json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}),
                json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "grown"}]}})
            );
            return ([("content-type", "text/event-stream")], stream).into_response();
        }
        "tools/list" => json!({"tools": [
            {"name": "lookup", "description": "Look a word up.", "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}},
            {"name": "flaky", "description": "Fails.", "inputSchema": {"type": "object"}},
            {"name": "grow", "description": "Adds a tool.", "inputSchema": {"type": "object"}},
            {"name": "locked", "description": "Refuses.", "inputSchema": {"type": "object"}},
            {"name": "hang", "description": "Never answers.", "inputSchema": {"type": "object"}}]}),
        "tools/call" if body["params"]["name"] == "flaky" => return StatusCode::BAD_GATEWAY.into_response(),
        "tools/call" if body["params"]["name"] == "locked" => {
            return StatusCode::UNAUTHORIZED.into_response()
        }
        "tools/call" if body["params"]["name"] == "hang" => {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            return StatusCode::OK.into_response();
        }
        "tools/call" => {
            json!({"content": [{"type": "text", "text": format!("found {}", body["params"]["arguments"]["q"])}]})
        }
        _ => return StatusCode::ACCEPTED.into_response(),
    };
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

async fn token(State(s): State<Arc<Server>>) -> Json<Value> {
    s.tokens.fetch_add(1, Ordering::SeqCst);
    Json(json!({"access_token": "fresh-token-7", "token_type": "Bearer", "expires_in": 3600}))
}

/// Start a server; its URL and the guard that lets the loop reach it.
async fn serve() -> (Arc<Server>, String, Guard) {
    let server = Arc::new(Server::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/mcp", post(rpc))
        .route("/token", post(token))
        .route(
            "/badtoken",
            post(|| async { (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))) }),
        )
        .with_state(server.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    (server, format!("http://{addr}/mcp"), Guard::allowing(vec![addr]))
}

fn mcp_toolset(policy: Option<&str>) -> Value {
    let mut t = json!({"type": "mcp_toolset", "mcp_server_name": "docs"});
    if let Some(p) = policy {
        t["default_config"] = json!({"permission_policy": {"type": p}});
    }
    t
}

/// A session of an agent with the server `url` as `docs`.
async fn mcp_session(app: &Router, env: &str, url: &str, policy: Option<&str>, vaults: Value) -> String {
    let agent = json!({"name": format!("mcp-{}", super::common::stamp()), "model": "claude-haiku-4-5",
        "mcp_servers": [{"type": "url", "name": "docs", "url": url}], "tools": [mcp_toolset(policy)]});
    let (st, agent) = send_to(app, request("POST", "/v1/agents", Auth::Key(KEY), Some(agent))).await;
    assert_eq!(st, StatusCode::OK, "{agent}");
    let body = json!({"agent": agent["id"], "environment_id": env, "vault_ids": vaults});
    let (st, s) = send_to(app, request("POST", "/v1/sessions", Auth::Key(KEY), Some(body))).await;
    assert_eq!(st, StatusCode::OK, "{s}");
    s["id"].as_str().unwrap().to_owned()
}

async fn rows(session: &str) -> i64 {
    #[derive(diesel::QueryableByName)]
    struct N {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        n: i64,
    }
    let pg = db().await;
    let mut conn = pg.diesel.get().await.unwrap();
    let q = diesel::sql_query("SELECT count(*) AS n FROM mcp_connections WHERE session_id = $1")
        .bind::<diesel::sql_types::Text, _>(session);
    let got: Vec<N> = diesel_async::RunQueryDsl::load(q, &mut *conn).await.unwrap();
    got[0].n
}

fn of<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events.iter().filter(|e| e["type"] == kind).collect()
}

#[tokio::test]
async fn an_allowed_mcp_call_runs_in_the_loop_and_the_next_call_hears_its_result() {
    // MC-1, MC-8, MC-13, MC-15, MC-24, MC-25.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let model = Scripted::new(vec![
        Say::Calls(vec![("mcp__docs__lookup", json!({"q": "cat"}))]),
        Say::Text("found it"),
    ]);
    let (app, _worker) = worker(&rig, model.clone(), Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    let item = format!("/v1/environments/{env}/work/{s}");
    let (_, before) = rig.call("GET", &item, Auth::Key(KEY), None).await;
    send(&app, &s, message("look up cat")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    assert_eq!(
        types(&events),
        [
            "session.status_running",
            "session.thread_status_running",
            "user.message",
            "span.model_request_start",
            "agent.mcp_tool_use",
            "span.model_request_end",
            "agent.mcp_tool_result",
            "span.model_request_start",
            "agent.message",
            "span.model_request_end",
            "session.thread_status_idle",
            "session.usage",
            "session.status_idle"
        ]
    );
    let call = of(&events, "agent.mcp_tool_use")[0];
    assert_eq!((call["name"].as_str(), call["mcp_server_name"].as_str()), (Some("lookup"), Some("docs")));
    assert_eq!(
        (call["evaluated_permission"].as_str(), &call["evaluation"]),
        (Some("allow"), &json!({"type": "always_allow"}))
    );
    let result = of(&events, "agent.mcp_tool_result")[0];
    assert_eq!(result["mcp_tool_use_id"], call["id"]);
    assert_eq!(
        (result["content"][0]["text"].as_str(), result["is_error"].as_bool()),
        (Some("found \"cat\""), Some(false))
    );
    assert_eq!(server.methods(), ["initialize", "notifications/initialized", "tools/list", "tools/call"]);
    let init = server.got.lock()[0].clone();
    assert_eq!(
        (init.agent.as_deref(), init.client),
        (Some("Claude-User"), json!({"name": "claude-ai", "version": "0.1.0"})),
        "live's session-time client (B27)"
    );
    let call_got = server.got.lock().last().cloned().unwrap();
    assert_eq!(call_got.session.as_deref(), Some("s-0"), "the call reuses the stored session (MC-13)");
    {
        let asked = model.asked.lock();
        assert_eq!(asked.len(), 2, "the tool step calls no model");
        assert!(
            asked[0].tools.iter().any(|t| t.name == "mcp__docs__lookup"),
            "live's name for the model (B27)"
        );
        let fed = serde_json::to_string(&asked[1].messages).unwrap();
        assert!(fed.contains("found \\\"cat\\\""), "{fed}");
    }
    let (_, after) = rig.call("GET", &item, Auth::Key(KEY), None).await;
    assert_eq!(after, before, "an MCP call queues no work item");
    assert_eq!(rows(&s).await, 1);
    send(&app, &s, message("again")).await;
    events_until(&app, &s, "session.status_idle", 2).await;
    assert_eq!(server.count("initialize"), 1, "a new turn keeps a live connection, as live does (B27)");
}

#[tokio::test]
async fn an_asked_mcp_call_waits_for_the_client_and_runs_only_when_allowed() {
    // MC-12: an `mcp_toolset` asks by default.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let lookup = || Say::Calls(vec![("mcp__docs__lookup", json!({"q": "dog"}))]);
    let model = Scripted::new(vec![lookup(), Say::Text("ok"), lookup(), Say::Text("fine")]);
    let (app, _worker) = worker(&rig, model, Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, None, json!([])).await;
    send(&app, &s, message("look up dog")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let call = of(&events, "agent.mcp_tool_use")[0].clone();
    assert_eq!(
        (call["evaluated_permission"].as_str(), &call["evaluation"]),
        (Some("ask"), &json!({"type": "always_ask"}))
    );
    assert_eq!(
        events.last().unwrap()["stop_reason"],
        json!({"type": "requires_action", "event_ids": [call["id"]]})
    );
    assert_eq!(server.count("tools/call"), 0);
    let allow =
        json!({"events": [{"type": "user.tool_confirmation", "tool_use_id": call["id"], "result": "allow"}]});
    send(&app, &s, allow).await;
    let events = events_until(&app, &s, "session.status_idle", 2).await;
    assert_eq!(of(&events, "agent.mcp_tool_result")[0]["is_error"], false);
    assert_eq!(server.count("tools/call"), 1);
    send(&app, &s, message("and again")).await;
    let events = events_until(&app, &s, "session.status_idle", 3).await;
    let second = of(&events, "agent.mcp_tool_use")[1].clone();
    let deny = json!({"events": [{"type": "user.tool_confirmation", "tool_use_id": second["id"], "result": "deny"}]});
    send(&app, &s, deny).await;
    let events = events_until(&app, &s, "session.status_idle", 4).await;
    let refused = of(&events, "agent.mcp_tool_result")[1];
    assert_eq!(
        (refused["mcp_tool_use_id"].clone(), refused["is_error"].as_bool()),
        (second["id"].clone(), Some(true))
    );
    assert_eq!(refused["content"][0]["text"], "The user denied this tool call.");
    assert_eq!(server.count("tools/call"), 1, "a denied call never reaches the server");
}

#[tokio::test]
async fn a_server_agentd_may_not_reach_records_a_connection_error_and_the_turn_goes_on() {
    // MC-5, MC-10, MC-11.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, _) = serve().await;
    let model = Scripted::new(vec![Say::Text("no tools")]);
    let (app, _worker) = worker(&rig, model.clone(), Tune::default()).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("hi")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let at = types(&events);
    let error = at.iter().position(|t| *t == "session.error").expect("a connection error");
    assert_eq!(at[error + 1], "span.model_request_start", "written before the call, as live does");
    let e = &events[error]["error"];
    assert_eq!(
        (e["type"].as_str(), e["mcp_server_name"].as_str()),
        (Some("mcp_connection_failed_error"), Some("docs"))
    );
    assert_eq!(e["retry_status"], json!({"type": "retrying"}));
    assert!(server.methods().is_empty(), "a loopback server is refused before any request (MC-11)");
    assert!(model.asked.lock()[0].tools.iter().all(|t| t.name != "mcp__docs__lookup"));
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));

    let limited = json!({"name": "limited", "config": {"type": "cloud",
        "networking": {"type": "limited", "allowed_hosts": ["example.com"]}}});
    let (_, env) = rig.call("POST", "/v1/environments", Auth::Key(KEY), Some(limited)).await;
    let (server, url, guard) = serve().await;
    let (app, _worker) = {
        drop(_worker);
        worker(&rig, Scripted::new(vec![]), Tune { guard, ..Tune::default() }).await
    };
    let s = mcp_session(&app, env["id"].as_str().unwrap(), &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("hi")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    assert_eq!(of(&events, "session.error").len(), 1, "a host the networking does not list (MC-10)");
    assert!(server.methods().is_empty());
}

#[tokio::test]
async fn a_server_that_refuses_authentication_records_an_authentication_error() {
    // MC-4: live's words with no vault credential for the URL (B27) and for a
    // static bearer the server refused (MC-4 capture); an `mcp_oauth` refusal
    // is uncaptured and keeps agentd's words. A refused bearer is sent once
    // more with no Authorization header, as live does.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    server.refuse.store(true, Ordering::SeqCst);
    let model = Scripted::new(vec![Say::Text("none"), Say::Text("bearer"), Say::Text("oauth")]);
    let (app, _worker) = worker(&rig, model, Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let (bearer, oauth) = (bearer_vault(&rig, &url, "tok-refused").await, oauth_vault(&rig, &url).await);
    let unstored =
        "no credential is stored for this server URL \u{2014} check that the agent's MCP server URL \
        matches the URL in the vault";
    let rejected = "server rejected the static bearer token \u{2014} verify the credential value";
    let cases =
        [(json!([]), unstored), (json!([bearer]), rejected), (json!([oauth]), "authentication required")];
    for (vaults, why) in cases {
        let s = mcp_session(&app, &env, &url, Some("always_allow"), vaults).await;
        send(&app, &s, message("go")).await;
        let events = events_until(&app, &s, "session.status_idle", 1).await;
        let errors: Vec<&Value> = of(&events, "session.error").into_iter().map(|e| &e["error"]).collect();
        let refused = json!({"type": "mcp_authentication_failed_error", "mcp_server_name": "docs",
            "message": format!("MCP server 'docs' initialize failed: {why}"), "retry_status": {"type": "retrying"}});
        assert_eq!(errors, [&refused], "{why}");
        assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
    }
    assert_eq!(server.methods(), ["initialize"; 5], "nothing after a refused initialize");
    let bearers: Vec<Option<String>> = server.got.lock().iter().map(|g| g.bearer.clone()).collect();
    let (refused, oauth) = (Some("Bearer tok-refused".into()), Some("Bearer tok-oauth".into()));
    assert_eq!(bearers, [None, refused, None, oauth, None], "one retry, only after a bearer");
}

/// A vault holding a credential with `auth`; its id.
async fn vault_with(rig: &Rig, auth: Value) -> String {
    let (_, vault) = rig.call("POST", "/v1/vaults", Auth::Key(KEY), Some(json!({"display_name": "v"}))).await;
    let vault = vault["id"].as_str().unwrap().to_owned();
    let path = format!("/v1/vaults/{vault}/credentials");
    let (st, cred) =
        rig.call("POST", &path, Auth::Key(KEY), Some(json!({"display_name": "docs", "auth": auth}))).await;
    assert_eq!(st, StatusCode::OK, "{cred}");
    vault
}

/// A vault holding a static bearer `token` for `url`.
async fn bearer_vault(rig: &Rig, url: &str, token: &str) -> String {
    vault_with(rig, json!({"type": "static_bearer", "mcp_server_url": url, "token": token})).await
}

/// A vault holding an unexpired `mcp_oauth` token `tok-oauth` for `url`.
async fn oauth_vault(rig: &Rig, url: &str) -> String {
    let auth = json!({"type": "mcp_oauth", "mcp_server_url": url, "access_token": "tok-oauth",
        "expires_at": "2099-01-01T00:00:00Z"});
    vault_with(rig, auth).await
}

#[tokio::test]
async fn a_call_whose_bearer_is_refused_gets_an_authentication_result_and_no_session_error() {
    // MC-26: live's words for a 401 to a call after a static bearer connected
    // (MC-26 capture); an `mcp_oauth` one is uncaptured and keeps agentd's
    // words. Either is sent once more with no Authorization header.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let locked = || Say::Calls(vec![("mcp__docs__locked", json!({}))]);
    let model = Scripted::new(vec![locked(), Say::Text("sorry"), locked(), Say::Text("sorry")]);
    let (app, _worker) = worker(&rig, model, Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let rejected =
        "MCP server 'docs' authentication failed: server rejected the static bearer token \u{2014} \
        verify the credential value. The tool is unavailable until the server is re-authorized.";
    let cases = [
        (bearer_vault(&rig, &url, "tok-late").await, rejected, "Bearer tok-late"),
        (oauth_vault(&rig, &url).await, "upstream server error (HTTP 401)", "Bearer tok-oauth"),
    ];
    for (vault, text, sent) in cases {
        server.got.lock().clear();
        let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([vault])).await;
        send(&app, &s, message("try")).await;
        let events = events_until(&app, &s, "session.status_idle", 1).await;
        let (call, result) = (of(&events, "agent.mcp_tool_use")[0], of(&events, "agent.mcp_tool_result")[0]);
        let mut want = json!({"type": "agent.mcp_tool_result", "mcp_tool_use_id": call["id"],
            "content": [{"type": "text", "text": text}], "is_error": true});
        want["id"] = result["id"].clone();
        want["processed_at"] = result["processed_at"].clone();
        assert_eq!(result, &want, "{sent}");
        assert!(of(&events, "session.error").is_empty(), "a refused call records no session.error");
        let calls: Vec<Option<String>> =
            server.got.lock().iter().filter(|g| g.method == "tools/call").map(|g| g.bearer.clone()).collect();
        assert_eq!(calls, [Some(sent.to_owned()), None], "sent again with no Authorization header");
        assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
    }
}

#[tokio::test]
async fn a_call_the_server_never_answers_gets_live_s_502_result_and_no_session_error() {
    // MC-26: live gives up on an unanswered call after about 125 s; the test
    // waits 1 s.
    let Some(rig) = Rig::new().await else { return };
    let built = api::agent_loop::mcp(&rig.stores, Guard::default(), &Arc::default());
    assert_eq!(built.timeout, std::time::Duration::from_secs(125), "live's wait (MC-26 capture)");
    let (_server, url, guard) = serve().await;
    let model = Scripted::new(vec![Say::Calls(vec![("mcp__docs__hang", json!({}))]), Say::Text("sorry")]);
    let tune = Tune { guard, mcp_timeout: Some(std::time::Duration::from_secs(1)), ..Tune::default() };
    let (app, _worker) = worker(&rig, model, tune).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("try")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let (call, result) = (of(&events, "agent.mcp_tool_use")[0], of(&events, "agent.mcp_tool_result")[0]);
    let mut want = json!({"type": "agent.mcp_tool_result", "mcp_tool_use_id": call["id"],
        "content": [{"type": "text", "text": "upstream server error (HTTP 502)"}], "is_error": true});
    want["id"] = result["id"].clone();
    want["processed_at"] = result["processed_at"].clone();
    assert_eq!(result, &want, "live's result (MC-26 capture)");
    assert!(of(&events, "session.error").is_empty(), "an unanswered call records no session.error");
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
}

#[tokio::test]
async fn a_call_whose_server_fails_gets_an_error_result_and_the_turn_goes_on() {
    // MC-26.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let calls = vec![("mcp__docs__flaky", json!({})), ("mcp__docs__lookup", json!({"q": "x"}))];
    let model = Scripted::new(vec![Say::Calls(calls), Say::Text("sorry")]);
    let (app, _worker) = worker(&rig, model, Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("try")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let (calls, results) = (of(&events, "agent.mcp_tool_use"), of(&events, "agent.mcp_tool_result"));
    let result = |r: &Value| (r["mcp_tool_use_id"].clone(), r["content"].clone(), r["is_error"].clone());
    let failed = json!([{"type": "text", "text": "upstream server error (HTTP 502)"}]);
    assert_eq!(result(results[0]), (calls[0]["id"].clone(), failed, json!(true)), "live's content (B27)");
    let found = json!([{"type": "text", "text": "found \"x\""}]);
    assert_eq!(result(results[1]), (calls[1]["id"].clone(), found, json!(false)), "the server stays usable");
    assert!(of(&events, "session.error").is_empty(), "a failed call records no session.error (B27)");
    assert_eq!(server.count("tools/call"), 2);
    assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
}

#[tokio::test]
async fn a_vault_credential_authenticates_and_never_reaches_the_log() {
    // MC-2, MC-9, MC-22.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let model =
        Scripted::new(vec![Say::Calls(vec![("mcp__docs__lookup", json!({"q": "k"}))]), Say::Text("done")]);
    let (app, _worker) = worker(&rig, model, Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let vault = bearer_vault(&rig, &url, "tok-secret-9").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([vault])).await;
    send(&app, &s, message("go")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    assert!(server.got.lock().iter().all(|g| g.bearer.as_deref() == Some("Bearer tok-secret-9")));
    assert!(!serde_json::to_string(&events).unwrap().contains("tok-secret-9"), "MC-9");
    assert_eq!(rows(&s).await, 1);
    let (st, _) = send_to(&app, request("DELETE", &format!("/v1/sessions/{s}"), Auth::Key(KEY), None)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rows(&s).await, 0, "the delete takes the session's connections (MC-22)");
}

#[tokio::test]
async fn a_session_the_server_ended_reconnects_and_the_call_goes_through() {
    // MC-15: a 404 to the stored session id.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let model =
        Scripted::new(vec![Say::Calls(vec![("mcp__docs__lookup", json!({"q": "z"}))]), Say::Text("done")]);
    let (app, _worker) = worker(&rig, model, Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    server.forget.store(true, Ordering::SeqCst);
    send(&app, &s, message("go")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let result = of(&events, "agent.mcp_tool_result")[0];
    let call = of(&events, "agent.mcp_tool_use")[0];
    assert_eq!(result["mcp_tool_use_id"], call["id"]);
    assert_eq!(
        (&result["content"], &result["is_error"]),
        (&json!([{"type": "text", "text": "found \"z\""}]), &json!(false))
    );
    assert_eq!(server.count("initialize"), 2);
}

/// A vault holding an expired `mcp_oauth` token for `url`, refreshed at
/// `endpoint`, its row last updated `age` seconds ago.
async fn expired(rig: &Rig, url: &str, endpoint: &str, age: i64) -> (String, String) {
    expiring(rig, url, endpoint, "2020-01-01T00:00:00Z", age).await
}

/// A vault holding an `mcp_oauth` token for `url` that expires at `at`,
/// refreshed at `endpoint`, its row last updated `age` seconds ago.
async fn expiring(rig: &Rig, url: &str, endpoint: &str, at: &str, age: i64) -> (String, String) {
    let (_, vault) = rig.call("POST", "/v1/vaults", Auth::Key(KEY), Some(json!({"display_name": "v"}))).await;
    let vault = vault["id"].as_str().unwrap().to_owned();
    let auth = json!({"type": "mcp_oauth", "mcp_server_url": url, "access_token": "stale-token-3",
        "expires_at": at, "refresh": {"client_id": "c", "token_endpoint": endpoint,
        "token_endpoint_auth": {"type": "none"}, "refresh_token": "r"}});
    let path = format!("/v1/vaults/{vault}/credentials");
    let (st, cred) =
        rig.call("POST", &path, Auth::Key(KEY), Some(json!({"display_name": "c", "auth": auth}))).await;
    assert_eq!(st, StatusCode::OK, "{cred}");
    let id = cred["id"].as_str().unwrap().to_owned();
    age_row(&id, age).await;
    (vault, id)
}

/// Make credential `id`'s row read as updated `age` seconds ago.
async fn age_row(id: &str, age: i64) {
    let pg = db().await;
    let mut conn = pg.diesel.get().await.unwrap();
    let at =
        (chrono::Utc::now() - chrono::Duration::seconds(age)).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    let q = diesel::sql_query("UPDATE credentials SET doc = jsonb_set(doc, '{updated_at}', to_jsonb($1::text)) WHERE credential_id = $2")
        .bind::<diesel::sql_types::Text, _>(at)
        .bind::<diesel::sql_types::Text, _>(id);
    diesel_async::RunQueryDsl::execute(q, &mut *conn).await.unwrap();
}

#[tokio::test]
async fn an_expired_token_is_refreshed_once_stored_and_used() {
    // MC-7, MC-9.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let model =
        Scripted::new(vec![Say::Calls(vec![("mcp__docs__lookup", json!({"q": "r"}))]), Say::Text("done")]);
    let (app, _worker) = worker(&rig, model, Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let (vault, id) = expired(&rig, &url, &url.replace("/mcp", "/token"), 120).await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([vault])).await;
    send(&app, &s, message("go")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    assert_eq!(server.tokens.load(Ordering::SeqCst), 1, "one refresh");
    assert!(server.got.lock().iter().all(|g| g.bearer.as_deref() == Some("Bearer fresh-token-7")));
    assert_eq!(of(&events, "agent.mcp_tool_result")[0]["is_error"], false);
    let log = serde_json::to_string(&events).unwrap();
    assert!(!log.contains("fresh-token-7") && !log.contains("stale-token-3"));
    let (_, cred) =
        rig.call("GET", &format!("/v1/vaults/{vault}/credentials/{id}"), Auth::Key(KEY), None).await;
    assert!(cred["auth"]["expires_at"].as_str().is_some_and(|at| at > "2026"), "{cred}");
}

#[tokio::test]
async fn a_refresh_another_worker_holds_waits_without_a_model_call() {
    // MC-20: a row updated less than 30 s ago is a refresh in flight.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let model = Scripted::new(vec![Say::Text("done")]);
    let (app, _worker) = worker(&rig, model.clone(), Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let (vault, id) = expired(&rig, &url, &url.replace("/mcp", "/token"), 5).await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([vault])).await;
    send(&app, &s, message("go")).await;
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert!(model.asked.lock().is_empty(), "the step waits for the refresh");
    assert_eq!(server.tokens.load(Ordering::SeqCst), 0);
    let step = super::step_of(&s).await.expect("the step stays queued");
    assert!(step.due_in <= 1);
    age_row(&id, 120).await;
    events_until(&app, &s, "session.status_idle", 1).await;
    assert_eq!((server.tokens.load(Ordering::SeqCst), model.asked.lock().len()), (1, 1));
}

#[tokio::test]
async fn a_refused_or_internal_token_endpoint_records_an_authentication_error() {
    // MC-21, MC-27.
    let Some(rig) = Rig::new().await else { return };
    for endpoint in ["/badtoken", "http://169.254.169.254/token"] {
        let (server, url, guard) = serve().await;
        let model = Scripted::new(vec![Say::Text("done")]);
        let tune = Tune { guard, hooks: Some(super::hooks(&rig)), ..Tune::default() };
        let (app, worker_) = worker(&rig, model, tune).await;
        let env = rig.environment("cloud").await;
        let endpoint = match endpoint.starts_with('/') {
            true => url.replace("/mcp", endpoint),
            false => endpoint.to_owned(),
        };
        let (vault, cred) = expired(&rig, &url, &endpoint, 120).await;
        let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([vault])).await;
        send(&app, &s, message("go")).await;
        let events = events_until(&app, &s, "session.status_idle", 1).await;
        let errors = of(&events, "session.error");
        assert_eq!(errors.len(), 1, "{endpoint}");
        let refused = json!({"type": "mcp_authentication_failed_error", "mcp_server_name": "docs",
            "message": "MCP server 'docs' initialize failed: the access token expired and could not be refreshed",
            "retry_status": {"type": "retrying"}});
        assert_eq!(errors[0]["error"], refused);
        assert!(server.methods().is_empty(), "the server is not called with a dead token");
        assert_eq!(events.last().unwrap()["stop_reason"], json!({"type": "end_turn"}));
        let sent = super::webhooks_about(&cred).await;
        assert_eq!(sent, ["vault_credential.refresh_failed"], "H-5: {endpoint}");
        drop(worker_);
    }
}

#[tokio::test]
async fn a_crowded_prompt_clears_old_results_and_caps_long_ones() {
    // CP-2, CP-3: the prompt changes, the stored events stay whole.
    let Some(rig) = Rig::new().await else { return };
    let (_server, url, guard) = serve().await;
    let calls = (0..5).map(|i| Say::Calls(vec![("mcp__docs__lookup", json!({"q": format!("w{i}")}))]));
    let model = Scripted::new(calls.chain([Say::Text("done")]).collect());
    let compaction = engine::turn::compact::Compaction { clear_at: 1, keep_results: 3, tool_output_limit: 2 };
    let (app, _worker) = worker(&rig, model.clone(), Tune { guard, compaction, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("look five up")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let results = of(&events, "agent.mcp_tool_result");
    assert_eq!(results[0]["content"][0]["text"], "found \"w0\"", "the stored result is whole");
    let calls = of(&events, "agent.mcp_tool_use");
    let asked = model.asked.lock();
    let fed: Vec<String> = asked
        .last()
        .unwrap()
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            super::Block::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(fed.len(), 5);
    for (i, text) in fed.iter().enumerate().take(2) {
        assert_eq!(*text, format!("[tool result cleared: event {}]", calls[i]["id"].as_str().unwrap()));
    }
    for text in &fed[2..] {
        assert_eq!(*text, "found \"w\n[tool result cut at 2 tokens]", "8 bytes, 2 tokens");
    }
}

#[tokio::test]
async fn a_list_changed_notification_lists_the_tools_again_and_stores_them() {
    // MC-16: the answer comes as an event stream carrying the notification.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let model = Scripted::new(vec![Say::Calls(vec![("mcp__docs__grow", json!({}))]), Say::Text("done")]);
    let (app, _worker) = worker(&rig, model.clone(), Tune { guard, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("grow")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let (call, result) = (of(&events, "agent.mcp_tool_use")[0], of(&events, "agent.mcp_tool_result")[0]);
    assert_eq!(
        (&result["mcp_tool_use_id"], &result["content"], &result["is_error"]),
        (&call["id"], &json!([{"type": "text", "text": "grown"}]), &json!(false)),
        "read from the event stream"
    );
    assert_eq!(server.count("tools/list"), 2, "listed again after the notification");
    let asked = model.asked.lock();
    assert!(asked[1].tools.iter().any(|t| t.name == "mcp__docs__extra"), "the stored list is the new one");
}

#[tokio::test]
async fn a_step_whose_calls_outlast_its_lease_keeps_it_and_sends_each_call_once() {
    // S-2: two unanswered calls take longer than the lease; the step renews
    // it, so no claim runs the step again and repeats its calls.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let calls = vec![("mcp__docs__hang", json!({"n": 1})), ("mcp__docs__hang", json!({"n": 2}))];
    let model = Scripted::new(vec![Say::Calls(calls), Say::Text("sorry")]);
    let timeout = Some(std::time::Duration::from_millis(1500));
    let lease = std::time::Duration::from_secs(2);
    let (app, _worker) =
        worker(&rig, model, Tune { guard, mcp_timeout: timeout, lease, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("try")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let (calls, results) = (of(&events, "agent.mcp_tool_use"), of(&events, "agent.mcp_tool_result"));
    let answered: Vec<&Value> = results.iter().map(|r| &r["mcp_tool_use_id"]).collect();
    assert_eq!(answered, [&calls[0]["id"], &calls[1]["id"]], "one result for each call");
    assert_eq!(server.count("tools/call"), 2, "no claim sent a call again");
}

#[tokio::test]
async fn a_step_whose_claim_is_gone_stops_before_its_next_call() {
    // S-2: the step's row is taken away during its first call; the next
    // renewal finds the claim gone and the step sends nothing more.
    let Some(rig) = Rig::new().await else { return };
    let (server, _app, _worker, _s) = gone_during_first_call(&rig, 2).await;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert_eq!(server.count("tools/call"), 1, "the step stopped before its second call");
}

/// A session whose step, under a `lease` of that many seconds, makes two
/// 1.5 s calls and loses its row during the first.
async fn gone_during_first_call(rig: &Rig, lease: u64) -> (Arc<Server>, Router, super::Worker, String) {
    let (server, url, guard) = serve().await;
    let calls = vec![("mcp__docs__hang", json!({"n": 1})), ("mcp__docs__hang", json!({"n": 2}))];
    let model = Scripted::new(vec![Say::Calls(calls), Say::Text("sorry")]);
    let timeout = Some(std::time::Duration::from_millis(1500));
    let lease = std::time::Duration::from_secs(lease);
    let (app, worker_) =
        worker(rig, model, Tune { guard, mcp_timeout: timeout, lease, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("try")).await;
    while server.count("tools/call") == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let pg = db().await;
    let mut conn = pg.diesel.get().await.unwrap();
    let q = diesel::sql_query("DELETE FROM loop_steps WHERE session_id = $1")
        .bind::<diesel::sql_types::Text, _>(&s);
    diesel_async::RunQueryDsl::execute(q, &mut *conn).await.unwrap();
    (server, app, worker_, s)
}

/// The rig's step queue, failing the first write or commit `fails` picks.
struct FailOnce(Arc<dyn Steps>, AtomicBool, fn(&StepWrite<'_>) -> bool);

impl FailOnce {
    fn fails(&self, write: &StepWrite<'_>) -> Result<(), StoreError> {
        match (self.2)(write) && !self.1.swap(true, Ordering::SeqCst) {
            true => {
                Err(StoreError::Backend(shared::BackendError::new(std::io::Error::other("database down"))))
            }
            false => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl Steps for FailOnce {
    async fn claim(&self, owner: &str, lease: i64, per_workspace: i64) -> Result<Option<Claim>, StoreError> {
        self.0.claim(owner, lease, per_workspace).await
    }

    async fn write(&self, claim: &Claim, lease: i64, write: StepWrite<'_>) -> Result<bool, StoreError> {
        self.fails(&write)?;
        self.0.write(claim, lease, write).await
    }

    async fn commit(&self, claim: &Claim, end: StepEnd, write: StepWrite<'_>) -> Result<bool, StoreError> {
        self.fails(&write)?;
        self.0.commit(claim, end, write).await
    }

    async fn defer(&self, claim: &Claim, seconds: i64) -> Result<(), StoreError> {
        self.0.defer(claim, seconds).await
    }

    async fn counts(&self) -> Result<StepCounts, StoreError> {
        self.0.counts().await
    }
}

#[tokio::test]
async fn a_renewal_that_fails_is_tried_again_and_the_step_goes_on() {
    // S-2: the first renewal fails; the next keeps the lease, so each call
    // is sent once and the step commits.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let calls = vec![("mcp__docs__hang", json!({"n": 1})), ("mcp__docs__hang", json!({"n": 2}))];
    let model = Scripted::new(vec![Say::Calls(calls), Say::Text("sorry")]);
    let renewal = |w: &StepWrite<'_>| w.events.is_empty() && w.read.is_empty();
    let queue: Arc<dyn Steps> = Arc::new(FailOnce(rig.stores.steps.clone(), AtomicBool::new(false), renewal));
    let tune = Tune {
        guard,
        mcp_timeout: Some(std::time::Duration::from_millis(1500)),
        lease: std::time::Duration::from_secs(3),
        queue: Some(queue),
        ..Tune::default()
    };
    let (app, _worker) = worker(&rig, model, tune).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("try")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    assert_eq!(of(&events, "agent.mcp_tool_result").len(), 2, "one result for each call");
    assert_eq!(server.count("tools/call"), 2, "no claim sent a call again");
}

/// When an event of the session's log was processed.
fn processed(e: &Value) -> chrono::DateTime<chrono::Utc> {
    e["processed_at"].as_str().and_then(|t| t.parse().ok()).unwrap()
}

#[tokio::test]
async fn each_mcp_result_is_recorded_before_the_next_call_goes_out() {
    // MC-25, as live's MC-26 capture shows: each result has its own
    // `processed_at`, and the second `tools/call` goes out after the first
    // result is in the log.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let calls = vec![("mcp__docs__lookup", json!({"q": "1"})), ("mcp__docs__hang", json!({"q": "2"}))];
    let model = Scripted::new(vec![Say::Calls(calls), Say::Text("done")]);
    let timeout = Some(std::time::Duration::from_millis(1500));
    let (app, _worker) = worker(&rig, model, Tune { guard, mcp_timeout: timeout, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("look both up")).await;
    while server.calls().len() < 2 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let during = events_until(&app, &s, "agent.mcp_tool_use", 2).await;
    let (uses, results) = (of(&during, "agent.mcp_tool_use"), of(&during, "agent.mcp_tool_result"));
    assert_eq!(results.len(), 1, "only the first call's result while the second runs");
    assert_eq!(results[0]["mcp_tool_use_id"], uses[0]["id"]);
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let (results, sent) = (of(&events, "agent.mcp_tool_result"), server.calls());
    assert_eq!((results.len(), sent.len()), (2, 2));
    assert!(processed(results[0]) < sent[1].at, "the first result is recorded before the second call");
    assert!(processed(results[1]) > sent[1].at, "the second result is recorded when its call ends");
}

#[tokio::test]
async fn a_result_write_that_finds_the_claim_gone_stops_the_step() {
    // S-2: the step's row is taken away during its first call, and the lease
    // renews long after the call, so the result's write is what finds the
    // claim gone: it writes nothing and the step sends no second call.
    let Some(rig) = Rig::new().await else { return };
    let (server, app, _worker, s) = gone_during_first_call(&rig, 30).await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert_eq!(server.count("tools/call"), 1, "the step stopped at the first result's write");
    let events = events_until(&app, &s, "agent.mcp_tool_use", 2).await;
    assert!(of(&events, "agent.mcp_tool_result").is_empty(), "the result's write landed nothing");
}

#[tokio::test]
async fn a_result_already_recorded_is_not_sent_again_when_the_step_reruns() {
    // MC-25: the second result's write fails, so the step runs again; it
    // sends only the call with no result.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let calls = vec![("mcp__docs__lookup", json!({"q": "1"})), ("mcp__docs__lookup", json!({"q": "2"}))];
    let model = Scripted::new(vec![Say::Calls(calls), Say::Text("done")]);
    let second = |w: &StepWrite<'_>| serde_json::to_string(w.events).unwrap().contains(r#"found \"2\""#);
    let queue: Arc<dyn Steps> = Arc::new(FailOnce(rig.stores.steps.clone(), AtomicBool::new(false), second));
    let (app, _worker) = worker(&rig, model, Tune { guard, queue: Some(queue), ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([])).await;
    send(&app, &s, message("look both up")).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let sent: Vec<Value> = server.calls().into_iter().map(|g| g.arguments["q"].clone()).collect();
    assert_eq!(sent, [json!("1"), json!("2"), json!("2")], "the recorded call is not sent again");
    let (uses, results) = (of(&events, "agent.mcp_tool_use"), of(&events, "agent.mcp_tool_result"));
    let recorded: Vec<(&Value, &Value, &Value)> =
        results.iter().map(|r| (&r["mcp_tool_use_id"], &r["content"][0]["text"], &r["is_error"])).collect();
    assert_eq!(
        recorded,
        [
            (&uses[0]["id"], &json!("found \"1\""), &json!(false)),
            (&uses[1]["id"], &json!("found \"2\""), &json!(false))
        ],
        "one result for each call"
    );
}

#[tokio::test]
async fn a_refresh_another_worker_holds_after_a_call_ran_keeps_its_result_and_defers_the_rest() {
    // MC-20, MC-25: the token expires during the first call, which times out
    // and drops its connection; the second call's reconnect meets a refresh
    // in flight. The first result stays recorded, the second call is not
    // sent and the step waits a second, and the second call runs once the
    // refresh can be made.
    let Some(rig) = Rig::new().await else { return };
    let (server, url, guard) = serve().await;
    let calls = vec![("mcp__docs__hang", json!({"q": "1"})), ("mcp__docs__lookup", json!({"q": "2"}))];
    let model = Scripted::new(vec![Say::Calls(calls), Say::Text("done")]);
    let timeout = Some(std::time::Duration::from_secs(3));
    let (app, _worker) = worker(&rig, model, Tune { guard, mcp_timeout: timeout, ..Tune::default() }).await;
    let env = rig.environment("cloud").await;
    let soon = (chrono::Utc::now() + chrono::Duration::milliseconds(2500)).to_rfc3339();
    let (vault, cred) = expiring(&rig, &url, &url.replace("/mcp", "/token"), &soon, 0).await;
    let s = mcp_session(&app, &env, &url, Some("always_allow"), json!([vault])).await;
    send(&app, &s, message("go")).await;
    let first = events_until(&app, &s, "agent.mcp_tool_result", 1).await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let during = events_until(&app, &s, "agent.mcp_tool_use", 2).await;
    let (uses, results) = (of(&during, "agent.mcp_tool_use"), of(&during, "agent.mcp_tool_result"));
    assert_eq!(results, of(&first, "agent.mcp_tool_result"), "the first call's result is kept, and no other");
    assert_eq!(results[0]["mcp_tool_use_id"], uses[0]["id"]);
    assert_eq!(server.calls().len(), 1, "the second call waits for the refresh");
    assert_eq!(server.tokens.load(Ordering::SeqCst), 0, "no refresh while another worker's is in flight");
    let step = super::step_of(&s).await.expect("the step stays queued");
    assert!(step.due_in <= 1, "the step waits a second");
    age_row(&cred, 120).await;
    let events = events_until(&app, &s, "session.status_idle", 1).await;
    let results = of(&events, "agent.mcp_tool_result");
    let recorded: Vec<(&Value, &Value, &Value)> =
        results.iter().map(|r| (&r["mcp_tool_use_id"], &r["content"][0]["text"], &r["is_error"])).collect();
    assert_eq!(
        recorded,
        [
            (&uses[0]["id"], &json!("upstream server error (HTTP 502)"), &json!(true)),
            (&uses[1]["id"], &json!("found \"2\""), &json!(false))
        ],
        "one result for each call"
    );
    assert_eq!(server.tokens.load(Ordering::SeqCst), 1, "one refresh");
}
